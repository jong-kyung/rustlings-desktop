#![cfg(unix)]

use app_lib::{
    curriculum::{Curriculum, EXERCISE_IDS},
    process::ProcessRunner,
    session::{CancelRunResult, ExerciseStatus, Session},
    toolchain::Toolchain,
    validator::{OperationalKind, ValidationOutcome},
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
const SLOW_PASSING_SOURCE: &str =
    "fn main() { std::thread::sleep(std::time::Duration::from_millis(250)); }\n";
const FAILING_PROGRAM_SOURCE: &str = "fn main() { panic!(\"expected failure\"); }\n";

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
        schema_version: 2,
        curriculum: curriculum().identity().clone(),
        selected: selected.into(),
        completed: EXERCISE_IDS[..count]
            .iter()
            .map(|id| Completion {
                id: (*id).into(),
                digest: workspace.source_digest(id).unwrap(),
            })
            .collect(),
        curriculum_complete: None,
    }
}

async fn wait_for_file(path: &Path) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !path.exists() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "validation program did not start"
        );
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}

fn replace_state_file_with_directory(state_path: &Path) -> Vec<u8> {
    let bytes = fs::read(state_path).unwrap();
    fs::remove_file(state_path).unwrap();
    fs::create_dir(state_path).unwrap();
    bytes
}

fn restore_state_file(state_path: &Path, bytes: &[u8]) {
    fs::remove_dir(state_path).unwrap();
    fs::write(state_path, bytes).unwrap();
}

fn workspace_root(app_data: &Path) -> PathBuf {
    fs::read_dir(app_data)
        .unwrap()
        .find_map(|entry| {
            let path = entry.unwrap().path();
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("workspace-v1-")
                .then_some(path)
        })
        .unwrap()
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
async fn final_two_runs_follow_manifest_order_and_recheck_all_94_sources() {
    let app_data = TestDir::new("all-pass");
    let curriculum = curriculum();
    let workspace = workspace(&app_data.0, &curriculum);
    for id in EXERCISE_IDS {
        workspace
            .save_source(id, 0, PASSING_SOURCE.as_bytes())
            .unwrap();
    }
    let penultimate = EXERCISE_IDS.len() - 2;
    workspace
        .save_progress(&progress_with_prefix(
            &workspace,
            penultimate,
            EXERCISE_IDS[penultimate],
        ))
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

    let ticket = session.start_run(EXERCISE_IDS[penultimate]).unwrap();
    let result = session.await_run(&ticket.run_id).await.unwrap();
    assert_eq!(result.validation.outcome, ValidationOutcome::Passed);
    assert_eq!(result.snapshot.selected, EXERCISE_IDS[penultimate + 1]);
    assert!(result.final_recheck.is_empty());

    let ticket = session.start_run(EXERCISE_IDS[penultimate + 1]).unwrap();
    let result = session.await_run(&ticket.run_id).await.unwrap();
    assert_eq!(result.validation.outcome, ValidationOutcome::Passed);
    assert_eq!(result.final_recheck.len(), EXERCISE_IDS.len());
    assert!(result
        .final_recheck
        .iter()
        .zip(EXERCISE_IDS)
        .all(|(check, id)| check.exercise_id == id && check.outcome == ValidationOutcome::Passed));
    assert!(result.snapshot.curriculum_complete);
    drop(session);

    let reopened = open_session(&app_data.0).await;
    let snapshot = reopened.snapshot().unwrap();
    assert!(snapshot.curriculum_complete);
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
    assert!(!result.snapshot.curriculum_complete);
}

#[tokio::test]
async fn edit_during_final_recheck_rejects_the_captured_all_source_proof() {
    let app_data = TestDir::new("final-stale");
    let curriculum = curriculum();
    let workspace = workspace(&app_data.0, &curriculum);
    let marker = app_data.0.join("final-recheck-started");
    let intro_source = format!(
        "fn main() {{ std::fs::write({marker:?}, b\"started\").unwrap(); std::thread::sleep(std::time::Duration::from_millis(250)); panic!(\"expected failure\"); }}\n"
    );
    for id in EXERCISE_IDS {
        workspace
            .save_source(
                id,
                0,
                if id == "intro1" {
                    intro_source.as_bytes()
                } else {
                    PASSING_SOURCE.as_bytes()
                },
            )
            .unwrap();
    }
    let last = EXERCISE_IDS.len() - 1;
    workspace
        .save_progress(&progress_with_prefix(&workspace, last, EXERCISE_IDS[last]))
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

    let ticket = session.start_run(EXERCISE_IDS[last]).unwrap();
    wait_for_file(&marker).await;

    session
        .save_source("intro1", 1, "fn main() { changed_during_recheck(); }\n")
        .unwrap();
    let result = session.await_run(&ticket.run_id).await.unwrap();
    assert!(result.stale);
    assert_eq!(result.final_recheck.len(), 1);
    assert!(!result.snapshot.curriculum_complete);
    assert_eq!(result.snapshot.selected, "intro1");
}

#[tokio::test]
async fn progress_commit_failures_surface_storage_outcomes_without_losing_stages() {
    for final_commit in [false, true] {
        let app_data = TestDir::new(if final_commit {
            "final-commit-failure"
        } else {
            "normal-commit-failure"
        });
        let curriculum = curriculum();
        let workspace = workspace(&app_data.0, &curriculum);
        if final_commit {
            for id in EXERCISE_IDS {
                workspace
                    .save_source(id, 0, PASSING_SOURCE.as_bytes())
                    .unwrap();
            }
            let last = EXERCISE_IDS.len() - 1;
            workspace
                .save_source("intro1", 1, FAILING_PROGRAM_SOURCE.as_bytes())
                .unwrap();
            workspace
                .save_source(EXERCISE_IDS[last], 1, SLOW_PASSING_SOURCE.as_bytes())
                .unwrap();
            workspace
                .save_progress(&progress_with_prefix(
                    &workspace,
                    EXERCISE_IDS.len(),
                    EXERCISE_IDS[last],
                ))
                .unwrap();
        } else {
            workspace
                .save_source("intro1", 0, SLOW_PASSING_SOURCE.as_bytes())
                .unwrap();
        }
        let state_path = workspace.state_path().to_owned();
        let state_bytes = replace_state_file_with_directory(&state_path);
        let toolchain = Toolchain::discover()
            .await
            .map_err(|error| error.to_string());
        let session = Arc::new(Session::new(
            curriculum,
            workspace,
            ProcessRunner::new(),
            toolchain,
        ));

        let ticket = session
            .start_run(if final_commit {
                EXERCISE_IDS[EXERCISE_IDS.len() - 1]
            } else {
                "intro1"
            })
            .unwrap();
        let result = session.await_run(&ticket.run_id).await.unwrap();
        assert!(matches!(
            result.validation.outcome,
            ValidationOutcome::OperationalFailure {
                kind: OperationalKind::Storage,
                ..
            }
        ));
        assert!(!result.validation.stages.is_empty());
        if final_commit {
            assert_eq!(result.final_recheck.len(), 1);
        }
        restore_state_file(&state_path, &state_bytes);
    }
}

#[tokio::test]
async fn post_validation_snapshot_failure_is_terminal_and_clears_the_active_run() {
    let app_data = TestDir::new("snapshot-failure");
    let session = open_session(&app_data.0).await;
    session
        .save_source("intro1", 0, SLOW_PASSING_SOURCE)
        .unwrap();
    let original_snapshot = session.snapshot().unwrap();
    let ticket = session.start_run("intro1").unwrap();
    let root = workspace_root(&app_data.0);
    fs::write(root.join("answers/intro1.rs"), [0xff]).unwrap();

    let error = tokio::time::timeout(Duration::from_secs(10), session.await_run(&ticket.run_id))
        .await
        .expect("await_run remained pending")
        .unwrap_err();
    assert!(error.to_string().contains("UTF-8"));
    assert_eq!(
        session
            .await_run(&ticket.run_id)
            .await
            .unwrap_err()
            .to_string(),
        error.to_string()
    );

    fs::write(
        root.join("answers/intro1.rs"),
        original_snapshot.source.as_bytes(),
    )
    .unwrap();
    assert!(session.snapshot().unwrap().active_run_id.is_none());
}

#[tokio::test]
async fn shutdown_cancels_and_awaits_the_active_session_run() {
    let app_data = TestDir::new("session-shutdown");
    let session = open_session(&app_data.0).await;
    let marker = app_data.0.join("shutdown-program-started");
    session
        .save_source(
            "intro1",
            0,
            &format!(
                "fn main() {{ std::fs::write({marker:?}, b\"started\").unwrap(); loop {{ std::hint::spin_loop(); }} }}\n"
            ),
        )
        .unwrap();
    let ticket = session.start_run("intro1").unwrap();
    wait_for_file(&marker).await;

    tokio::time::timeout(Duration::from_secs(10), session.shutdown())
        .await
        .expect("session shutdown did not finish");
    assert!(session.snapshot().unwrap().active_run_id.is_none());
    assert_eq!(
        session
            .await_run(&ticket.run_id)
            .await
            .unwrap()
            .validation
            .outcome,
        ValidationOutcome::Cancelled
    );
    assert_eq!(
        session.cancel_run(&ticket.run_id).await,
        CancelRunResult::NotActive
    );
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
