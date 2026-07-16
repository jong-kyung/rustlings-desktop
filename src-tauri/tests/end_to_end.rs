#![cfg(target_os = "macos")]

use app_lib::{
    curriculum::{Curriculum, EXERCISE_IDS},
    diagnostics::ValidationStage,
    process::ProcessRunner,
    session::{CancelRunResult, ExerciseStatus, Session},
    toolchain::Toolchain,
    validator::ValidationOutcome,
    workspace::{Workspace, WorkspaceError, WorkspaceOwner},
};
use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const PASSING_SOURCE: &str = "fn main() {}\n";
const FIXED_INTRO2: &str = "fn main() { println!(\"Hello world!\"); }\n";
const CANCELLABLE_SOURCE: &str = r#"fn main() {
    std::fs::write("started.pid", std::process::id().to_string()).unwrap();
    std::thread::sleep(std::time::Duration::from_secs(30));
}
"#;

struct TestDir(PathBuf);

impl TestDir {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("lustlings-u8-e2e-{}-{nonce}", std::process::id()));
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

fn open_session(app_data: &Path, toolchain: &Toolchain) -> (Arc<Session>, PathBuf) {
    let curriculum = curriculum();
    let workspace =
        Workspace::open(WorkspaceOwner::acquire(app_data).unwrap(), &curriculum).unwrap();
    let root = workspace.root().to_owned();
    (
        Arc::new(Session::new(
            curriculum,
            workspace,
            ProcessRunner::new(),
            Ok(toolchain.clone()),
        )),
        root,
    )
}

async fn wait_for_started_pid(generated: &Path) -> u32 {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        for entry in fs::read_dir(generated).unwrap().flatten() {
            let marker = entry.path().join("started.pid");
            if let Ok(value) = fs::read_to_string(marker) {
                return value.parse().unwrap();
            }
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "cancellable learner program did not start"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn process_exists(pid: u32) -> bool {
    if unsafe { libc::kill(pid as i32, 0) } == 0 {
        return true;
    }
    io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
}

#[tokio::test]
async fn macos_vertical_slice_recovers_across_runs_and_restarts() {
    let app_data = TestDir::new();
    let toolchain = Toolchain::discover().await.unwrap();
    let (mut session, root) = open_session(&app_data.0, &toolchain);

    let mut answers = fs::read_dir(root.join("answers"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect::<Vec<_>>();
    answers.sort();
    let mut expected = EXERCISE_IDS
        .iter()
        .map(|id| format!("{id}.rs"))
        .collect::<Vec<_>>();
    expected.sort();
    assert_eq!(answers, expected);
    assert_eq!(session.snapshot().unwrap().selected, "intro1");

    let source_before = fs::read(root.join("answers/intro1.rs")).unwrap();
    let progress_before = fs::read(root.join("state/progress.json")).unwrap();
    assert!(matches!(
        WorkspaceOwner::acquire(&app_data.0),
        Err(WorkspaceError::OwnershipUnavailable)
    ));
    assert_eq!(
        fs::read(root.join("answers/intro1.rs")).unwrap(),
        source_before
    );
    assert_eq!(
        fs::read(root.join("state/progress.json")).unwrap(),
        progress_before
    );

    let intro1 = session.start_run("intro1").unwrap();
    let intro1 = session.await_run(&intro1.run_id).await.unwrap();
    assert_eq!(intro1.validation.outcome, ValidationOutcome::Passed);
    assert_eq!(intro1.snapshot.selected, "intro2");

    let broken_intro2 = session.start_run("intro2").unwrap();
    let broken_intro2 = session.await_run(&broken_intro2.run_id).await.unwrap();
    assert_eq!(
        broken_intro2.validation.outcome,
        ValidationOutcome::LearnerFailure {
            stage: ValidationStage::Build
        }
    );
    assert_eq!(broken_intro2.snapshot.selected, "intro2");
    assert_eq!(
        broken_intro2.snapshot.exercises[2].status,
        ExerciseStatus::Locked
    );

    session.save_source("intro2", 0, FIXED_INTRO2).unwrap();
    let fixed_intro2 = session.start_run("intro2").unwrap();
    let fixed_intro2 = session.await_run(&fixed_intro2.run_id).await.unwrap();
    assert_eq!(fixed_intro2.validation.outcome, ValidationOutcome::Passed);
    assert_eq!(fixed_intro2.snapshot.selected, "variables1");
    assert_eq!(
        fixed_intro2.snapshot.exercises[2].status,
        ExerciseStatus::Current
    );
    session.shutdown().await;
    drop(session);

    (session, _) = open_session(&app_data.0, &toolchain);
    let resumed = session.snapshot().unwrap();
    assert_eq!(resumed.selected, "variables1");
    assert_eq!(
        resumed.source.as_bytes(),
        curriculum().source_bytes("variables1").unwrap()
    );
    assert_eq!(resumed.exercises[0].status, ExerciseStatus::Completed);
    assert_eq!(resumed.exercises[1].status, ExerciseStatus::Completed);
    assert_eq!(
        fs::read(root.join("answers/intro2.rs")).unwrap(),
        FIXED_INTRO2.as_bytes()
    );

    session
        .save_source("variables1", 0, CANCELLABLE_SOURCE)
        .unwrap();
    let cancellable = session.start_run("variables1").unwrap();
    let learner_pid = wait_for_started_pid(&root.join("generated")).await;
    assert_eq!(
        session.cancel_run(&cancellable.run_id).await,
        CancelRunResult::Requested
    );
    let cancelled = session.await_run(&cancellable.run_id).await.unwrap();
    assert_eq!(cancelled.validation.outcome, ValidationOutcome::Cancelled);
    assert!(!process_exists(learner_pid));
    assert!(fs::read_dir(root.join("generated"))
        .unwrap()
        .next()
        .is_none());

    session
        .save_source("variables1", 1, PASSING_SOURCE)
        .unwrap();
    let next = session.start_run("variables1").unwrap();
    let next = session.await_run(&next.run_id).await.unwrap();
    assert_eq!(next.validation.outcome, ValidationOutcome::Passed);
    assert_eq!(next.snapshot.selected, "variables2");
    session.shutdown().await;
    drop(session);

    let reconciled_intro2 = b"fn main() { println!(\"preserved after crash\"); }\n";
    fs::write(root.join("answers/intro2.rs"), reconciled_intro2).unwrap();
    (session, _) = open_session(&app_data.0, &toolchain);
    let reconciled = session.snapshot().unwrap();
    assert_eq!(reconciled.selected, "intro2");
    assert_eq!(reconciled.exercises[0].status, ExerciseStatus::Completed);
    assert_eq!(reconciled.exercises[2].status, ExerciseStatus::Locked);
    assert_eq!(
        fs::read(root.join("answers/intro2.rs")).unwrap(),
        reconciled_intro2
    );
    session.shutdown().await;
    drop(session);

    fs::write(root.join("state/progress.json"), b"{ corrupt progress").unwrap();
    (session, _) = open_session(&app_data.0, &toolchain);
    let recovered = session.snapshot().unwrap();
    assert_eq!(recovered.selected, "intro1");
    assert!(recovered
        .exercises
        .iter()
        .skip(1)
        .all(|exercise| exercise.status == ExerciseStatus::Locked));
    assert_eq!(
        fs::read(root.join("answers/intro2.rs")).unwrap(),
        reconciled_intro2
    );
    let backups = fs::read_dir(root.join("state/backups"))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(backups.len(), 1);
    assert_eq!(fs::read(backups[0].path()).unwrap(), b"{ corrupt progress");
    session.shutdown().await;
}
