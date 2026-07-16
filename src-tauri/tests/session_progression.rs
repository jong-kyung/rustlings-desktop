#![cfg(unix)]

use app_lib::{
    curriculum::{Curriculum, EXERCISE_IDS},
    process::ProcessRunner,
    session::{CancelRunResult, ExerciseStatus, Session},
    toolchain::Toolchain,
    validator::ValidationOutcome,
    workspace::{
        Completion, Progress, Workspace, WorkspaceError, WorkspaceOwner, MAX_SOURCE_BYTES,
    },
};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const PASSING_SOURCE: &str = "fn main() {}\n";

struct TestDir(PathBuf);

impl TestDir {
    fn new(label: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "lustlings-u5-{label}-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn curriculum() -> Curriculum {
    Curriculum::load(Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/rustlings-6.5.0"))
        .unwrap()
}

fn workspace(app_data: &Path, curriculum: &Curriculum) -> Workspace {
    Workspace::open(WorkspaceOwner::acquire(app_data).unwrap(), curriculum).unwrap()
}

async fn open_session(app_data: &Path) -> Arc<Session> {
    let curriculum = curriculum();
    let workspace = workspace(app_data, &curriculum);
    let toolchain = Toolchain::discover()
        .await
        .map_err(|error| error.to_string());
    Arc::new(Session::new(
        curriculum,
        workspace,
        ProcessRunner::new(),
        toolchain,
    ))
}

fn progress_with_prefix(workspace: &Workspace, count: usize, selected: &str) -> Progress {
    Progress {
        schema_version: 1,
        curriculum: curriculum().identity().clone(),
        selected: selected.into(),
        completed: EXERCISE_IDS[..count]
            .iter()
            .map(|id| Completion {
                id: (*id).into(),
                digest: workspace.source_digest(id).unwrap(),
            })
            .collect(),
        slice_complete: None,
    }
}

#[tokio::test]
async fn initial_authority_rejects_locked_unknown_path_like_and_oversized_inputs() {
    let app_data = TestDir::new("authority");
    let session = open_session(&app_data.0).await;
    let snapshot = session.snapshot().unwrap();
    assert_eq!(snapshot.selected, "intro1");
    assert_eq!(snapshot.exercises[0].status, ExerciseStatus::Current);
    assert!(snapshot.exercises[1..]
        .iter()
        .all(|exercise| exercise.status == ExerciseStatus::Locked));
    assert!(snapshot.readme.contains("Intro"));
    assert!(!session.reveal_hint("intro1").unwrap().is_empty());

    for id in ["intro2", "../intro1", "--manifest-path", "unknown"] {
        assert!(session.save_source(id, 0, "changed").is_err(), "{id}");
        assert!(session.select_exercise(id).is_err(), "{id}");
        assert!(session.reveal_hint(id).is_err(), "{id}");
        assert!(session.start_run(id).is_err(), "{id}");
    }
    assert!(session
        .save_source("intro1", 0, &"x".repeat(MAX_SOURCE_BYTES + 1))
        .is_err());
    assert!(session.snapshot().unwrap().source.len() < MAX_SOURCE_BYTES);
}

#[tokio::test]
async fn revision_safe_save_preserves_identical_completion_and_changed_save_revokes_downstream() {
    let app_data = TestDir::new("save");
    let curriculum = curriculum();
    let workspace = workspace(&app_data.0, &curriculum);
    let original = String::from_utf8(workspace.source("intro1").unwrap()).unwrap();
    workspace
        .save_progress(&progress_with_prefix(&workspace, 2, "variables1"))
        .unwrap();
    let session = Arc::new(Session::new(
        curriculum,
        workspace,
        ProcessRunner::new(),
        Err("not needed".into()),
    ));

    let identical = session.save_source("intro1", 0, &original).unwrap();
    assert_eq!(identical.revision, 1);
    assert_eq!(
        session.snapshot().unwrap().exercises[0].status,
        ExerciseStatus::Completed
    );
    session.select_exercise("intro1").unwrap();
    assert!(session.save_source("intro1", 0, "stale").is_err());
    assert_eq!(session.snapshot().unwrap().source, original);

    session.save_source("intro1", 1, "durably changed").unwrap();
    let snapshot = session.snapshot().unwrap();
    assert_eq!(snapshot.selected, "intro1");
    assert_eq!(snapshot.exercises[1].status, ExerciseStatus::Locked);
    drop(session);

    let reopened = open_session(&app_data.0).await;
    assert_eq!(reopened.snapshot().unwrap().source, "durably changed");
    assert_eq!(reopened.snapshot().unwrap().selected, "intro1");
}

#[tokio::test]
async fn passing_runs_unlock_one_at_a_time_and_final_recheck_commits_all_eight_digests() {
    let app_data = TestDir::new("all-pass");
    let session = open_session(&app_data.0).await;
    for (index, id) in EXERCISE_IDS.into_iter().enumerate() {
        let revision = session
            .snapshot()
            .unwrap()
            .exercises
            .iter()
            .find(|exercise| exercise.id == id)
            .unwrap()
            .revision;
        session.save_source(id, revision, PASSING_SOURCE).unwrap();
        let ticket = session.start_run(id).unwrap();
        let result = session.await_run(&ticket.run_id).await.unwrap();
        assert!(!result.stale, "{id}: {result:#?}");
        assert_eq!(result.validation.outcome, ValidationOutcome::Passed, "{id}");
        if index + 1 < EXERCISE_IDS.len() {
            assert_eq!(result.snapshot.selected, EXERCISE_IDS[index + 1]);
            assert_eq!(
                result.snapshot.exercises[index + 1].status,
                ExerciseStatus::Current
            );
            assert_eq!(
                result.snapshot.exercises[index + 2..]
                    .iter()
                    .filter(|exercise| exercise.status != ExerciseStatus::Locked)
                    .count(),
                0
            );
        } else {
            assert_eq!(result.final_recheck.len(), EXERCISE_IDS.len());
            assert!(result
                .final_recheck
                .iter()
                .all(|check| check.outcome == ValidationOutcome::Passed));
            assert!(result.snapshot.slice_complete);
        }
    }
    drop(session);

    let reopened = open_session(&app_data.0).await;
    let snapshot = reopened.snapshot().unwrap();
    assert!(snapshot.slice_complete);
    assert!(snapshot
        .exercises
        .iter()
        .all(|exercise| exercise.status != ExerciseStatus::Locked));
}

#[tokio::test]
async fn run_snapshot_becomes_stale_after_save_and_cannot_restore_progress() {
    let app_data = TestDir::new("stale");
    let session = open_session(&app_data.0).await;
    session.save_source("intro1", 0, PASSING_SOURCE).unwrap();
    let ticket = session.start_run("intro1").unwrap();
    session
        .save_source("intro1", ticket.revision, "fn main() { missing(); }\n")
        .unwrap();
    let result = session.await_run(&ticket.run_id).await.unwrap();
    assert!(result.stale);
    assert_eq!(result.validation.source_digest, ticket.source_digest);
    assert_eq!(result.snapshot.selected, "intro1");
    assert_eq!(result.snapshot.exercises[1].status, ExerciseStatus::Locked);
}

#[tokio::test]
async fn busy_cancel_and_late_cancel_are_scoped_to_the_matching_session_run() {
    let app_data = TestDir::new("cancel");
    let session = open_session(&app_data.0).await;
    session
        .save_source(
            "intro1",
            0,
            "fn main() { loop { std::hint::spin_loop(); } }\n",
        )
        .unwrap();
    let ticket = session.start_run("intro1").unwrap();
    assert!(session.start_run("intro1").is_err());
    assert_eq!(
        session.cancel_run("not-the-active-run").await,
        CancelRunResult::IdMismatch
    );
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        session.cancel_run(&ticket.run_id).await,
        CancelRunResult::Requested
    );
    let cancelled = session.await_run(&ticket.run_id).await.unwrap();
    assert!(!cancelled.stale);
    assert_eq!(cancelled.validation.outcome, ValidationOutcome::Cancelled);
    assert_eq!(cancelled.snapshot.selected, "intro1");
    assert_eq!(
        session.cancel_run(&ticket.run_id).await,
        CancelRunResult::NotActive
    );

    let revision = cancelled.snapshot.exercises[0].revision;
    session
        .save_source("intro1", revision, PASSING_SOURCE)
        .unwrap();
    let next = session.start_run("intro1").unwrap();
    let passed = session.await_run(&next.run_id).await.unwrap();
    assert_eq!(passed.validation.outcome, ValidationOutcome::Passed);
}

#[tokio::test]
async fn definitive_learner_failure_revokes_that_exercise_and_downstream() {
    let app_data = TestDir::new("learner-failure");
    let curriculum = curriculum();
    let workspace = workspace(&app_data.0, &curriculum);
    workspace
        .save_source("intro1", 0, b"fn main() { missing(); }\n")
        .unwrap();
    workspace
        .save_progress(&progress_with_prefix(&workspace, 1, "intro1"))
        .unwrap();
    let toolchain = Toolchain::discover()
        .await
        .map_err(|error| error.to_string());
    let session = Arc::new(Session::new(
        curriculum,
        workspace,
        ProcessRunner::new(),
        toolchain,
    ));

    let ticket = session.start_run("intro1").unwrap();
    let result = session.await_run(&ticket.run_id).await.unwrap();
    assert!(matches!(
        result.validation.outcome,
        ValidationOutcome::LearnerFailure { .. }
    ));
    assert_eq!(result.snapshot.selected, "intro1");
    assert_eq!(result.snapshot.exercises[1].status, ExerciseStatus::Locked);
    assert!(!result.snapshot.slice_complete);
}

#[tokio::test]
async fn edit_during_final_recheck_rejects_the_captured_all_source_proof() {
    let app_data = TestDir::new("final-stale");
    let curriculum = curriculum();
    let workspace = workspace(&app_data.0, &curriculum);
    for id in EXERCISE_IDS {
        workspace
            .save_source(id, 0, PASSING_SOURCE.as_bytes())
            .unwrap();
    }
    workspace
        .save_progress(&progress_with_prefix(&workspace, 7, "variables6"))
        .unwrap();
    let generated = workspace.generated_dir().to_owned();
    let toolchain = Toolchain::discover()
        .await
        .map_err(|error| error.to_string());
    let session = Arc::new(Session::new(
        curriculum,
        workspace,
        ProcessRunner::new(),
        toolchain,
    ));

    let ticket = session.start_run("variables6").unwrap();
    let mut first_snapshot = None;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let current = fs::read_dir(&generated)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name())
            .find(|name| name.to_string_lossy().starts_with("validation-"));
        if let Some(current) = current {
            match &first_snapshot {
                None => first_snapshot = Some(current),
                Some(first) if first != &current => break,
                Some(_) => {}
            }
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "final snapshot did not start"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    session
        .save_source("intro1", 1, "fn main() { changed_during_recheck(); }\n")
        .unwrap();
    let result = session.await_run(&ticket.run_id).await.unwrap();
    assert!(result.stale);
    assert_eq!(result.final_recheck.len(), EXERCISE_IDS.len());
    assert!(!result.snapshot.slice_complete);
    assert_eq!(result.snapshot.selected, "intro1");
}

#[tokio::test]
async fn operational_failure_and_second_owner_guard_preserve_durable_progress() {
    let app_data = TestDir::new("operational");
    let curriculum = curriculum();
    let workspace = workspace(&app_data.0, &curriculum);
    workspace
        .save_progress(&progress_with_prefix(&workspace, 1, "intro1"))
        .unwrap();
    let manifest = workspace.root().join("Cargo.toml");
    let state_path = workspace.state_path().to_owned();
    let state_before = fs::read(&state_path).unwrap();
    assert!(matches!(
        WorkspaceOwner::acquire(&app_data.0),
        Err(WorkspaceError::OwnershipUnavailable)
    ));
    assert_eq!(fs::read(&state_path).unwrap(), state_before);

    let toolchain = Toolchain::discover()
        .await
        .map_err(|error| error.to_string());
    let session = Arc::new(Session::new(
        curriculum,
        workspace,
        ProcessRunner::new(),
        toolchain,
    ));
    fs::write(&manifest, b"tampered infrastructure").unwrap();
    let ticket = session.start_run("intro1").unwrap();
    let result = session.await_run(&ticket.run_id).await.unwrap();
    assert!(matches!(
        result.validation.outcome,
        ValidationOutcome::OperationalFailure { .. }
    ));
    assert_eq!(result.snapshot.exercises[0].status, ExerciseStatus::Current);
    assert_eq!(
        result.snapshot.exercises[1].status,
        ExerciseStatus::Unlocked
    );
}
