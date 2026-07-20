use crate::{
    curriculum::{digest, Curriculum, EXERCISE_IDS},
    process::{CancellationToken, ProcessRunner},
    toolchain::Toolchain,
    workspace::{SaveResult, Workspace},
};
use serde::Serialize;
use std::{
    collections::HashMap,
    fmt,
    sync::{atomic::AtomicU64, Mutex, RwLock},
};
use tokio::sync::watch;

mod run;
pub use run::{ActiveRunSnapshot, CancelRunResult, RunResponse, RunTarget, RunTicket};

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
    pub source_path: String,
    pub solution_path: String,
    pub solution_available: bool,
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
    pub active_run: Option<ActiveRunSnapshot>,
    pub curriculum_complete: bool,
    pub preflight: PreflightSnapshot,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SolutionResponse {
    pub exercise_id: String,
    pub path: String,
    pub source: String,
    pub source_digest: String,
    pub readme: String,
}

#[derive(Clone, Debug)]
pub struct SessionError(String);

impl fmt::Display for SessionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for SessionError {}

struct CapturedRun {
    id: String,
    target: RunTarget,
    revision: u64,
    digest: String,
    source: Vec<u8>,
    cancellation: CancellationToken,
}

struct SessionState {
    active: Option<CapturedRun>,
    results: HashMap<String, watch::Sender<Option<Result<RunResponse, SessionError>>>>,
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

    pub fn reveal_solution(&self, exercise_id: &str) -> Result<SolutionResponse, SessionError> {
        let _state = self.state.lock().expect("session mutex poisoned");
        self.require_solution_authorized(exercise_id)?;
        let exercise = self
            .curriculum
            .exercise(exercise_id)
            .ok_or_else(|| SessionError(format!("unknown exercise: {exercise_id}")))?;
        let source = self
            .curriculum
            .solution_bytes(exercise_id)
            .map_err(display)?;
        let source_digest = digest(&source);
        let source = String::from_utf8(source)
            .map_err(|error| SessionError(format!("solution is not UTF-8: {error}")))?;
        Ok(SolutionResponse {
            exercise_id: exercise_id.to_owned(),
            path: exercise.solution.clone(),
            source,
            source_digest,
            readme: self.curriculum.readme(exercise_id).map_err(display)?,
        })
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

    fn require_solution_authorized(&self, exercise_id: &str) -> Result<(), SessionError> {
        let index = exercise_index(exercise_id)
            .ok_or_else(|| SessionError(format!("unknown exercise: {exercise_id}")))?;
        let progress = self.workspace.progress();
        let authorized = progress.completed.get(index).is_some_and(|proof| {
            proof.id == exercise_id
                && self.workspace.source_digest(exercise_id).ok().as_deref()
                    == Some(proof.digest.as_str())
        });
        if !authorized {
            return Err(SessionError(
                "solution is available after completing the exercise".into(),
            ));
        }
        Ok(())
    }

    fn snapshot_locked(&self, state: &SessionState) -> Result<SessionSnapshot, SessionError> {
        let progress = self.workspace.progress();
        let selected_index = exercise_index(&progress.selected)
            .ok_or_else(|| SessionError("progress selected an unknown exercise".into()))?;
        let source = self.workspace.source(&progress.selected).map_err(display)?;
        let source_digest = digest(&source);
        let source = String::from_utf8(source)
            .map_err(|error| SessionError(format!("answer is not UTF-8: {error}")))?;
        let mut exercises = Vec::with_capacity(EXERCISE_IDS.len());
        for (index, exercise) in self.curriculum.exercises().iter().enumerate() {
            let id = exercise.id.as_str();
            let solution_available = progress.completed.get(index).is_some_and(|proof| {
                proof.id == id
                    && self.workspace.source_digest(id).ok().as_deref()
                        == Some(proof.digest.as_str())
            });
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
                source_path: exercise.source.clone(),
                solution_path: exercise.solution.clone(),
                solution_available,
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
            active_run: state.active.as_ref().map(|active| ActiveRunSnapshot {
                run_id: active.id.clone(),
                target: active.target.clone(),
                source_digest: active.digest.clone(),
            }),
            curriculum_complete: progress.curriculum_complete.is_some(),
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

fn display(error: impl fmt::Display) -> SessionError {
    SessionError(error.to_string())
}
