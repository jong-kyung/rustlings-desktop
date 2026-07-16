use crate::{
    curriculum::{digest, Curriculum, EXERCISE_IDS},
    process::{CancelResult, CancellationToken, ProcessRunner},
    toolchain::Toolchain,
    validator::{OperationalKind, ValidationOutcome, ValidationResult, Validator},
    workspace::{Completion, SaveResult, SliceProof, Workspace},
};
use serde::Serialize;
use std::{
    collections::{BTreeMap, HashMap},
    fmt,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex, RwLock,
    },
};
use tokio::sync::watch;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExerciseStatus {
    Locked,
    Current,
    Completed,
    Unlocked,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExerciseSnapshot {
    pub id: String,
    pub status: ExerciseStatus,
    pub revision: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreflightSnapshot {
    pub ready: bool,
    pub message: Option<String>,
    pub rustc_version: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSnapshot {
    pub selected: String,
    pub source: String,
    pub source_digest: String,
    pub readme: String,
    pub exercises: Vec<ExerciseSnapshot>,
    pub active_run_id: Option<String>,
    pub slice_complete: bool,
    pub preflight: PreflightSnapshot,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunTicket {
    pub run_id: String,
    pub exercise_id: String,
    pub revision: u64,
    pub source_digest: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunResponse {
    pub run_id: String,
    pub revision: u64,
    pub stale: bool,
    pub validation: ValidationResult,
    pub final_recheck: Vec<ValidationResult>,
    pub snapshot: SessionSnapshot,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CancelRunResult {
    Requested,
    NotActive,
    IdMismatch,
}

#[derive(Debug)]
pub struct SessionError(String);

impl fmt::Display for SessionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for SessionError {}

struct CapturedRun {
    id: String,
    exercise_id: String,
    revision: u64,
    digest: String,
    source: Vec<u8>,
    cancellation: CancellationToken,
}

struct SessionState {
    active: Option<CapturedRun>,
    results: HashMap<String, watch::Sender<Option<RunResponse>>>,
}

struct FinalCapture {
    sources: BTreeMap<String, Vec<u8>>,
    proofs: Vec<Completion>,
}

pub struct Session {
    curriculum: Curriculum,
    workspace: Workspace,
    runner: ProcessRunner,
    toolchain: RwLock<Result<Toolchain, String>>,
    state: Mutex<SessionState>,
    next_run: AtomicU64,
}

impl Session {
    pub fn new(
        curriculum: Curriculum,
        workspace: Workspace,
        runner: ProcessRunner,
        toolchain: Result<Toolchain, String>,
    ) -> Self {
        Self {
            curriculum,
            workspace,
            runner,
            toolchain: RwLock::new(toolchain),
            state: Mutex::new(SessionState {
                active: None,
                results: HashMap::new(),
            }),
            next_run: AtomicU64::new(1),
        }
    }

    pub fn snapshot(&self) -> Result<SessionSnapshot, SessionError> {
        let state = self.state.lock().expect("session mutex poisoned");
        self.snapshot_locked(&state)
    }

    pub async fn retry_preflight(&self) -> PreflightSnapshot {
        let discovered = Toolchain::discover()
            .await
            .map_err(|error| error.to_string());
        *self.toolchain.write().expect("toolchain lock poisoned") = discovered;
        self.preflight()
    }

    pub fn save_source(
        &self,
        exercise_id: &str,
        expected_revision: u64,
        source: &str,
    ) -> Result<SaveResult, SessionError> {
        let state = self.state.lock().expect("session mutex poisoned");
        self.require_unlocked(exercise_id)?;
        let result = self
            .workspace
            .save_source(exercise_id, expected_revision, source.as_bytes())
            .map_err(display)?;
        drop(state);
        Ok(result)
    }

    pub fn select_exercise(&self, exercise_id: &str) -> Result<SessionSnapshot, SessionError> {
        let state = self.state.lock().expect("session mutex poisoned");
        if state.active.is_some() {
            return Err(SessionError("a validation run is active".into()));
        }
        self.require_unlocked(exercise_id)?;
        let mut progress = self.workspace.progress();
        progress.selected = exercise_id.to_owned();
        self.workspace.save_progress(&progress).map_err(display)?;
        self.snapshot_locked(&state)
    }

    pub fn reveal_hint(&self, exercise_id: &str) -> Result<String, SessionError> {
        let _state = self.state.lock().expect("session mutex poisoned");
        self.require_unlocked(exercise_id)?;
        self.curriculum
            .exercise(exercise_id)
            .map(|exercise| exercise.hint.clone())
            .ok_or_else(|| SessionError("unknown exercise".into()))
    }

    pub fn start_run(self: &Arc<Self>, exercise_id: &str) -> Result<RunTicket, SessionError> {
        if self
            .toolchain
            .read()
            .expect("toolchain lock poisoned")
            .is_err()
        {
            return Err(SessionError("Rust toolchain preflight is not ready".into()));
        }
        let mut state = self.state.lock().expect("session mutex poisoned");
        if state.active.is_some() {
            return Err(SessionError("a validation run is active".into()));
        }
        self.require_unlocked(exercise_id)?;
        if self.workspace.progress().selected != exercise_id {
            return Err(SessionError("only the selected exercise can be run".into()));
        }
        let source = self.workspace.source(exercise_id).map_err(display)?;
        let revision = self.workspace.revision(exercise_id).map_err(display)?;
        let source_digest = digest(&source);
        let run_number = self.next_run.fetch_add(1, Ordering::Relaxed);
        assert_ne!(run_number, 0, "session run ID space exhausted");
        let run_id = format!("{}-{run_number}", std::process::id());
        let cancellation = CancellationToken::new();
        let captured = CapturedRun {
            id: run_id.clone(),
            exercise_id: exercise_id.to_owned(),
            revision,
            digest: source_digest.clone(),
            source,
            cancellation,
        };
        state.results.retain(|_, sender| sender.borrow().is_none());
        let (sender, _receiver) = watch::channel(None);
        state.results.insert(run_id.clone(), sender);
        state.active = Some(captured);
        drop(state);

        let session = Arc::clone(self);
        tauri::async_runtime::spawn(async move {
            session.execute_run().await;
        });
        Ok(RunTicket {
            run_id,
            exercise_id: exercise_id.to_owned(),
            revision,
            source_digest,
        })
    }

    pub async fn await_run(&self, run_id: &str) -> Result<RunResponse, SessionError> {
        let mut receiver = {
            let state = self.state.lock().expect("session mutex poisoned");
            state
                .results
                .get(run_id)
                .ok_or_else(|| SessionError("unknown run ID".into()))?
                .subscribe()
        };
        loop {
            if let Some(result) = receiver.borrow().clone() {
                return Ok(result);
            }
            receiver
                .changed()
                .await
                .map_err(|_| SessionError("run result channel closed".into()))?;
        }
    }

    pub async fn cancel_run(&self, run_id: &str) -> CancelRunResult {
        let cancellation = {
            let state = self.state.lock().expect("session mutex poisoned");
            let Some(active) = state.active.as_ref() else {
                return CancelRunResult::NotActive;
            };
            if active.id != run_id {
                return CancelRunResult::IdMismatch;
            }
            active.cancellation.clone()
        };
        match self.runner.cancel_token(&cancellation).await {
            CancelResult::Requested | CancelResult::NotActive => CancelRunResult::Requested,
            CancelResult::IdMismatch => CancelRunResult::IdMismatch,
        }
    }

    pub async fn shutdown(&self) {
        self.runner.shutdown().await;
    }

    async fn execute_run(self: Arc<Self>) {
        let (id, exercise_id, revision, source, source_digest, cancellation) = {
            let state = self.state.lock().expect("session mutex poisoned");
            let active = state.active.as_ref().expect("captured run disappeared");
            (
                active.id.clone(),
                active.exercise_id.clone(),
                active.revision,
                active.source.clone(),
                active.digest.clone(),
                active.cancellation.clone(),
            )
        };
        let validation = self
            .validate_captured(&exercise_id, &source, &source_digest, cancellation.clone())
            .await;
        let mut final_recheck = Vec::new();
        let (mut stale, final_capture) = self.commit_normal(&id, &validation);
        if let Some(capture) = final_capture {
            final_recheck = self.validate_all(&capture.sources, cancellation).await;
            stale |= self.commit_final(&id, &capture.proofs, &final_recheck);
        }
        let response = {
            let mut state = self.state.lock().expect("session mutex poisoned");
            if state.active.as_ref().is_some_and(|active| active.id == id) {
                state.active = None;
            } else {
                stale = true;
            }
            RunResponse {
                run_id: id.clone(),
                revision,
                stale,
                validation,
                final_recheck,
                snapshot: self
                    .snapshot_locked(&state)
                    .expect("session snapshot failed after validation"),
            }
        };
        let state = self.state.lock().expect("session mutex poisoned");
        if let Some(sender) = state.results.get(&id) {
            sender.send_replace(Some(response));
        }
    }

    async fn validate_captured(
        &self,
        exercise_id: &str,
        source: &[u8],
        source_digest: &str,
        cancellation: CancellationToken,
    ) -> ValidationResult {
        let toolchain = match self
            .toolchain
            .read()
            .expect("toolchain lock poisoned")
            .clone()
        {
            Ok(toolchain) => toolchain,
            Err(message) => {
                return operational(
                    exercise_id,
                    source_digest,
                    OperationalKind::Infrastructure,
                    message,
                )
            }
        };
        let validator = Validator::new(&self.curriculum, &toolchain, &self.runner, &self.workspace)
            .with_cancellation(cancellation);
        match validator.snapshot(&BTreeMap::from([(exercise_id.to_owned(), source.to_vec())])) {
            Ok(snapshot) => validator.validate_snapshot(exercise_id, &snapshot).await,
            Err(error) => operational(
                exercise_id,
                source_digest,
                OperationalKind::Storage,
                error.to_string(),
            ),
        }
    }

    async fn validate_all(
        &self,
        sources: &BTreeMap<String, Vec<u8>>,
        cancellation: CancellationToken,
    ) -> Vec<ValidationResult> {
        let toolchain = match self
            .toolchain
            .read()
            .expect("toolchain lock poisoned")
            .clone()
        {
            Ok(toolchain) => toolchain,
            Err(message) => {
                return vec![operational(
                    "intro1",
                    "",
                    OperationalKind::Infrastructure,
                    message,
                )]
            }
        };
        let validator = Validator::new(&self.curriculum, &toolchain, &self.runner, &self.workspace)
            .with_cancellation(cancellation);
        let snapshot = match validator.snapshot(sources) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                return vec![operational(
                    "intro1",
                    "",
                    OperationalKind::Storage,
                    error.to_string(),
                )]
            }
        };
        let mut results = Vec::with_capacity(EXERCISE_IDS.len());
        for id in EXERCISE_IDS {
            let result = validator.validate_snapshot(id, &snapshot).await;
            let passed = result.outcome == ValidationOutcome::Passed;
            results.push(result);
            if !passed {
                break;
            }
        }
        results
    }

    fn commit_normal(
        &self,
        run_id: &str,
        validation: &ValidationResult,
    ) -> (bool, Option<FinalCapture>) {
        let state = self.state.lock().expect("session mutex poisoned");
        let Some(active) = state.active.as_ref() else {
            return (true, None);
        };
        if active.id != run_id
            || active.exercise_id != validation.exercise_id
            || active.digest != validation.source_digest
            || self.workspace.progress().selected != active.exercise_id
            || self
                .workspace
                .source_digest(&active.exercise_id)
                .ok()
                .as_deref()
                != Some(active.digest.as_str())
        {
            return (true, None);
        }
        let index = exercise_index(&active.exercise_id).expect("captured exercise is known");
        let mut progress = self.workspace.progress();
        apply_validation_outcome(
            &mut progress,
            index,
            &active.exercise_id,
            &active.digest,
            &validation.outcome,
        );
        if progress != self.workspace.progress() && self.workspace.save_progress(&progress).is_err()
        {
            return (false, None);
        }
        if validation.outcome != ValidationOutcome::Passed || index + 1 != EXERCISE_IDS.len() {
            return (false, None);
        }

        let mut sources = BTreeMap::new();
        let mut proofs = Vec::with_capacity(EXERCISE_IDS.len());
        for id in EXERCISE_IDS {
            let Ok(bytes) = self.workspace.source(id) else {
                return (false, None);
            };
            let source_digest = digest(&bytes);
            sources.insert(id.to_owned(), bytes);
            proofs.push(Completion {
                id: id.to_owned(),
                digest: source_digest,
            });
        }
        (false, Some(FinalCapture { sources, proofs }))
    }

    fn commit_final(
        &self,
        run_id: &str,
        proofs: &[Completion],
        results: &[ValidationResult],
    ) -> bool {
        let state = self.state.lock().expect("session mutex poisoned");
        let Some(active) = state.active.as_ref() else {
            return true;
        };
        if active.id != run_id
            || active.exercise_id != EXERCISE_IDS[EXERCISE_IDS.len() - 1]
            || self.workspace.progress().selected != active.exercise_id
            || proofs.iter().any(|proof| {
                self.workspace.source_digest(&proof.id).ok().as_deref()
                    != Some(proof.digest.as_str())
            })
        {
            return true;
        }
        let mut progress = self.workspace.progress();
        if results.len() == EXERCISE_IDS.len()
            && results
                .iter()
                .all(|result| result.outcome == ValidationOutcome::Passed)
        {
            progress.completed = proofs.to_vec();
            progress.slice_complete = Some(SliceProof {
                sources: proofs.to_vec(),
            });
        } else if let Some(failed) = results
            .iter()
            .find(|result| matches!(result.outcome, ValidationOutcome::LearnerFailure { .. }))
        {
            let failed_index = exercise_index(&failed.exercise_id).expect("validated ID is known");
            progress.completed.truncate(failed_index);
            progress.slice_complete = None;
            progress.selected = failed.exercise_id.clone();
        }
        if progress != self.workspace.progress() {
            let _ = self.workspace.save_progress(&progress);
        }
        false
    }

    fn require_unlocked(&self, exercise_id: &str) -> Result<usize, SessionError> {
        let index = exercise_index(exercise_id)
            .ok_or_else(|| SessionError(format!("unknown exercise: {exercise_id}")))?;
        let progress = self.workspace.progress();
        if progress.completed.len() < EXERCISE_IDS.len() && index > progress.completed.len() {
            return Err(SessionError(format!("exercise is locked: {exercise_id}")));
        }
        Ok(index)
    }

    fn snapshot_locked(&self, state: &SessionState) -> Result<SessionSnapshot, SessionError> {
        let progress = self.workspace.progress();
        let selected_index = exercise_index(&progress.selected)
            .ok_or_else(|| SessionError("progress selected an unknown exercise".into()))?;
        let source = self.workspace.source(&progress.selected).map_err(display)?;
        let source = String::from_utf8(source)
            .map_err(|error| SessionError(format!("answer is not UTF-8: {error}")))?;
        let source_digest = self
            .workspace
            .source_digest(&progress.selected)
            .map_err(display)?;
        let mut exercises = Vec::with_capacity(EXERCISE_IDS.len());
        for (index, id) in EXERCISE_IDS.into_iter().enumerate() {
            let status = if index == selected_index {
                ExerciseStatus::Current
            } else if index < progress.completed.len() {
                ExerciseStatus::Completed
            } else if progress.completed.len() == EXERCISE_IDS.len()
                || index <= progress.completed.len()
            {
                ExerciseStatus::Unlocked
            } else {
                ExerciseStatus::Locked
            };
            exercises.push(ExerciseSnapshot {
                id: id.to_owned(),
                status,
                revision: self.workspace.revision(id).map_err(display)?,
            });
        }
        Ok(SessionSnapshot {
            selected: progress.selected.clone(),
            source,
            source_digest,
            readme: self
                .curriculum
                .readme(&progress.selected)
                .map_err(display)?,
            exercises,
            active_run_id: state.active.as_ref().map(|active| active.id.clone()),
            slice_complete: progress.slice_complete.is_some(),
            preflight: self.preflight(),
        })
    }

    fn preflight(&self) -> PreflightSnapshot {
        match &*self.toolchain.read().expect("toolchain lock poisoned") {
            Ok(toolchain) => PreflightSnapshot {
                ready: true,
                message: None,
                rustc_version: Some(format!(
                    "{}.{}.{}",
                    toolchain.rustc_version().major,
                    toolchain.rustc_version().minor,
                    toolchain.rustc_version().patch
                )),
            },
            Err(message) => PreflightSnapshot {
                ready: false,
                message: Some(message.clone()),
                rustc_version: None,
            },
        }
    }
}

fn exercise_index(exercise_id: &str) -> Option<usize> {
    EXERCISE_IDS.iter().position(|id| *id == exercise_id)
}

fn apply_validation_outcome(
    progress: &mut crate::workspace::Progress,
    index: usize,
    exercise_id: &str,
    source_digest: &str,
    outcome: &ValidationOutcome,
) {
    match outcome {
        ValidationOutcome::Passed => {
            if progress.completed.len() == index {
                progress.completed.push(Completion {
                    id: exercise_id.to_owned(),
                    digest: source_digest.to_owned(),
                });
            }
            if index + 1 < EXERCISE_IDS.len() {
                progress.selected = EXERCISE_IDS[index + 1].into();
            }
        }
        ValidationOutcome::LearnerFailure { .. } => {
            progress.completed.truncate(index);
            progress.slice_complete = None;
            progress.selected = exercise_id.to_owned();
        }
        _ => {}
    }
}

fn operational(
    exercise_id: &str,
    source_digest: &str,
    kind: OperationalKind,
    message: String,
) -> ValidationResult {
    ValidationResult {
        exercise_id: exercise_id.to_owned(),
        source_digest: source_digest.to_owned(),
        outcome: ValidationOutcome::OperationalFailure { kind, message },
        stages: Vec::new(),
        diagnostics: Vec::new(),
    }
}

fn display(error: impl fmt::Display) -> SessionError {
    SessionError(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        curriculum::CurriculumIdentity, diagnostics::ValidationStage, workspace::Progress,
    };

    fn completed_progress() -> Progress {
        let completed = EXERCISE_IDS
            .iter()
            .map(|id| Completion {
                id: (*id).into(),
                digest: "a".repeat(64),
            })
            .collect::<Vec<_>>();
        Progress {
            schema_version: 1,
            curriculum: CurriculumIdentity {
                rustlings_version: "6.5.0".into(),
                upstream_commit: "test".into(),
            },
            selected: "variables6".into(),
            completed: completed.clone(),
            slice_complete: Some(SliceProof { sources: completed }),
        }
    }

    #[test]
    fn only_definitive_learner_outcomes_revoke_progress() {
        let baseline = completed_progress();
        let preserving = [
            ValidationOutcome::Cancelled,
            ValidationOutcome::TimedOut,
            ValidationOutcome::OutputLimit,
            ValidationOutcome::OperationalFailure {
                kind: OperationalKind::Spawn,
                message: "spawn".into(),
            },
            ValidationOutcome::OperationalFailure {
                kind: OperationalKind::Busy,
                message: "busy".into(),
            },
            ValidationOutcome::OperationalFailure {
                kind: OperationalKind::Storage,
                message: "storage".into(),
            },
            ValidationOutcome::OperationalFailure {
                kind: OperationalKind::CargoStartup,
                message: "parser/startup".into(),
            },
        ];
        for outcome in preserving {
            let mut progress = baseline.clone();
            apply_validation_outcome(&mut progress, 2, "variables1", "b", &outcome);
            assert_eq!(progress, baseline, "{outcome:?}");
        }

        let mut failed = baseline;
        apply_validation_outcome(
            &mut failed,
            2,
            "variables1",
            "b",
            &ValidationOutcome::LearnerFailure {
                stage: ValidationStage::Build,
            },
        );
        assert_eq!(failed.completed.len(), 2);
        assert_eq!(failed.selected, "variables1");
        assert!(failed.slice_complete.is_none());
    }
}
