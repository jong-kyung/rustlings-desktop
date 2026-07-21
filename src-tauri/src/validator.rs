use crate::{
    curriculum::{digest, Curriculum, EXERCISE_IDS},
    diagnostics::{
        floor_char_boundary, parse_cargo_output, NormalizedDiagnostic, ParsedCargoOutput,
        ValidationStage, MAX_DIAGNOSTICS,
    },
    process::{
        CancellationToken, ProcessOutcome, ProcessResult, ProcessRunner, ProcessSpec, StartError,
    },
    toolchain::Toolchain,
    workspace::{Workspace, MAX_SOURCE_BYTES},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    env,
    ffi::OsString,
    fmt, fs, io,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

const MAX_VALIDATION_OUTPUT_BYTES: usize = 2 * 1024 * 1024;
static NEXT_SNAPSHOT: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationalKind {
    Infrastructure,
    Storage,
    Spawn,
    Busy,
    Process,
    CargoStartup,
    Artifact,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ValidationOutcome {
    Passed,
    LearnerFailure {
        stage: ValidationStage,
    },
    Cancelled,
    TimedOut,
    OutputLimit,
    OperationalFailure {
        kind: OperationalKind,
        message: String,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct StageResult {
    pub stage: ValidationStage,
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
    pub output_truncated: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ValidationResult {
    pub exercise_id: String,
    pub source_digest: String,
    pub outcome: ValidationOutcome,
    pub stages: Vec<StageResult>,
    pub diagnostics: Vec<NormalizedDiagnostic>,
}

#[derive(Debug)]
pub struct ValidationError(String);

impl fmt::Display for ValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for ValidationError {}

struct SnapshotSource {
    relative: String,
    absolute: PathBuf,
    bytes: Vec<u8>,
    digest: String,
}

pub struct ValidationSnapshot {
    root: PathBuf,
    target: PathBuf,
    cargo_home: PathBuf,
    home: PathBuf,
    manifest: PathBuf,
    sources: BTreeMap<String, SnapshotSource>,
}

impl Drop for ValidationSnapshot {
    fn drop(&mut self) {
        let _ = make_tree_writable(&self.root);
        let _ = fs::remove_dir_all(&self.root);
    }
}

pub struct Validator<'a> {
    curriculum: &'a Curriculum,
    toolchain: &'a Toolchain,
    runner: &'a ProcessRunner,
    workspace_root: PathBuf,
    cancellation: CancellationToken,
}

impl<'a> Validator<'a> {
    pub fn new(
        curriculum: &'a Curriculum,
        toolchain: &'a Toolchain,
        runner: &'a ProcessRunner,
        workspace: &'a Workspace,
    ) -> Self {
        Self {
            curriculum,
            toolchain,
            runner,
            workspace_root: workspace.root().to_owned(),
            cancellation: CancellationToken::new(),
        }
    }

    pub fn with_cancellation(mut self, cancellation: CancellationToken) -> Self {
        self.cancellation = cancellation;
        self
    }

    pub fn snapshot(
        &self,
        captured_sources: &BTreeMap<String, Vec<u8>>,
    ) -> Result<ValidationSnapshot, ValidationError> {
        self.verify_workspace_infrastructure()?;
        if captured_sources
            .keys()
            .any(|id| !EXERCISE_IDS.contains(&id.as_str()))
        {
            return Err(ValidationError(
                "snapshot contains an unknown exercise".into(),
            ));
        }
        if captured_sources
            .values()
            .any(|source| source.len() > MAX_SOURCE_BYTES)
        {
            return Err(ValidationError(
                "snapshot source exceeds the 1 MiB limit".into(),
            ));
        }

        let root = env::temp_dir().join(format!(
            "lustlings-validation-{}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos(),
            NEXT_SNAPSHOT.fetch_add(1, Ordering::Relaxed)
        ));
        create_private_dir(&root)?;
        let result = (|| {
            let cargo_home = root.join("cargo-home");
            let target = root.join("target");
            let home = root.join("home");
            create_private_dir(&cargo_home)?;
            create_private_dir(&target)?;
            create_private_dir(&home)?;
            let manifest = root.join("Cargo.toml");
            write_private_file(
                &manifest,
                &self.curriculum.cargo_manifest_bytes().map_err(display)?,
            )?;
            write_private_file(
                &root.join("Cargo.lock"),
                &self.curriculum.cargo_lockfile_bytes().map_err(display)?,
            )?;

            let mut sources = BTreeMap::new();
            for exercise in self.curriculum.exercises() {
                let bytes = captured_sources
                    .get(&exercise.id)
                    .cloned()
                    .map(Ok)
                    .unwrap_or_else(|| {
                        self.curriculum.source_bytes(&exercise.id).map_err(display)
                    })?;
                let absolute = root.join(&exercise.source);
                create_private_dir_all(absolute.parent().expect("exercise source has a parent"))?;
                write_private_file(&absolute, &bytes)?;
                sources.insert(
                    exercise.id.clone(),
                    SnapshotSource {
                        relative: exercise.source.clone(),
                        absolute,
                        digest: digest(&bytes),
                        bytes,
                    },
                );
            }
            Ok(ValidationSnapshot {
                root: root.clone(),
                target,
                cargo_home,
                home,
                manifest,
                sources,
            })
        })();
        if result.is_err() {
            let _ = make_tree_writable(&root);
            let _ = fs::remove_dir_all(&root);
        }
        result
    }

    pub async fn validate(
        &self,
        exercise_id: &str,
        source: &[u8],
    ) -> Result<ValidationResult, ValidationError> {
        let captured = BTreeMap::from([(exercise_id.to_owned(), source.to_vec())]);
        let snapshot = self.snapshot(&captured)?;
        Ok(self.validate_snapshot(exercise_id, &snapshot).await)
    }

    pub async fn validate_snapshot(
        &self,
        exercise_id: &str,
        snapshot: &ValidationSnapshot,
    ) -> ValidationResult {
        let Some(exercise) = self.curriculum.exercise(exercise_id) else {
            return operational_result(
                exercise_id,
                "",
                OperationalKind::Infrastructure,
                "unknown exercise",
            );
        };
        let Some(source) = snapshot.sources.get(exercise_id) else {
            return operational_result(
                exercise_id,
                "",
                OperationalKind::Infrastructure,
                "exercise is absent from validation snapshot",
            );
        };
        let mut result = ValidationResult {
            exercise_id: exercise_id.to_owned(),
            source_digest: source.digest.clone(),
            outcome: ValidationOutcome::Passed,
            stages: Vec::new(),
            diagnostics: Vec::new(),
        };
        let mut output_budget = MAX_VALIDATION_OUTPUT_BYTES;

        let build = match self
            .run_cargo(
                snapshot,
                source,
                exercise_id,
                ValidationStage::Build,
                false,
                &mut output_budget,
            )
            .await
        {
            Ok(stage) => stage,
            Err(outcome) => {
                result.outcome = outcome;
                return result;
            }
        };
        append_cargo_stage(&mut result, &build);
        if !build.success {
            result.outcome =
                cargo_failure_outcome(ValidationStage::Build, &build.parsed, &build.stderr);
            return result;
        }
        let executable = match controlled_executable(&build.parsed.executables, &snapshot.target) {
            Ok(executable) => executable,
            Err(message) => {
                result.outcome = ValidationOutcome::OperationalFailure {
                    kind: OperationalKind::Artifact,
                    message,
                };
                return result;
            }
        };

        if exercise.test {
            let test = match self
                .run_cargo(
                    snapshot,
                    source,
                    exercise_id,
                    ValidationStage::Test,
                    false,
                    &mut output_budget,
                )
                .await
            {
                Ok(stage) => stage,
                Err(outcome) => {
                    result.outcome = outcome;
                    return result;
                }
            };
            append_cargo_stage(&mut result, &test);
            if !test.success {
                match self
                    .run_program(snapshot, &executable, &mut output_budget)
                    .await
                {
                    Ok(program) => result.stages.push(program),
                    Err(outcome) => {
                        result.outcome = outcome;
                        return result;
                    }
                }
                result.outcome =
                    cargo_failure_outcome(ValidationStage::Test, &test.parsed, &test.stderr);
                return result;
            }
        }

        let clippy = match self
            .run_cargo(
                snapshot,
                source,
                exercise_id,
                ValidationStage::Clippy,
                exercise.strict_clippy,
                &mut output_budget,
            )
            .await
        {
            Ok(stage) => stage,
            Err(outcome) => {
                result.outcome = outcome;
                return result;
            }
        };
        append_cargo_stage(&mut result, &clippy);
        let program = match self
            .run_program(snapshot, &executable, &mut output_budget)
            .await
        {
            Ok(program) => program,
            Err(outcome) => {
                result.outcome = outcome;
                return result;
            }
        };
        let program_success = program.success;
        result.stages.push(program);
        result.outcome = if !clippy.success {
            cargo_failure_outcome(ValidationStage::Clippy, &clippy.parsed, &clippy.stderr)
        } else if !program_success {
            ValidationOutcome::LearnerFailure {
                stage: ValidationStage::Program,
            }
        } else {
            ValidationOutcome::Passed
        };
        result
    }

    async fn run_cargo(
        &self,
        snapshot: &ValidationSnapshot,
        source: &SnapshotSource,
        exercise_id: &str,
        stage: ValidationStage,
        strict_clippy: bool,
        output_budget: &mut usize,
    ) -> Result<CargoStage, ValidationOutcome> {
        let arguments = cargo_arguments(
            stage,
            exercise_id,
            &snapshot.manifest,
            &snapshot.target,
            strict_clippy,
        );
        let spec = self
            .base_spec(self.toolchain.cargo(), Path::new("/"), snapshot)
            .args(arguments);
        let process = self.run(spec).await?;
        let success = process_success(&process);
        let parsed = parse_cargo_output(
            &process.stdout,
            stage,
            &source.relative,
            &source.absolute,
            &source.bytes,
            &source.digest,
            exercise_id,
        );
        if parsed.build_script_executed {
            return Err(ValidationOutcome::OperationalFailure {
                kind: OperationalKind::Infrastructure,
                message: "Cargo executed an unexpected build script".into(),
            });
        }
        let stdout = take_output(&parsed.text, output_budget);
        let stderr = take_output(&String::from_utf8_lossy(&process.stderr), output_budget);
        Ok(CargoStage {
            stage,
            success,
            stdout,
            stderr,
            output_truncated: process.output_truncated || *output_budget == 0,
            parsed,
        })
    }

    async fn run_program(
        &self,
        snapshot: &ValidationSnapshot,
        executable: &Path,
        output_budget: &mut usize,
    ) -> Result<StageResult, ValidationOutcome> {
        let process = self
            .run(self.base_spec(executable, &snapshot.root, snapshot))
            .await?;
        let success = process_success(&process);
        let mut stdout = String::from_utf8_lossy(&process.stdout).into_owned();
        if !success {
            stdout.push_str("The exercise didn't run successfully (nonzero exit code)\n");
        }
        Ok(StageResult {
            stage: ValidationStage::Program,
            success,
            stdout: take_output(&stdout, output_budget),
            stderr: take_output(&String::from_utf8_lossy(&process.stderr), output_budget),
            output_truncated: process.output_truncated || *output_budget == 0,
        })
    }

    async fn run(&self, spec: ProcessSpec) -> Result<ProcessResult, ValidationOutcome> {
        let started = self
            .runner
            .start_cancellable(spec, self.cancellation.clone())
            .await
            .map_err(start_error)?;
        let result =
            started
                .wait()
                .await
                .map_err(|error| ValidationOutcome::OperationalFailure {
                    kind: OperationalKind::Process,
                    message: error.to_string(),
                })?;
        match result.outcome {
            ProcessOutcome::Cancelled => Err(ValidationOutcome::Cancelled),
            ProcessOutcome::TimedOut => Err(ValidationOutcome::TimedOut),
            ProcessOutcome::OutputLimit => Err(ValidationOutcome::OutputLimit),
            ProcessOutcome::Exited { .. } => Ok(result),
        }
    }

    fn base_spec(
        &self,
        executable: &Path,
        cwd: &Path,
        snapshot: &ValidationSnapshot,
    ) -> ProcessSpec {
        let path = env::join_paths([
            self.toolchain.root(),
            Path::new("/usr/bin"),
            Path::new("/bin"),
        ])
        .expect("fixed toolchain PATH is valid");
        ProcessSpec::new(executable, cwd)
            .env("PATH", path)
            .env("HOME", &snapshot.home)
            .env("CARGO_HOME", &snapshot.cargo_home)
            .env("CARGO_TARGET_DIR", &snapshot.target)
            .env("CARGO_NET_OFFLINE", "true")
            .env("CARGO_TERM_COLOR", "never")
            .env("RUSTC", self.toolchain.rustc())
            .env("TERM", "dumb")
    }

    fn verify_workspace_infrastructure(&self) -> Result<(), ValidationError> {
        require_directory(&self.workspace_root)?;
        verify_file(
            &self.workspace_root.join("Cargo.toml"),
            &self.curriculum.cargo_manifest_bytes().map_err(display)?,
        )?;
        verify_file(
            &self.workspace_root.join("Cargo.lock"),
            &self.curriculum.cargo_lockfile_bytes().map_err(display)?,
        )?;
        Ok(())
    }
}

struct CargoStage {
    stage: ValidationStage,
    success: bool,
    stdout: String,
    stderr: String,
    output_truncated: bool,
    parsed: ParsedCargoOutput,
}

fn append_cargo_stage(result: &mut ValidationResult, stage: &CargoStage) {
    let remaining = MAX_DIAGNOSTICS.saturating_sub(result.diagnostics.len());
    result
        .diagnostics
        .extend(stage.parsed.diagnostics.iter().take(remaining).cloned());
    result.stages.push(StageResult {
        stage: stage.stage,
        success: stage.success,
        stdout: stage.stdout.clone(),
        stderr: stage.stderr.clone(),
        output_truncated: stage.output_truncated,
    });
}

fn cargo_arguments(
    stage: ValidationStage,
    exercise_id: &str,
    manifest: &Path,
    target: &Path,
    strict_clippy: bool,
) -> Vec<OsString> {
    let command = match stage {
        ValidationStage::Build => "build",
        ValidationStage::Test => "test",
        ValidationStage::Clippy => "clippy",
        ValidationStage::Program => unreachable!("program execution does not use Cargo"),
    };
    let mut arguments = [command, "--offline", "--locked", "--manifest-path"]
        .into_iter()
        .map(OsString::from)
        .collect::<Vec<_>>();
    arguments.push(manifest.as_os_str().to_owned());
    arguments.push("--target-dir".into());
    arguments.push(target.as_os_str().to_owned());
    arguments.extend(
        ["--bin", exercise_id, "--message-format=json"]
            .into_iter()
            .map(OsString::from),
    );
    if command == "test" {
        arguments.extend(
            ["--", "--color", "never", "--format", "pretty"]
                .into_iter()
                .map(OsString::from),
        );
    } else if command == "clippy" {
        arguments.extend(["--profile", "test"].into_iter().map(OsString::from));
        if strict_clippy {
            arguments.extend(["--", "-D", "warnings"].into_iter().map(OsString::from));
        }
    }
    arguments
}

fn controlled_executable(candidates: &[PathBuf], target: &Path) -> Result<PathBuf, String> {
    if candidates.is_empty() {
        return Err("Cargo did not report the expected compiler artifact executable".into());
    }
    let canonical_target = fs::canonicalize(target)
        .map_err(|error| format!("invalid controlled target directory: {error}"))?;
    let mut executable = None;
    for candidate in candidates {
        let metadata = fs::symlink_metadata(candidate).map_err(|error| {
            format!("invalid compiler artifact {}: {error}", candidate.display())
        })?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(format!(
                "compiler artifact is not a regular file: {}",
                candidate.display()
            ));
        }
        let canonical = fs::canonicalize(candidate).map_err(|error| {
            format!("invalid compiler artifact {}: {error}", candidate.display())
        })?;
        if !canonical.starts_with(&canonical_target) {
            return Err(format!(
                "compiler artifact is outside the controlled target: {}",
                candidate.display()
            ));
        }
        executable = Some(canonical);
    }
    executable.ok_or_else(|| "Cargo did not report an executable".into())
}

fn cargo_failure_outcome(
    stage: ValidationStage,
    parsed: &ParsedCargoOutput,
    stderr: &str,
) -> ValidationOutcome {
    if stage != ValidationStage::Test
        && !parsed.saw_compiler_message
        && parsed.build_finished.is_none()
    {
        ValidationOutcome::OperationalFailure {
            kind: OperationalKind::CargoStartup,
            message: if stderr.is_empty() {
                "Cargo failed before reporting compiler output".into()
            } else {
                stderr.to_owned()
            },
        }
    } else {
        ValidationOutcome::LearnerFailure { stage }
    }
}

fn process_success(result: &ProcessResult) -> bool {
    matches!(
        result.outcome,
        ProcessOutcome::Exited {
            code: Some(0),
            signal: None
        }
    )
}

fn start_error(error: StartError) -> ValidationOutcome {
    if matches!(error, StartError::Cancelled) {
        return ValidationOutcome::Cancelled;
    }
    let kind = match error {
        StartError::Busy { .. } => OperationalKind::Busy,
        StartError::Spawn(_) => OperationalKind::Spawn,
        StartError::InvalidSpec(_) | StartError::Unsupported => OperationalKind::Infrastructure,
        StartError::Cancelled => unreachable!(),
    };
    ValidationOutcome::OperationalFailure {
        kind,
        message: error.to_string(),
    }
}

fn take_output(value: &str, budget: &mut usize) -> String {
    let length = floor_char_boundary(value, *budget);
    *budget -= length;
    value[..length].to_owned()
}

fn operational_result(
    exercise_id: &str,
    source_digest: &str,
    kind: OperationalKind,
    message: &str,
) -> ValidationResult {
    ValidationResult {
        exercise_id: exercise_id.to_owned(),
        source_digest: source_digest.to_owned(),
        outcome: ValidationOutcome::OperationalFailure {
            kind,
            message: message.to_owned(),
        },
        stages: Vec::new(),
        diagnostics: Vec::new(),
    }
}

fn require_directory(path: &Path) -> Result<(), ValidationError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| io_error(path, error))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(ValidationError(format!(
            "symlink or non-directory validation path: {}",
            path.display()
        )));
    }
    Ok(())
}

fn verify_file(path: &Path, expected: &[u8]) -> Result<(), ValidationError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| io_error(path, error))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(ValidationError(format!(
            "symlink or non-regular validation file: {}",
            path.display()
        )));
    }
    if fs::read(path).map_err(|error| io_error(path, error))? != expected {
        return Err(ValidationError(format!(
            "validation infrastructure changed: {}",
            path.display()
        )));
    }
    Ok(())
}

fn create_private_dir(path: &Path) -> Result<(), ValidationError> {
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path).map_err(|error| io_error(path, error))
}

fn create_private_dir_all(path: &Path) -> Result<(), ValidationError> {
    if path.is_dir() {
        return require_directory(path);
    }
    let parent = path
        .parent()
        .ok_or_else(|| ValidationError("validation directory has no parent".into()))?;
    create_private_dir_all(parent)?;
    create_private_dir(path)
}

fn write_private_file(path: &Path, bytes: &[u8]) -> Result<(), ValidationError> {
    use std::io::Write;
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o400);
    }
    let mut file = options.open(path).map_err(|error| io_error(path, error))?;
    // Snapshot files are scratch state deleted after the run; durability via fsync
    // is not needed and costs ~one fsync per curriculum file on every Run click.
    file.write_all(bytes).map_err(|error| io_error(path, error))
}

fn make_tree_writable(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(metadata) = fs::symlink_metadata(path) {
            if metadata.is_dir() && !metadata.file_type().is_symlink() {
                fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
                for entry in fs::read_dir(path)? {
                    make_tree_writable(&entry?.path())?;
                }
            } else if metadata.is_file() {
                fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
            }
        }
    }
    Ok(())
}

fn display(error: impl fmt::Display) -> ValidationError {
    ValidationError(error.to_string())
}

fn io_error(path: &Path, error: impl fmt::Display) -> ValidationError {
    ValidationError(format!("{}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upstream_650_policy_orders_and_short_circuits_as_characterized() {
        assert_eq!(
            cargo_arguments(
                ValidationStage::Clippy,
                "intro1",
                Path::new("/snapshot/Cargo.toml"),
                Path::new("/target"),
                true,
            )
            .iter()
            .map(|value| value.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .as_slice(),
            [
                "clippy",
                "--offline",
                "--locked",
                "--manifest-path",
                "/snapshot/Cargo.toml",
                "--target-dir",
                "/target",
                "--bin",
                "intro1",
                "--message-format=json",
                "--profile",
                "test",
                "--",
                "-D",
                "warnings",
            ]
        );
        // Pinned src/exercise.rs: build; optional test; Clippy; binary. A failed
        // test runs the binary for learner output and remains a test failure.
    }

    #[test]
    fn forged_and_non_regular_artifacts_are_rejected() {
        let root = env::temp_dir().join(format!(
            "lustlings-artifact-test-{}-{}",
            std::process::id(),
            NEXT_SNAPSHOT.fetch_add(1, Ordering::Relaxed)
        ));
        let target = root.join("target");
        let outside = root.join("forged");
        fs::create_dir_all(&target).unwrap();
        fs::write(&outside, b"forged").unwrap();
        assert!(
            controlled_executable(std::slice::from_ref(&outside), &target)
                .unwrap_err()
                .contains("outside the controlled target")
        );
        assert!(
            controlled_executable(std::slice::from_ref(&target), &target)
                .unwrap_err()
                .contains("not a regular file")
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn outcome_mapping_keeps_process_limits_timeout_cancel_and_startup_distinct() {
        assert_ne!(ValidationOutcome::Cancelled, ValidationOutcome::TimedOut);
        assert_ne!(ValidationOutcome::TimedOut, ValidationOutcome::OutputLimit);
        let parsed = ParsedCargoOutput::default();
        assert!(matches!(
            cargo_failure_outcome(ValidationStage::Build, &parsed, "cargo startup failed"),
            ValidationOutcome::OperationalFailure {
                kind: OperationalKind::CargoStartup,
                ..
            }
        ));
    }
}
