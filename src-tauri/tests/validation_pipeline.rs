#![cfg(unix)]

use app_lib::{
    curriculum::Curriculum,
    diagnostics::ValidationStage,
    process::ProcessRunner,
    toolchain::Toolchain,
    validator::{OperationalKind, ValidationOutcome, Validator},
    workspace::{Workspace, WorkspaceOwner},
};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
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
            "lustlings-u4-{label}-{}-{nonce}",
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

fn curriculum_with_intro1_flags(root: &Path, test: bool, strict_clippy: bool) -> Curriculum {
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/rustlings-6.5.0");
    copy_tree(&source, root);
    let manifest_path = root.join("manifest.json");
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
    manifest["exercises"][0]["test"] = test.into();
    manifest["exercises"][0]["strict_clippy"] = strict_clippy.into();
    fs::write(manifest_path, serde_json::to_vec_pretty(&manifest).unwrap()).unwrap();
    Curriculum::load_test_fixture(root).unwrap()
}

fn copy_tree(source: &Path, destination: &Path) {
    fs::create_dir(destination).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let destination = destination.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &destination);
        } else {
            fs::copy(entry.path(), destination).unwrap();
        }
    }
}

#[test]
fn pinned_upstream_validation_policy_is_recorded() {
    let baseline = include_str!("fixtures/cargo/rustlings-6.5.0-exercise-policy.txt");
    assert!(baseline.contains("2af9e89ba536fad01aa828b06e0ac2174bad0f6d"));
    assert!(baseline.contains("build -> optional test -> clippy -> binary"));
    assert!(baseline.contains("test failure -> binary output -> failure"));
    assert!(baseline.contains("strict clippy -> --profile test -- -D warnings"));
}

#[test]
fn inherited_cargo_rust_and_loader_injection_is_cleared() {
    let toolchain_root = std::env::current_exe()
        .ok()
        .and_then(|_| std::env::var_os("PATH"))
        .and_then(|_| {
            let runtime = tokio::runtime::Runtime::new().ok()?;
            runtime
                .block_on(Toolchain::discover())
                .ok()
                .map(|toolchain| toolchain.root().to_owned())
        })
        .unwrap();
    let shadow = TestDir::new("path-shadow");
    fs::write(shadow.0.join("cargo"), b"#!/bin/sh\nexit 99\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(shadow.0.join("cargo"), fs::Permissions::from_mode(0o700)).unwrap();
    }
    let hostile_path =
        std::env::join_paths([shadow.0.as_path(), toolchain_root.as_path()]).unwrap();
    let status = Command::new(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "validator_hostile_environment_fixture",
            "--nocapture",
        ])
        .env("PATH", hostile_path)
        .env("U4_HOSTILE_ENV_FIXTURE", "1")
        .status()
        .unwrap();
    assert!(status.success());
}

#[tokio::test]
#[ignore]
async fn validator_hostile_environment_fixture() {
    if std::env::var_os("U4_HOSTILE_ENV_FIXTURE").is_none() {
        return;
    }
    for (key, value) in [
        ("RUSTC_WRAPPER", "/forged/rustc-wrapper"),
        ("RUSTC_WORKSPACE_WRAPPER", "/forged/workspace-wrapper"),
        ("RUSTFLAGS", "--cfg injected_rustflags"),
        (
            "CARGO_ENCODED_RUSTFLAGS",
            "--cfg\u{1f}injected_encoded_flags",
        ),
        ("CARGO_TARGET_AARCH64_APPLE_DARWIN_RUNNER", "/forged/runner"),
        ("CARGO_REGISTRIES_CRATES_IO_CREDENTIAL_PROVIDER", "forged"),
        ("DYLD_INSERT_LIBRARIES", "/forged/library.dylib"),
        ("DYLD_LIBRARY_PATH", "/forged"),
    ] {
        std::env::set_var(key, value);
    }
    let app_data = TestDir::new("hostile-env");
    let curriculum = curriculum();
    let workspace = workspace(&app_data.0, &curriculum);
    let toolchain = Toolchain::discover().await.unwrap();
    let runner = ProcessRunner::new();
    let validator = Validator::new(&curriculum, &toolchain, &runner, &workspace);
    let source = br#"
#[cfg(injected_rustflags)]
compile_error!("inherited RUSTFLAGS reached rustc");
fn main() {
    for key in [
        "RUSTC_WRAPPER",
        "RUSTC_WORKSPACE_WRAPPER",
        "RUSTFLAGS",
        "CARGO_ENCODED_RUSTFLAGS",
        "CARGO_TARGET_AARCH64_APPLE_DARWIN_RUNNER",
        "CARGO_REGISTRIES_CRATES_IO_CREDENTIAL_PROVIDER",
        "DYLD_INSERT_LIBRARIES",
        "DYLD_LIBRARY_PATH",
    ] {
        assert!(std::env::var_os(key).is_none(), "inherited {key}");
    }
    assert!(!std::env::var("PATH").unwrap().contains("path-shadow"));
}
"#;
    let result = validator.validate("intro1", source).await.unwrap();
    assert_eq!(result.outcome, ValidationOutcome::Passed, "{result:#?}");
}

#[tokio::test]
async fn build_failure_short_circuits_and_diagnostics_are_digest_bound() {
    let app_data = TestDir::new("build-failure");
    let curriculum = curriculum();
    let workspace = workspace(&app_data.0, &curriculum);
    let toolchain = Toolchain::discover().await.unwrap();
    let runner = ProcessRunner::new();
    let validator = Validator::new(&curriculum, &toolchain, &runner, &workspace);
    let source = "fn main() {\n    let 😀 = missing;\n}\n";

    let result = validator
        .validate("intro1", source.as_bytes())
        .await
        .unwrap();

    assert!(matches!(
        result.outcome,
        ValidationOutcome::LearnerFailure {
            stage: ValidationStage::Build
        }
    ));
    assert_eq!(
        result
            .stages
            .iter()
            .map(|stage| stage.stage)
            .collect::<Vec<_>>(),
        [ValidationStage::Build]
    );
    assert!(!result.diagnostics.is_empty(), "{result:#?}");
    assert!(result
        .diagnostics
        .iter()
        .all(|diagnostic| diagnostic.source_digest == result.source_digest));
    assert!(result
        .diagnostics
        .iter()
        .filter_map(|diagnostic| diagnostic.range.as_ref())
        .all(|range| range.start_line_number >= 1 && range.start_column >= 1));
}

#[tokio::test]
async fn passing_validation_uses_snapshot_bytes_and_fixed_stage_order() {
    let app_data = TestDir::new("snapshot");
    let curriculum = curriculum();
    let workspace = workspace(&app_data.0, &curriculum);
    let toolchain = Toolchain::discover().await.unwrap();
    let runner = ProcessRunner::new();
    let validator = Validator::new(&curriculum, &toolchain, &runner, &workspace);
    let captured_source = b"fn main() { println!(\"captured snapshot\"); }\n".to_vec();
    let captured = BTreeMap::from([("intro1".to_owned(), captured_source)]);
    let snapshot = validator.snapshot(&captured).unwrap();

    workspace
        .save_source("intro1", 0, b"fn main() { this durable edit is invalid }\n")
        .unwrap();
    let result = validator.validate_snapshot("intro1", &snapshot).await;

    assert_eq!(result.outcome, ValidationOutcome::Passed);
    assert_eq!(
        result
            .stages
            .iter()
            .map(|stage| stage.stage)
            .collect::<Vec<_>>(),
        [
            ValidationStage::Build,
            ValidationStage::Clippy,
            ValidationStage::Program,
        ]
    );
    assert!(result.stages[2].stdout.contains("captured snapshot"));
}

#[tokio::test]
async fn relative_learner_writes_cannot_reach_the_durable_workspace() {
    let app_data = TestDir::new("snapshot-isolation");
    let curriculum = curriculum();
    let workspace = workspace(&app_data.0, &curriculum);
    let original = workspace.source("intro1").unwrap();
    let toolchain = Toolchain::discover().await.unwrap();
    let runner = ProcessRunner::new();
    let validator = Validator::new(&curriculum, &toolchain, &runner, &workspace);

    let result = validator
        .validate(
            "intro1",
            b"fn main() { let _ = std::fs::write(\"../../answers/intro1.rs\", b\"corrupt\"); }\n",
        )
        .await
        .unwrap();

    assert_eq!(result.outcome, ValidationOutcome::Passed);
    assert_eq!(workspace.source("intro1").unwrap(), original);
}

#[tokio::test]
async fn optional_test_failure_runs_binary_and_strict_clippy_warnings_fail() {
    let test_root = TestDir::new("test-metadata");
    let test_resources = test_root.0.join("resources");
    let test_curriculum = curriculum_with_intro1_flags(&test_resources, true, false);
    let test_app_data = test_root.0.join("app-data");
    fs::create_dir(&test_app_data).unwrap();
    let test_workspace = workspace(&test_app_data, &test_curriculum);
    let toolchain = Toolchain::discover().await.unwrap();
    let runner = ProcessRunner::new();
    let validator = Validator::new(&test_curriculum, &toolchain, &runner, &test_workspace);
    let failing_test = br#"
fn main() { println!("binary output after failed test"); }
#[test]
fn official_test() { panic!("learner test failure"); }
"#;
    let result = validator.validate("intro1", failing_test).await.unwrap();
    assert!(matches!(
        result.outcome,
        ValidationOutcome::LearnerFailure {
            stage: ValidationStage::Test
        }
    ));
    assert_eq!(
        result
            .stages
            .iter()
            .map(|stage| stage.stage)
            .collect::<Vec<_>>(),
        [
            ValidationStage::Build,
            ValidationStage::Test,
            ValidationStage::Program,
        ]
    );
    assert!(result.stages[2]
        .stdout
        .contains("binary output after failed test"));

    let strict_root = TestDir::new("strict-clippy-metadata");
    let strict_resources = strict_root.0.join("resources");
    let strict_curriculum = curriculum_with_intro1_flags(&strict_resources, false, true);
    let strict_app_data = strict_root.0.join("app-data");
    fs::create_dir(&strict_app_data).unwrap();
    let strict_workspace = workspace(&strict_app_data, &strict_curriculum);
    let strict_validator =
        Validator::new(&strict_curriculum, &toolchain, &runner, &strict_workspace);
    let clippy_warning = b"fn main() { let value = true; if value == true {} }\n";
    let result = strict_validator
        .validate("intro1", clippy_warning)
        .await
        .unwrap();
    assert!(matches!(
        result.outcome,
        ValidationOutcome::LearnerFailure {
            stage: ValidationStage::Clippy
        }
    ));
    assert_eq!(
        result.stages.last().unwrap().stage,
        ValidationStage::Program
    );
}

#[tokio::test]
async fn modified_manifest_fails_before_process_start() {
    let app_data = TestDir::new("manifest");
    let curriculum = curriculum();
    let workspace = workspace(&app_data.0, &curriculum);
    let toolchain = Toolchain::discover().await.unwrap();
    let runner = ProcessRunner::new();
    let validator = Validator::new(&curriculum, &toolchain, &runner, &workspace);
    fs::write(
        workspace.root().join("Cargo.toml"),
        b"[package]\nname='forged'\n",
    )
    .unwrap();

    let error = validator
        .validate("intro1", b"fn main() {}\n")
        .await
        .unwrap_err();

    assert!(error.to_string().contains("infrastructure changed"));
    assert!(runner
        .start(app_lib::process::ProcessSpec::new(
            toolchain.cargo(),
            workspace.root()
        ))
        .await
        .is_ok());
    runner.shutdown().await;
}

#[tokio::test]
async fn ambient_workspace_and_home_cargo_config_is_not_loaded() {
    let app_data = TestDir::new("cargo-config");
    let curriculum = curriculum();
    let workspace = workspace(&app_data.0, &curriculum);
    let toolchain = Toolchain::discover().await.unwrap();
    let runner = ProcessRunner::new();
    let validator = Validator::new(&curriculum, &toolchain, &runner, &workspace);
    fs::create_dir(app_data.0.join(".cargo")).unwrap();
    fs::write(
        app_data.0.join(".cargo/config.toml"),
        b"[build]\nrustc-wrapper = '/forged/wrapper'\n",
    )
    .unwrap();

    let result = validator
        .validate("intro1", b"fn main() {}\n")
        .await
        .unwrap();
    assert_eq!(result.outcome, ValidationOutcome::Passed);
}

#[test]
fn operational_outcomes_are_not_learner_failures() {
    let outcomes = [
        ValidationOutcome::Cancelled,
        ValidationOutcome::TimedOut,
        ValidationOutcome::OutputLimit,
        ValidationOutcome::OperationalFailure {
            kind: OperationalKind::Spawn,
            message: "spawn".into(),
        },
    ];
    assert!(outcomes
        .iter()
        .all(|outcome| !matches!(outcome, ValidationOutcome::LearnerFailure { .. })));
}
