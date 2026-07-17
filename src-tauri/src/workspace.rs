use crate::curriculum::{digest, Curriculum, CurriculumIdentity, EXERCISE_IDS};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fmt, fs,
    fs::{File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex,
    },
    time::{SystemTime, UNIX_EPOCH},
};

pub const MAX_SOURCE_BYTES: usize = 1024 * 1024;
pub const MAX_STATE_BYTES: usize = 1024 * 1024;
const STATE_SCHEMA_VERSION: u8 = 2;
const LEGACY_SCHEMA_VERSION: u8 = 1;
const LEGACY_EXERCISE_IDS: [&str; 8] = [
    "intro1",
    "intro2",
    "variables1",
    "variables2",
    "variables3",
    "variables4",
    "variables5",
    "variables6",
];
const LEGACY_CARGO_MANIFEST: &[u8] = include_bytes!("../tests/fixtures/workspace-v1/Cargo.toml");
const LEGACY_CARGO_LOCKFILE: &[u8] = include_bytes!("../tests/fixtures/workspace-v1/Cargo.lock");
static UNIQUE_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Completion {
    pub id: String,
    pub digest: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CurriculumProof {
    pub sources: Vec<Completion>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Progress {
    pub schema_version: u8,
    pub curriculum: CurriculumIdentity,
    pub selected: String,
    pub completed: Vec<Completion>,
    pub curriculum_complete: Option<CurriculumProof>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyProgress {
    schema_version: u8,
    curriculum: CurriculumIdentity,
    selected: String,
    completed: Vec<Completion>,
    slice_complete: Option<LegacySliceProof>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacySliceProof {
    sources: Vec<Completion>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SaveResult {
    pub revision: u64,
    pub digest: String,
}

#[derive(Debug)]
pub enum WorkspaceError {
    Io(String),
    Curriculum(String),
    UnsafePath(String),
    InfrastructureMismatch(String),
    OwnershipUnavailable,
    UnknownExercise(String),
    SourceTooLarge,
    RevisionConflict { expected: u64, actual: u64 },
    InvalidProgress(String),
    RecoveryRequired(String),
}

impl fmt::Display for WorkspaceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(message)
            | Self::Curriculum(message)
            | Self::UnsafePath(message)
            | Self::InfrastructureMismatch(message)
            | Self::UnknownExercise(message)
            | Self::InvalidProgress(message)
            | Self::RecoveryRequired(message) => formatter.write_str(message),
            Self::OwnershipUnavailable => {
                formatter.write_str("workspace is owned by another process")
            }
            Self::SourceTooLarge => formatter.write_str("source exceeds the 1 MiB limit"),
            Self::RevisionConflict { expected, actual } => {
                write!(
                    formatter,
                    "source revision conflict: expected {expected}, actual {actual}"
                )
            }
        }
    }
}

impl std::error::Error for WorkspaceError {}

pub struct WorkspaceOwner {
    app_data: PathBuf,
    _lock: File,
}

impl WorkspaceOwner {
    pub fn acquire(app_data: impl Into<PathBuf>) -> Result<Self, WorkspaceError> {
        let app_data = app_data.into();
        require_directory(&app_data)?;
        let lock_path = app_data.join(".workspace-owner.lock");
        let lock = open_lock_file(&lock_path)?;
        match try_lock_exclusive(&lock) {
            Ok(true) => {
                set_private_file(&lock_path)?;
                set_private_directory(&app_data)?;
                Ok(Self {
                    app_data,
                    _lock: lock,
                })
            }
            Ok(false) => Err(WorkspaceError::OwnershipUnavailable),
            Err(error) => Err(io_error(&lock_path, error)),
        }
    }
}

pub struct Workspace {
    _owner: WorkspaceOwner,
    root: PathBuf,
    answers: PathBuf,
    state_path: PathBuf,
    generated: PathBuf,
    identity: CurriculumIdentity,
    revisions: Mutex<HashMap<String, u64>>,
    progress: Mutex<Progress>,
}

impl Workspace {
    pub fn open(owner: WorkspaceOwner, curriculum: &Curriculum) -> Result<Self, WorkspaceError> {
        let root = owner.app_data.join(workspace_name(curriculum.identity()));
        match fs::symlink_metadata(&root) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() || !metadata.is_dir() {
                    return Err(unsafe_path(&root));
                }
                verify_existing_workspace(&root, curriculum)?;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                initialize_workspace(&owner.app_data, &root, curriculum)?;
            }
            Err(error) => return Err(io_error(&root, error)),
        }

        let answers = root.join("answers");
        let state_dir = root.join("state");
        let generated = root.join("generated");
        let state_path = state_dir.join("progress.json");
        let default = default_progress(curriculum.identity());
        let (mut progress, migrated) = match fs::symlink_metadata(&state_path) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() || !metadata.is_file() {
                    return Err(unsafe_path(&state_path));
                }
                set_private_file(&state_path)?;
                match load_progress(&state_path, curriculum.identity()) {
                    Ok(progress) => progress,
                    Err(_) => (recover_progress(&state_path, &default)?, false),
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                atomic_write(&state_path, &serialize_progress(&default)?)?;
                (default, false)
            }
            Err(error) => return Err(io_error(&state_path, error)),
        };

        let reconciled = reconcile_progress(&answers, &mut progress)?;
        if migrated || reconciled {
            atomic_write(&state_path, &serialize_progress(&progress)?)?;
        }

        Ok(Self {
            _owner: owner,
            root,
            answers,
            state_path,
            generated,
            identity: curriculum.identity().clone(),
            revisions: Mutex::new(
                EXERCISE_IDS
                    .iter()
                    .map(|id| ((*id).to_owned(), 0))
                    .collect(),
            ),
            progress: Mutex::new(progress),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn answers_dir(&self) -> &Path {
        &self.answers
    }

    pub fn state_path(&self) -> &Path {
        &self.state_path
    }

    pub fn generated_dir(&self) -> &Path {
        &self.generated
    }

    pub fn progress(&self) -> Progress {
        self.progress
            .lock()
            .expect("progress mutex poisoned")
            .clone()
    }

    pub fn source(&self, id: &str) -> Result<Vec<u8>, WorkspaceError> {
        let path = self.answer_path(id)?;
        read_regular(&path, Some(MAX_SOURCE_BYTES))
    }

    pub fn source_digest(&self, id: &str) -> Result<String, WorkspaceError> {
        Ok(digest(&self.source(id)?))
    }

    pub fn revision(&self, id: &str) -> Result<u64, WorkspaceError> {
        self.revisions
            .lock()
            .expect("revision mutex poisoned")
            .get(id)
            .copied()
            .ok_or_else(|| WorkspaceError::UnknownExercise(format!("unknown exercise: {id}")))
    }

    pub fn save_source(
        &self,
        id: &str,
        expected_revision: u64,
        bytes: &[u8],
    ) -> Result<SaveResult, WorkspaceError> {
        if bytes.len() > MAX_SOURCE_BYTES {
            return Err(WorkspaceError::SourceTooLarge);
        }
        let path = self.answer_path(id)?;
        let mut revisions = self.revisions.lock().expect("revision mutex poisoned");
        let revision = revisions
            .get_mut(id)
            .ok_or_else(|| WorkspaceError::UnknownExercise(format!("unknown exercise: {id}")))?;
        if *revision != expected_revision {
            return Err(WorkspaceError::RevisionConflict {
                expected: expected_revision,
                actual: *revision,
            });
        }
        require_regular_file(&path)?;
        atomic_write(&path, bytes)?;

        let mut progress = self.progress.lock().expect("progress mutex poisoned");
        let mut reconciled = progress.clone();
        if reconcile_progress(&self.answers, &mut reconciled)? {
            atomic_write(&self.state_path, &serialize_progress(&reconciled)?)?;
            *progress = reconciled;
        }
        *revision += 1;
        Ok(SaveResult {
            revision: *revision,
            digest: digest(bytes),
        })
    }

    pub fn save_progress(&self, progress: &Progress) -> Result<(), WorkspaceError> {
        validate_progress(progress, &self.identity)?;
        validate_current_proofs(&self.answers, progress)?;
        atomic_write(&self.state_path, &serialize_progress(progress)?)?;
        *self.progress.lock().expect("progress mutex poisoned") = progress.clone();
        Ok(())
    }

    fn answer_path(&self, id: &str) -> Result<PathBuf, WorkspaceError> {
        if !EXERCISE_IDS.contains(&id) {
            return Err(WorkspaceError::UnknownExercise(format!(
                "unknown exercise: {id}"
            )));
        }
        Ok(self.answers.join(format!("{id}.rs")))
    }
}

fn workspace_name(identity: &CurriculumIdentity) -> String {
    format!(
        "workspace-v1-rustlings-{}-{}",
        identity.rustlings_version, identity.upstream_commit
    )
}

fn initialize_workspace(
    app_data: &Path,
    destination: &Path,
    curriculum: &Curriculum,
) -> Result<(), WorkspaceError> {
    let temporary = app_data.join(format!(
        ".{}.tmp-{}",
        workspace_name(curriculum.identity()),
        unique()
    ));
    create_private_directory(&temporary)?;
    let result = (|| {
        let answers = temporary.join("answers");
        let state = temporary.join("state");
        let generated = temporary.join("generated");
        create_private_directory(&answers)?;
        create_private_directory(&state)?;
        create_private_directory(&generated)?;
        create_private_file(
            &temporary.join("Cargo.toml"),
            &curriculum
                .cargo_manifest_bytes()
                .map_err(curriculum_error)?,
        )?;
        create_private_file(
            &temporary.join("Cargo.lock"),
            &curriculum
                .cargo_lockfile_bytes()
                .map_err(curriculum_error)?,
        )?;
        for id in EXERCISE_IDS {
            create_private_file(
                &answers.join(format!("{id}.rs")),
                &curriculum.source_bytes(id).map_err(curriculum_error)?,
            )?;
        }
        create_private_file(
            &state.join("progress.json"),
            &serialize_progress(&default_progress(curriculum.identity()))?,
        )?;
        sync_directory(&answers)?;
        sync_directory(&state)?;
        sync_directory(&generated)?;
        sync_directory(&temporary)?;
        fs::rename(&temporary, destination).map_err(|error| io_error(destination, error))?;
        sync_directory(app_data)
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&temporary);
    }
    result
}

fn verify_existing_workspace(root: &Path, curriculum: &Curriculum) -> Result<(), WorkspaceError> {
    let answers = root.join("answers");
    let state = root.join("state");
    let generated = root.join("generated");
    require_directory(root)?;
    require_directory(&answers)?;
    require_directory(&state)?;
    inspect_optional_directory(&generated)?;

    let cargo_manifest = curriculum
        .cargo_manifest_bytes()
        .map_err(curriculum_error)?;
    let cargo_lockfile = curriculum
        .cargo_lockfile_bytes()
        .map_err(curriculum_error)?;
    let manifest_state = inspect_infrastructure(
        &root.join("Cargo.toml"),
        &cargo_manifest,
        LEGACY_CARGO_MANIFEST,
    )?;
    let lockfile_state = inspect_infrastructure(
        &root.join("Cargo.lock"),
        &cargo_lockfile,
        LEGACY_CARGO_LOCKFILE,
    )?;
    for id in EXERCISE_IDS {
        inspect_optional_regular_file(&answers.join(format!("{id}.rs")))?;
    }
    inspect_optional_regular_file(&state.join("progress.json"))?;
    inspect_optional_directory(&state.join("backups"))?;

    set_private_directory(root)?;
    set_private_directory(&answers)?;
    set_private_directory(&state)?;
    ensure_directory(&generated)?;
    converge_infrastructure(&root.join("Cargo.toml"), &cargo_manifest, manifest_state)?;
    converge_infrastructure(&root.join("Cargo.lock"), &cargo_lockfile, lockfile_state)?;
    for id in EXERCISE_IDS {
        let path = answers.join(format!("{id}.rs"));
        match fs::symlink_metadata(&path) {
            Ok(_) => {
                require_regular_file(&path)?;
                set_private_file(&path)?;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                atomic_write(
                    &path,
                    &curriculum.source_bytes(id).map_err(curriculum_error)?,
                )?;
            }
            Err(error) => return Err(io_error(&path, error)),
        }
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum InfrastructureState {
    Current,
    Legacy,
    Missing,
}

fn inspect_infrastructure(
    path: &Path,
    current: &[u8],
    legacy: &[u8],
) -> Result<InfrastructureState, WorkspaceError> {
    match fs::symlink_metadata(path) {
        Ok(_) => {
            require_regular_file(path)?;
            let bytes = read_regular(path, None)?;
            if bytes == current {
                Ok(InfrastructureState::Current)
            } else if bytes == legacy {
                Ok(InfrastructureState::Legacy)
            } else {
                Err(WorkspaceError::InfrastructureMismatch(format!(
                    "workspace Cargo infrastructure changed: {}",
                    path.display()
                )))
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(InfrastructureState::Missing),
        Err(error) => Err(io_error(path, error)),
    }
}

fn converge_infrastructure(
    path: &Path,
    current: &[u8],
    state: InfrastructureState,
) -> Result<(), WorkspaceError> {
    match state {
        InfrastructureState::Current => set_private_file(path),
        InfrastructureState::Legacy | InfrastructureState::Missing => atomic_write(path, current),
    }
}

fn inspect_optional_regular_file(path: &Path) -> Result<(), WorkspaceError> {
    match fs::symlink_metadata(path) {
        Ok(_) => require_regular_file(path),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(io_error(path, error)),
    }
}

fn inspect_optional_directory(path: &Path) -> Result<(), WorkspaceError> {
    match fs::symlink_metadata(path) {
        Ok(_) => require_directory(path),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(io_error(path, error)),
    }
}

fn load_progress(
    path: &Path,
    identity: &CurriculumIdentity,
) -> Result<(Progress, bool), WorkspaceError> {
    let bytes = read_regular(path, Some(MAX_STATE_BYTES))?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|error| {
        WorkspaceError::InvalidProgress(format!("invalid progress JSON: {error}"))
    })?;
    match value
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
    {
        Some(version) if version == u64::from(STATE_SCHEMA_VERSION) => {
            let progress: Progress = serde_json::from_value(value).map_err(|error| {
                WorkspaceError::InvalidProgress(format!("invalid progress JSON: {error}"))
            })?;
            validate_progress(&progress, identity)?;
            Ok((progress, false))
        }
        Some(version) if version == u64::from(LEGACY_SCHEMA_VERSION) => {
            let legacy: LegacyProgress = serde_json::from_value(value).map_err(|error| {
                WorkspaceError::InvalidProgress(format!("invalid legacy progress JSON: {error}"))
            })?;
            Ok((migrate_legacy_progress(legacy, identity)?, true))
        }
        _ => Err(WorkspaceError::InvalidProgress(
            "incompatible progress identity".into(),
        )),
    }
}

fn migrate_legacy_progress(
    legacy: LegacyProgress,
    identity: &CurriculumIdentity,
) -> Result<Progress, WorkspaceError> {
    if legacy.schema_version != LEGACY_SCHEMA_VERSION || &legacy.curriculum != identity {
        return Err(WorkspaceError::InvalidProgress(
            "incompatible legacy progress identity".into(),
        ));
    }
    if legacy.completed.len() > LEGACY_EXERCISE_IDS.len() {
        return Err(WorkspaceError::InvalidProgress(
            "too many legacy completed exercises".into(),
        ));
    }
    for (proof, expected_id) in legacy.completed.iter().zip(LEGACY_EXERCISE_IDS) {
        if proof.id != expected_id || !valid_digest(&proof.digest) {
            return Err(WorkspaceError::InvalidProgress(
                "legacy completion is not a contiguous digest-bound prefix".into(),
            ));
        }
    }
    let selected_index = LEGACY_EXERCISE_IDS
        .iter()
        .position(|id| *id == legacy.selected)
        .ok_or_else(|| WorkspaceError::InvalidProgress("unknown legacy selection".into()))?;
    if legacy.completed.len() < LEGACY_EXERCISE_IDS.len() && selected_index > legacy.completed.len()
    {
        return Err(WorkspaceError::InvalidProgress(
            "legacy selected exercise is locked".into(),
        ));
    }
    if let Some(proof) = legacy.slice_complete {
        if legacy.completed.len() != LEGACY_EXERCISE_IDS.len() || proof.sources != legacy.completed
        {
            return Err(WorkspaceError::InvalidProgress(
                "invalid legacy slice completion proof".into(),
            ));
        }
    }
    Ok(Progress {
        schema_version: STATE_SCHEMA_VERSION,
        curriculum: legacy.curriculum,
        selected: legacy.selected,
        completed: legacy.completed,
        curriculum_complete: None,
    })
}

fn validate_progress(
    progress: &Progress,
    identity: &CurriculumIdentity,
) -> Result<(), WorkspaceError> {
    if progress.schema_version != STATE_SCHEMA_VERSION || &progress.curriculum != identity {
        return Err(WorkspaceError::InvalidProgress(
            "incompatible progress identity".into(),
        ));
    }
    if progress.completed.len() > EXERCISE_IDS.len() {
        return Err(WorkspaceError::InvalidProgress(
            "too many completed exercises".into(),
        ));
    }
    for (proof, expected_id) in progress.completed.iter().zip(EXERCISE_IDS) {
        if proof.id != expected_id || !valid_digest(&proof.digest) {
            return Err(WorkspaceError::InvalidProgress(
                "completion is not a contiguous digest-bound prefix".into(),
            ));
        }
    }
    let selected_index = EXERCISE_IDS
        .iter()
        .position(|id| *id == progress.selected)
        .ok_or_else(|| WorkspaceError::InvalidProgress("unknown selected exercise".into()))?;
    if progress.completed.len() < EXERCISE_IDS.len() && selected_index > progress.completed.len() {
        return Err(WorkspaceError::InvalidProgress(
            "selected exercise is locked".into(),
        ));
    }
    if let Some(proof) = &progress.curriculum_complete {
        if progress.completed.len() != EXERCISE_IDS.len() || proof.sources != progress.completed {
            return Err(WorkspaceError::InvalidProgress(
                "invalid curriculum completion proof".into(),
            ));
        }
    }
    Ok(())
}

fn validate_current_proofs(answers: &Path, progress: &Progress) -> Result<(), WorkspaceError> {
    for proof in &progress.completed {
        let current = digest(&read_regular(
            &answers.join(format!("{}.rs", proof.id)),
            Some(MAX_SOURCE_BYTES),
        )?);
        if proof.digest != current {
            return Err(WorkspaceError::InvalidProgress(format!(
                "completion digest does not match durable source: {}",
                proof.id
            )));
        }
    }
    Ok(())
}

fn reconcile_progress(answers: &Path, progress: &mut Progress) -> Result<bool, WorkspaceError> {
    let original = progress.clone();
    let mut matching = 0;
    for proof in &progress.completed {
        let current = digest(&read_regular(
            &answers.join(format!("{}.rs", proof.id)),
            Some(MAX_SOURCE_BYTES),
        )?);
        if proof.digest != current {
            break;
        }
        matching += 1;
    }
    progress.completed.truncate(matching);
    if progress.completed.len() != EXERCISE_IDS.len() {
        progress.curriculum_complete = None;
    }
    let selected_index = EXERCISE_IDS
        .iter()
        .position(|id| *id == progress.selected)
        .unwrap_or(EXERCISE_IDS.len());
    if matching < EXERCISE_IDS.len() && selected_index > matching {
        progress.selected = EXERCISE_IDS[matching].into();
    }
    Ok(*progress != original)
}

fn recover_progress(path: &Path, default: &Progress) -> Result<Progress, WorkspaceError> {
    let result = (|| {
        let state_dir = path
            .parent()
            .ok_or_else(|| WorkspaceError::RecoveryRequired("progress has no parent".into()))?;
        let backups = state_dir.join("backups");
        match fs::symlink_metadata(&backups) {
            Ok(metadata) if !metadata.file_type().is_symlink() && metadata.is_dir() => {
                set_private_directory(&backups)?;
            }
            Ok(_) => return Err(unsafe_path(&backups)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                create_private_directory(&backups)?;
                sync_directory(state_dir)?;
            }
            Err(error) => return Err(io_error(&backups, error)),
        }
        let backup = backups.join(format!("progress-{}.json", unique()));
        copy_private(path, &backup)?;
        sync_directory(&backups)?;
        atomic_write(path, &serialize_progress(default)?)?;
        Ok(default.clone())
    })();
    result.map_err(|error: WorkspaceError| {
        WorkspaceError::RecoveryRequired(format!(
            "progress recovery must be retried before editing: {error}"
        ))
    })
}

fn default_progress(identity: &CurriculumIdentity) -> Progress {
    Progress {
        schema_version: STATE_SCHEMA_VERSION,
        curriculum: identity.clone(),
        selected: EXERCISE_IDS[0].into(),
        completed: Vec::new(),
        curriculum_complete: None,
    }
}

fn serialize_progress(progress: &Progress) -> Result<Vec<u8>, WorkspaceError> {
    let mut bytes = serde_json::to_vec_pretty(progress)
        .map_err(|error| WorkspaceError::InvalidProgress(error.to_string()))?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), WorkspaceError> {
    if let Ok(metadata) = fs::symlink_metadata(path) {
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(unsafe_path(path));
        }
    }
    let parent = path
        .parent()
        .ok_or_else(|| WorkspaceError::Io("file has no parent directory".into()))?;
    require_directory(parent)?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| WorkspaceError::UnsafePath("invalid file name".into()))?;
    let temporary = parent.join(format!(".{file_name}.tmp-{}", unique()));
    let result = (|| {
        create_private_file(&temporary, bytes)?;
        fs::rename(&temporary, path).map_err(|error| io_error(path, error))?;
        sync_directory(parent)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn create_private_directory(path: &Path) -> Result<(), WorkspaceError> {
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
        .create(path)
        .map_err(|error| io_error(path, error))?;
    set_private_directory(path)
}

fn ensure_directory(path: &Path) -> Result<(), WorkspaceError> {
    match fs::symlink_metadata(path) {
        Ok(_) => {
            require_directory(path)?;
            set_private_directory(path)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            create_private_directory(path)?;
            let parent = path.parent().expect("generated directory has a parent");
            sync_directory(parent)
        }
        Err(error) => Err(io_error(path, error)),
    }
}

fn create_private_file(path: &Path, bytes: &[u8]) -> Result<(), WorkspaceError> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path).map_err(|error| io_error(path, error))?;
    set_private_file(path)?;
    file.write_all(bytes)
        .map_err(|error| io_error(path, error))?;
    file.sync_all().map_err(|error| io_error(path, error))
}

fn copy_private(source: &Path, destination: &Path) -> Result<(), WorkspaceError> {
    require_regular_file(source)?;
    let mut input = File::open(source).map_err(|error| io_error(source, error))?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut output = options
        .open(destination)
        .map_err(|error| io_error(destination, error))?;
    set_private_file(destination)?;
    io::copy(&mut input, &mut output).map_err(|error| io_error(destination, error))?;
    output
        .sync_all()
        .map_err(|error| io_error(destination, error))
}

fn read_regular(path: &Path, maximum: Option<usize>) -> Result<Vec<u8>, WorkspaceError> {
    require_regular_file(path)?;
    let metadata = fs::symlink_metadata(path).map_err(|error| io_error(path, error))?;
    if maximum.is_some_and(|maximum| metadata.len() > maximum as u64) {
        return Err(WorkspaceError::InvalidProgress(format!(
            "file exceeds limit: {}",
            path.display()
        )));
    }
    let mut file = File::open(path).map_err(|error| io_error(path, error))?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.read_to_end(&mut bytes)
        .map_err(|error| io_error(path, error))?;
    if maximum.is_some_and(|maximum| bytes.len() > maximum) {
        return Err(WorkspaceError::InvalidProgress(format!(
            "file exceeds limit: {}",
            path.display()
        )));
    }
    Ok(bytes)
}

#[cfg(unix)]
fn try_lock_exclusive(file: &File) -> io::Result<bool> {
    use std::os::fd::AsRawFd;

    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
        return Ok(true);
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::EWOULDBLOCK) {
        Ok(false)
    } else {
        Err(error)
    }
}

#[cfg(not(unix))]
fn try_lock_exclusive(_file: &File) -> io::Result<bool> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "workspace ownership is currently supported on Unix only",
    ))
}

fn open_lock_file(path: &Path) -> Result<File, WorkspaceError> {
    match fs::symlink_metadata(path) {
        Ok(_) => require_regular_file(path)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(io_error(path, error)),
    }
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    options.open(path).map_err(|error| {
        if error.raw_os_error() == Some(libc::ELOOP) {
            unsafe_path(path)
        } else {
            io_error(path, error)
        }
    })
}

fn require_directory(path: &Path) -> Result<(), WorkspaceError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| io_error(path, error))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(unsafe_path(path));
    }
    Ok(())
}

fn require_regular_file(path: &Path) -> Result<(), WorkspaceError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| io_error(path, error))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(unsafe_path(path));
    }
    Ok(())
}

#[cfg(unix)]
fn set_private_directory(path: &Path) -> Result<(), WorkspaceError> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|error| io_error(path, error))
}

#[cfg(not(unix))]
fn set_private_directory(_path: &Path) -> Result<(), WorkspaceError> {
    Ok(())
}

#[cfg(unix)]
fn set_private_file(path: &Path) -> Result<(), WorkspaceError> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .map_err(|error| io_error(path, error))
}

#[cfg(not(unix))]
fn set_private_file(_path: &Path) -> Result<(), WorkspaceError> {
    Ok(())
}

fn sync_directory(path: &Path) -> Result<(), WorkspaceError> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| io_error(path, error))
}

fn valid_digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn unique() -> String {
    let time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let count = UNIQUE_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{}-{time}-{count}", std::process::id())
}

fn curriculum_error(error: impl fmt::Display) -> WorkspaceError {
    WorkspaceError::Curriculum(error.to_string())
}

fn unsafe_path(path: &Path) -> WorkspaceError {
    WorkspaceError::UnsafePath(format!(
        "symlink or non-regular workspace path: {}",
        path.display()
    ))
}

fn io_error(path: &Path, error: impl fmt::Display) -> WorkspaceError {
    WorkspaceError::Io(format!("{}: {error}", path.display()))
}
