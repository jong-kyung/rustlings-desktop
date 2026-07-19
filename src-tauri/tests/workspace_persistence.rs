use app_lib::{
    curriculum::{Curriculum, CurriculumIdentity, EXERCISE_IDS},
    workspace::{
        Completion, CurriculumProof, Progress, Workspace, WorkspaceError, WorkspaceOwner,
        MAX_STATE_BYTES,
    },
};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Barrier},
    thread,
    time::{SystemTime, UNIX_EPOCH},
};

struct TestDir(PathBuf);

impl TestDir {
    fn new(label: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "lustlings-u2-{label}-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
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

fn open(app_data: &Path) -> Workspace {
    let owner = WorkspaceOwner::acquire(app_data).unwrap();
    Workspace::open(owner, &curriculum()).unwrap()
}

fn write_progress(path: &Path, value: serde_json::Value) {
    fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
}

fn complete_prefix(workspace: &Workspace, count: usize) -> Vec<Completion> {
    EXERCISE_IDS[..count]
        .iter()
        .map(|id| Completion {
            id: (*id).to_owned(),
            digest: workspace.source_digest(id).unwrap(),
        })
        .collect()
}

const LEGACY_IDS: [&str; 8] = [
    "intro1",
    "intro2",
    "variables1",
    "variables2",
    "variables3",
    "variables4",
    "variables5",
    "variables6",
];
const LEGACY_CARGO_TOML: &[u8] = include_bytes!("fixtures/workspace-v1/Cargo.toml");
const LEGACY_CARGO_LOCK: &[u8] = include_bytes!("fixtures/workspace-v1/Cargo.lock");
const LEGACY_COMPLETE_PROGRESS: &[u8] =
    include_bytes!("fixtures/workspace-v1/progress-complete.json");

fn legacy_progress(selected: &str, completed: usize) -> Vec<u8> {
    let curriculum = curriculum();
    let completed = LEGACY_IDS[..completed]
        .iter()
        .map(|id| {
            serde_json::json!({
                "id": id,
                "digest": format!("{:x}", Sha256::digest(curriculum.source_bytes(id).unwrap())),
            })
        })
        .collect::<Vec<_>>();
    serde_json::to_vec_pretty(&serde_json::json!({
        "schema_version": 1,
        "curriculum": curriculum.identity(),
        "selected": selected,
        "completed": completed,
        "slice_complete": null,
    }))
    .unwrap()
}

fn create_legacy_workspace(app_data: &Path, progress: &[u8]) -> PathBuf {
    let root = app_data.join(format!(
        "workspace-v1-rustlings-6.5.0-{}",
        curriculum().identity().upstream_commit
    ));
    let answers = root.join("answers");
    fs::create_dir_all(&answers).unwrap();
    fs::create_dir(root.join("state")).unwrap();
    fs::create_dir(root.join("generated")).unwrap();
    fs::write(root.join("Cargo.toml"), LEGACY_CARGO_TOML).unwrap();
    fs::write(root.join("Cargo.lock"), LEGACY_CARGO_LOCK).unwrap();
    let curriculum = curriculum();
    for id in LEGACY_IDS {
        fs::write(
            answers.join(format!("{id}.rs")),
            curriculum.source_bytes(id).unwrap(),
        )
        .unwrap();
    }
    fs::write(root.join("state/progress.json"), progress).unwrap();
    root
}

#[test]
fn legacy_partial_progress_and_all_existing_answer_bytes_survive_migration() {
    let app_data = TestDir::new("legacy-partial");
    let root = create_legacy_workspace(app_data.path(), &legacy_progress("variables2", 3));
    let edited = b"fn main() { println!(\"learner edit\"); }\n";
    fs::write(root.join("answers/variables2.rs"), edited).unwrap();
    let before = LEGACY_IDS
        .iter()
        .map(|id| {
            (
                *id,
                fs::read(root.join(format!("answers/{id}.rs"))).unwrap(),
            )
        })
        .collect::<Vec<_>>();

    let workspace = open(app_data.path());
    assert_eq!(workspace.progress().schema_version, 2);
    assert_eq!(workspace.progress().selected, "variables2");
    assert_eq!(workspace.progress().completed.len(), 3);
    assert!(workspace.progress().curriculum_complete.is_none());
    let persisted: serde_json::Value =
        serde_json::from_slice(&fs::read(workspace.state_path()).unwrap()).unwrap();
    assert_eq!(persisted["schema_version"], 2);
    assert!(persisted.get("curriculum_complete").is_some());
    assert!(persisted.get("slice_complete").is_none());
    assert_eq!(fs::read_dir(workspace.answers_dir()).unwrap().count(), 94);
    assert!(!workspace.answers_dir().join("intro1_sol.rs").exists());
    for (id, bytes) in before {
        assert_eq!(
            fs::read(root.join(format!("answers/{id}.rs"))).unwrap(),
            bytes
        );
    }
    assert_eq!(workspace.source("variables2").unwrap(), edited);

    drop(workspace);
    let reopened = open(app_data.path());
    assert_eq!(reopened.progress().selected, "variables2");
    assert_eq!(reopened.progress().completed.len(), 3);
    assert_eq!(reopened.source("variables2").unwrap(), edited);
}

#[test]
fn legacy_complete_slice_becomes_incomplete_curriculum_without_losing_prefix() {
    let app_data = TestDir::new("legacy-complete");
    create_legacy_workspace(app_data.path(), LEGACY_COMPLETE_PROGRESS);

    let workspace = open(app_data.path());
    assert_eq!(workspace.progress().schema_version, 2);
    assert_eq!(workspace.progress().selected, "variables6");
    assert_eq!(workspace.progress().completed.len(), 8);
    assert!(workspace.progress().curriculum_complete.is_none());
    assert!(workspace.source(EXERCISE_IDS[8]).is_ok());
}

#[test]
fn legacy_digest_mismatch_truncates_and_missing_answer_is_recreated() {
    let app_data = TestDir::new("legacy-reconcile");
    let root = create_legacy_workspace(app_data.path(), &legacy_progress("variables2", 3));
    fs::write(root.join("answers/intro2.rs"), b"edited after completion").unwrap();
    fs::remove_file(root.join("answers/variables6.rs")).unwrap();

    let workspace = open(app_data.path());
    assert_eq!(workspace.progress().completed.len(), 1);
    assert_eq!(workspace.progress().selected, "intro2");
    assert_eq!(
        workspace.source("variables6").unwrap(),
        curriculum().source_bytes("variables6").unwrap()
    );
}

#[test]
fn every_known_legacy_and_current_infrastructure_pair_converges_idempotently() {
    let current_manifest = curriculum().cargo_manifest_bytes().unwrap();
    let current_lock = curriculum().cargo_lockfile_bytes().unwrap();
    for (manifest_current, lock_current) in
        [(false, false), (false, true), (true, false), (true, true)]
    {
        let app_data = TestDir::new(&format!("pair-{manifest_current}-{lock_current}"));
        let root = create_legacy_workspace(app_data.path(), &legacy_progress("intro1", 0));
        if manifest_current {
            fs::write(root.join("Cargo.toml"), &current_manifest).unwrap();
        }
        if lock_current {
            fs::write(root.join("Cargo.lock"), &current_lock).unwrap();
        }

        let workspace = open(app_data.path());
        assert_eq!(fs::read(root.join("Cargo.toml")).unwrap(), current_manifest);
        assert_eq!(fs::read(root.join("Cargo.lock")).unwrap(), current_lock);
        drop(workspace);
        let reopened = open(app_data.path());
        assert_eq!(fs::read(root.join("Cargo.toml")).unwrap(), current_manifest);
        assert_eq!(fs::read(root.join("Cargo.lock")).unwrap(), current_lock);
        drop(reopened);
    }
}

#[test]
fn unknown_legacy_infrastructure_rejects_before_answer_or_state_mutation() {
    let app_data = TestDir::new("legacy-tampered");
    let root = create_legacy_workspace(app_data.path(), &legacy_progress("variables2", 3));
    fs::write(root.join("Cargo.toml"), b"tampered infrastructure").unwrap();
    let answer = root.join("answers/variables2.rs");
    let state = root.join("state/progress.json");
    let answer_before = fs::read(&answer).unwrap();
    let state_before = fs::read(&state).unwrap();

    assert!(matches!(
        Workspace::open(
            WorkspaceOwner::acquire(app_data.path()).unwrap(),
            &curriculum()
        ),
        Err(WorkspaceError::InfrastructureMismatch(_))
    ));
    assert_eq!(fs::read(answer).unwrap(), answer_before);
    assert_eq!(fs::read(state).unwrap(), state_before);
    assert_eq!(fs::read_dir(root.join("answers")).unwrap().count(), 8);
}

#[test]
#[cfg(unix)]
fn legacy_unsafe_state_path_rejects_before_migration_writes() {
    let app_data = TestDir::new("legacy-unsafe-state");
    let root = create_legacy_workspace(app_data.path(), &legacy_progress("intro1", 0));
    let state = root.join("state/progress.json");
    let outside = app_data.path().join("outside-progress");
    fs::write(&outside, b"outside").unwrap();
    fs::remove_file(&state).unwrap();
    std::os::unix::fs::symlink(&outside, &state).unwrap();

    assert!(matches!(
        Workspace::open(
            WorkspaceOwner::acquire(app_data.path()).unwrap(),
            &curriculum()
        ),
        Err(WorkspaceError::UnsafePath(_))
    ));
    assert_eq!(
        fs::read(root.join("Cargo.toml")).unwrap(),
        LEGACY_CARGO_TOML
    );
    assert_eq!(
        fs::read(root.join("Cargo.lock")).unwrap(),
        LEGACY_CARGO_LOCK
    );
    assert_eq!(fs::read_dir(root.join("answers")).unwrap().count(), 8);
    assert_eq!(fs::read(outside).unwrap(), b"outside");

    let app_data = TestDir::new("legacy-unsafe-backups");
    let root = create_legacy_workspace(app_data.path(), b"{ malformed");
    let outside = app_data.path().join("outside-backups");
    fs::create_dir(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, root.join("state/backups")).unwrap();
    assert!(matches!(
        Workspace::open(
            WorkspaceOwner::acquire(app_data.path()).unwrap(),
            &curriculum()
        ),
        Err(WorkspaceError::UnsafePath(_))
    ));
    assert_eq!(
        fs::read(root.join("Cargo.toml")).unwrap(),
        LEGACY_CARGO_TOML
    );
    assert_eq!(fs::read_dir(root.join("answers")).unwrap().count(), 8);
    assert!(fs::read_dir(outside).unwrap().next().is_none());
}

#[test]
fn first_init_materializes_all_answers_and_second_init_preserves_edits() {
    let app_data = TestDir::new("init");
    let workspace = open(app_data.path());
    assert_eq!(workspace.progress().selected, "intro1");
    assert!(workspace.generated_dir().is_dir());
    assert_eq!(
        fs::read(workspace.root().join("Cargo.toml")).unwrap(),
        fs::read(curriculum().root().join("Cargo.toml")).unwrap()
    );
    assert_eq!(
        fs::read(workspace.root().join("Cargo.lock")).unwrap(),
        fs::read(curriculum().root().join("Cargo.lock")).unwrap()
    );
    let mut answers: Vec<_> = fs::read_dir(workspace.answers_dir())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    answers.sort();
    let mut expected: Vec<_> = EXERCISE_IDS.iter().map(|id| format!("{id}.rs")).collect();
    expected.sort();
    assert_eq!(answers, expected);
    for id in EXERCISE_IDS {
        assert_eq!(
            workspace.source(id).unwrap(),
            curriculum().source_bytes(id).unwrap()
        );
    }

    let saved = workspace
        .save_source("intro1", 0, b"fn main() { println!(\"mine\"); }\n")
        .unwrap();
    assert_eq!(saved.revision, 1);
    drop(workspace);

    let reopened = open(app_data.path());
    assert_eq!(
        reopened.source("intro1").unwrap(),
        b"fn main() { println!(\"mine\"); }\n"
    );
    assert_eq!(reopened.source_digest("intro1").unwrap(), saved.digest);
}

#[test]
fn stale_save_and_delayed_response_cannot_replace_newer_bytes() {
    let app_data = TestDir::new("revision");
    let workspace = open(app_data.path());
    let first = workspace.save_source("intro1", 0, b"newest").unwrap();
    assert_eq!(first.revision, 1);
    assert!(matches!(
        workspace.save_source("intro1", 0, b"stale"),
        Err(WorkspaceError::RevisionConflict {
            expected: 0,
            actual: 1
        })
    ));
    assert_eq!(workspace.source("intro1").unwrap(), b"newest");
}

#[test]
fn failed_progress_reconciliation_does_not_advance_source_revision() {
    let app_data = TestDir::new("save-retry");
    let workspace = open(app_data.path());
    workspace
        .save_progress(&Progress {
            schema_version: 2,
            curriculum: curriculum().identity().clone(),
            selected: "intro2".into(),
            completed: complete_prefix(&workspace, 1),
            curriculum_complete: None,
        })
        .unwrap();

    let state_path = workspace.state_path().to_owned();
    let state_bytes = fs::read(&state_path).unwrap();
    fs::remove_file(&state_path).unwrap();
    fs::create_dir(&state_path).unwrap();

    assert!(workspace
        .save_source("intro1", 0, b"durable despite state failure")
        .is_err());
    assert_eq!(
        workspace.source("intro1").unwrap(),
        b"durable despite state failure"
    );
    assert_eq!(workspace.revision("intro1").unwrap(), 0);
    assert_eq!(workspace.progress().completed.len(), 1);

    fs::remove_dir(&state_path).unwrap();
    fs::write(&state_path, state_bytes).unwrap();
    let saved = workspace
        .save_source("intro1", 0, b"durable despite state failure")
        .unwrap();
    assert_eq!(saved.revision, 1);
    assert!(workspace.progress().completed.is_empty());
    assert_eq!(workspace.progress().selected, "intro1");
}

#[test]
fn interrupted_files_are_ignored_and_source_progress_mismatch_is_reconciled() {
    let app_data = TestDir::new("interrupted");
    fs::create_dir(app_data.path().join(".workspace-v1-interrupted.tmp")).unwrap();
    let workspace = open(app_data.path());
    let completed = complete_prefix(&workspace, 2);
    let progress = Progress {
        schema_version: 2,
        curriculum: curriculum().identity().clone(),
        selected: "variables1".into(),
        completed,
        curriculum_complete: None,
    };
    workspace.save_progress(&progress).unwrap();
    let state_before = fs::read(workspace.state_path()).unwrap();
    fs::write(
        workspace.answers_dir().join("intro1.rs"),
        b"crash after answer rename",
    )
    .unwrap();
    fs::write(
        workspace.answers_dir().join(".intro1.rs.tmp-interrupted"),
        b"ignored temp",
    )
    .unwrap();
    fs::write(
        workspace
            .state_path()
            .with_file_name(".progress.json.tmp-interrupted"),
        b"ignored temp",
    )
    .unwrap();
    assert_ne!(fs::read(workspace.state_path()).unwrap(), Vec::<u8>::new());
    assert_eq!(fs::read(workspace.state_path()).unwrap(), state_before);
    drop(workspace);

    let reopened = open(app_data.path());
    assert_eq!(
        reopened.source("intro1").unwrap(),
        b"crash after answer rename"
    );
    assert!(reopened.progress().completed.is_empty());
    assert_eq!(reopened.progress().selected, "intro1");
}

#[test]
fn malformed_and_incompatible_states_are_backed_up_then_recovered() {
    let cases = [
        ("truncated", serde_json::json!({"schema_version": 1})),
        (
            "unknown-version",
            serde_json::json!({
                "schema_version": 99,
                "curriculum": curriculum().identity(),
                "selected": "intro1",
                "completed": [],
                "slice_complete": null
            }),
        ),
        (
            "wrong-curriculum",
            serde_json::json!({
                "schema_version": 1,
                "curriculum": CurriculumIdentity {
                    rustlings_version: "6.4.0".into(),
                    upstream_commit: "wrong".into(),
                },
                "selected": "intro1",
                "completed": [],
                "slice_complete": null
            }),
        ),
        (
            "completion-hole",
            serde_json::json!({
                "schema_version": 1,
                "curriculum": curriculum().identity(),
                "selected": "variables2",
                "completed": [{"id": "intro2", "digest": "0".repeat(64)}],
                "slice_complete": null
            }),
        ),
        (
            "locked-selection",
            serde_json::json!({
                "schema_version": 1,
                "curriculum": curriculum().identity(),
                "selected": "variables6",
                "completed": [],
                "slice_complete": null
            }),
        ),
        (
            "invalid-digest-syntax",
            serde_json::json!({
                "schema_version": 1,
                "curriculum": curriculum().identity(),
                "selected": "intro2",
                "completed": [{"id": "intro1", "digest": "not-a-digest"}],
                "slice_complete": null
            }),
        ),
        (
            "invalid-slice-proof",
            serde_json::json!({
                "schema_version": 1,
                "curriculum": curriculum().identity(),
                "selected": "intro1",
                "completed": [],
                "slice_complete": {"sources": []}
            }),
        ),
    ];

    for (label, value) in cases {
        let app_data = TestDir::new(label);
        let workspace = open(app_data.path());
        let state_path = workspace.state_path().to_owned();
        drop(workspace);
        write_progress(&state_path, value);
        let damaged = fs::read(&state_path).unwrap();

        let recovered = open(app_data.path());
        assert_eq!(recovered.progress().selected, "intro1", "{label}");
        assert!(recovered.progress().completed.is_empty(), "{label}");
        let backups: Vec<_> = fs::read_dir(state_path.parent().unwrap().join("backups"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        assert_eq!(backups.len(), 1, "{label}");
        assert_eq!(fs::read(&backups[0]).unwrap(), damaged, "{label}");
    }

    let app_data = TestDir::new("oversized");
    let workspace = open(app_data.path());
    let state_path = workspace.state_path().to_owned();
    drop(workspace);
    fs::write(&state_path, vec![b'x'; MAX_STATE_BYTES + 1]).unwrap();
    let recovered = open(app_data.path());
    assert!(recovered.progress().completed.is_empty());
}

#[test]
fn progress_requires_current_contiguous_digests_and_complete_curriculum_proof() {
    let app_data = TestDir::new("progress");
    let workspace = open(app_data.path());
    let first_two = complete_prefix(&workspace, 2);
    let valid = Progress {
        schema_version: 2,
        curriculum: curriculum().identity().clone(),
        selected: "variables1".into(),
        completed: first_two.clone(),
        curriculum_complete: None,
    };
    workspace.save_progress(&valid).unwrap();

    let mut hole = valid.clone();
    hole.completed[1].id = "variables1".into();
    assert!(matches!(
        workspace.save_progress(&hole),
        Err(WorkspaceError::InvalidProgress(_))
    ));
    let mut wrong_digest = valid.clone();
    wrong_digest.completed[0].digest = "0".repeat(64);
    assert!(matches!(
        workspace.save_progress(&wrong_digest),
        Err(WorkspaceError::InvalidProgress(_))
    ));
    let mut invalid_final = valid;
    invalid_final.curriculum_complete = Some(CurriculumProof { sources: first_two });
    assert!(matches!(
        workspace.save_progress(&invalid_final),
        Err(WorkspaceError::InvalidProgress(_))
    ));
}

#[test]
fn recovery_failure_preserves_canonical_state_and_allows_retry() {
    let app_data = TestDir::new("retry");
    let workspace = open(app_data.path());
    let state_path = workspace.state_path().to_owned();
    let backups = state_path.parent().unwrap().join("backups");
    drop(workspace);
    fs::write(&state_path, b"broken canonical bytes").unwrap();
    let outside = app_data.path().join("outside");
    fs::create_dir(&outside).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside, &backups).unwrap();

    assert!(matches!(
        Workspace::open(
            WorkspaceOwner::acquire(app_data.path()).unwrap(),
            &curriculum()
        ),
        Err(WorkspaceError::RecoveryRequired(_)) | Err(WorkspaceError::UnsafePath(_))
    ));
    assert_eq!(fs::read(&state_path).unwrap(), b"broken canonical bytes");
    assert!(fs::read_dir(&outside).unwrap().next().is_none());

    #[cfg(unix)]
    fs::remove_file(&backups).unwrap();
    let recovered = open(app_data.path());
    assert!(recovered.progress().completed.is_empty());
}

#[test]
#[cfg(unix)]
fn symlink_fifo_and_nonregular_paths_are_rejected_without_touching_outside_files() {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};

    let app_data = TestDir::new("paths");
    let workspace = open(app_data.path());
    let answer = workspace.answers_dir().join("intro1.rs");
    drop(workspace);
    let sentinel = app_data.path().join("sentinel");
    fs::write(&sentinel, b"outside").unwrap();
    fs::remove_file(&answer).unwrap();
    std::os::unix::fs::symlink(&sentinel, &answer).unwrap();
    assert!(matches!(
        Workspace::open(
            WorkspaceOwner::acquire(app_data.path()).unwrap(),
            &curriculum()
        ),
        Err(WorkspaceError::UnsafePath(_))
    ));
    assert_eq!(fs::read(&sentinel).unwrap(), b"outside");

    fs::remove_file(&answer).unwrap();
    let answer = CString::new(answer.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(answer.as_ptr(), 0o600) }, 0);
    assert!(matches!(
        Workspace::open(
            WorkspaceOwner::acquire(app_data.path()).unwrap(),
            &curriculum()
        ),
        Err(WorkspaceError::UnsafePath(_))
    ));

    let device = Path::new("/dev/null");
    assert!(matches!(
        WorkspaceOwner::acquire(device),
        Err(WorkspaceError::UnsafePath(_))
    ));
}

#[test]
#[cfg(unix)]
fn source_state_and_generated_directory_symlinks_are_rejected() {
    for name in ["answers", "state", "generated"] {
        let app_data = TestDir::new(name);
        let workspace = open(app_data.path());
        let target = workspace.root().join(name);
        drop(workspace);
        fs::remove_dir_all(&target).unwrap();
        let outside = app_data.path().join(format!("outside-{name}"));
        fs::create_dir(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, &target).unwrap();
        assert!(matches!(
            Workspace::open(
                WorkspaceOwner::acquire(app_data.path()).unwrap(),
                &curriculum()
            ),
            Err(WorkspaceError::UnsafePath(_))
        ));
        assert!(fs::read_dir(outside).unwrap().next().is_none());
    }
}

#[test]
fn generated_cache_is_disposable_without_affecting_durable_data() {
    let app_data = TestDir::new("generated");
    let workspace = open(app_data.path());
    workspace
        .save_source("intro1", 0, b"durable answer")
        .unwrap();
    let progress = Progress {
        schema_version: 2,
        curriculum: curriculum().identity().clone(),
        selected: "intro2".into(),
        completed: complete_prefix(&workspace, 1),
        curriculum_complete: None,
    };
    workspace.save_progress(&progress).unwrap();
    let generated = workspace.generated_dir().to_owned();
    fs::write(generated.join("cache"), b"throwaway").unwrap();
    drop(workspace);
    fs::remove_dir_all(generated).unwrap();

    let reopened = open(app_data.path());
    assert_eq!(reopened.source("intro1").unwrap(), b"durable answer");
    assert_eq!(reopened.progress().completed.len(), 1);
    assert!(reopened.generated_dir().is_dir());
}

#[test]
fn only_one_cold_launch_owner_can_initialize() {
    let app_data = Arc::new(TestDir::new("owner"));
    let barrier = Arc::new(Barrier::new(2));
    let handles: Vec<_> = (0..2)
        .map(|_| {
            let app_data = Arc::clone(&app_data);
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                match WorkspaceOwner::acquire(app_data.path()) {
                    Ok(owner) => {
                        let workspace = Workspace::open(owner, &curriculum()).unwrap();
                        thread::sleep(std::time::Duration::from_millis(100));
                        Some(workspace.root().to_owned())
                    }
                    Err(WorkspaceError::OwnershipUnavailable) => None,
                    Err(error) => panic!("unexpected ownership error: {error}"),
                }
            })
        })
        .collect();
    let owners = handles
        .into_iter()
        .filter_map(|handle| handle.join().unwrap())
        .count();
    assert_eq!(owners, 1);
}
