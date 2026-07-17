use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    fmt, fs,
    path::{Component, Path, PathBuf},
};

pub const EXERCISE_IDS: [&str; 8] = [
    "intro1",
    "intro2",
    "variables1",
    "variables2",
    "variables3",
    "variables4",
    "variables5",
    "variables6",
];
const RUSTLINGS_VERSION: &str = "6.5.0";
const UPSTREAM_COMMIT: &str = "2af9e89ba536fad01aa828b06e0ac2174bad0f6d";
const AUDITED_MANIFEST: &[u8] = include_bytes!("../resources/rustlings-6.5.0/manifest.json");

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CurriculumIdentity {
    pub rustlings_version: String,
    pub upstream_commit: String,
}

#[derive(Clone, Debug)]
pub struct Exercise {
    pub id: String,
    pub source: String,
    pub readme: String,
    pub hint: String,
    pub test: bool,
    pub strict_clippy: bool,
    pub skip_check_unsolved: bool,
}

#[derive(Debug)]
pub struct Curriculum {
    root: PathBuf,
    identity: CurriculumIdentity,
    exercises: Vec<Exercise>,
    file_digests: HashMap<String, String>,
    cargo_manifest: String,
    cargo_lockfile: String,
}

#[derive(Debug)]
pub struct CurriculumError(String);

impl fmt::Display for CurriculumError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for CurriculumError {}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    schema_version: u8,
    rustlings_version: String,
    upstream: Upstream,
    files: Vec<FileRecord>,
    exercises: Vec<ManifestExercise>,
    cargo: CargoInfrastructure,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Upstream {
    repository: String,
    tag: String,
    commit: String,
    metadata_path: String,
    license: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileRecord {
    path: String,
    sha256: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestExercise {
    id: String,
    source: String,
    readme: String,
    hint: String,
    hint_sha256: String,
    test: bool,
    strict_clippy: bool,
    skip_check_unsolved: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CargoInfrastructure {
    manifest: String,
    lockfile: String,
}

impl Curriculum {
    pub fn load(root: impl Into<PathBuf>) -> Result<Self, CurriculumError> {
        Self::load_inner(root.into(), true)
    }

    #[cfg(debug_assertions)]
    pub fn load_test_fixture(root: impl Into<PathBuf>) -> Result<Self, CurriculumError> {
        Self::load_inner(root.into(), false)
    }

    fn load_inner(root: PathBuf, require_audited_manifest: bool) -> Result<Self, CurriculumError> {
        require_directory(&root)?;
        let manifest_path = root.join("manifest.json");
        require_regular_file(&manifest_path)?;
        let manifest_bytes =
            fs::read(&manifest_path).map_err(|error| fail(&manifest_path, error))?;
        if require_audited_manifest && manifest_bytes != AUDITED_MANIFEST {
            return Err(CurriculumError(
                "curriculum manifest differs from the audited build".into(),
            ));
        }
        let manifest: Manifest = serde_json::from_slice(&manifest_bytes)
            .map_err(|error| CurriculumError(format!("invalid curriculum manifest: {error}")))?;

        if manifest.schema_version != 1
            || manifest.rustlings_version != RUSTLINGS_VERSION
            || manifest.upstream.repository != "https://github.com/rust-lang/rustlings"
            || manifest.upstream.tag != "v6.5.0"
            || manifest.upstream.commit != UPSTREAM_COMMIT
            || manifest.upstream.metadata_path != "rustlings-macros/info.toml"
            || manifest.upstream.license != "MIT"
        {
            return Err(CurriculumError("invalid curriculum identity".into()));
        }

        let ids: Vec<_> = manifest
            .exercises
            .iter()
            .map(|exercise| exercise.id.as_str())
            .collect();
        if ids != EXERCISE_IDS {
            return Err(CurriculumError(
                "exercise allowlist or order mismatch".into(),
            ));
        }

        let mut file_digests = HashMap::new();
        for record in &manifest.files {
            if !valid_relative_path(&record.path)
                || record.sha256.len() != 64
                || !record.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
                || file_digests
                    .insert(record.path.clone(), record.sha256.to_ascii_lowercase())
                    .is_some()
            {
                return Err(CurriculumError(format!(
                    "invalid curriculum inventory record: {}",
                    record.path
                )));
            }
            let path = root.join(&record.path);
            require_regular_file(&path)?;
            let actual = digest(&fs::read(&path).map_err(|error| fail(&path, error))?);
            if actual != record.sha256.to_ascii_lowercase() {
                return Err(CurriculumError(format!(
                    "curriculum digest mismatch: {}",
                    record.path
                )));
            }
        }

        let inventory: HashSet<_> = file_digests.keys().cloned().collect();
        let mut actual_files = HashSet::new();
        collect_files(&root, &root, &mut actual_files)?;
        actual_files.remove("manifest.json");
        if actual_files != inventory {
            return Err(CurriculumError(
                "curriculum contains an unlisted or missing file".into(),
            ));
        }

        let mut exercises = Vec::with_capacity(EXERCISE_IDS.len());
        for exercise in manifest.exercises {
            let directory = if exercise.id.starts_with("intro") {
                "00_intro"
            } else {
                "01_variables"
            };
            let expected_source = format!("exercises/{directory}/{}.rs", exercise.id);
            let expected_readme = format!("exercises/{directory}/README.md");
            if exercise.source != expected_source
                || exercise.readme != expected_readme
                || !inventory.contains(&exercise.source)
                || !inventory.contains(&exercise.readme)
                || digest(exercise.hint.as_bytes()) != exercise.hint_sha256
            {
                return Err(CurriculumError(format!(
                    "invalid exercise metadata: {}",
                    exercise.id
                )));
            }
            exercises.push(Exercise {
                id: exercise.id,
                source: exercise.source,
                readme: exercise.readme,
                hint: exercise.hint,
                test: exercise.test,
                strict_clippy: exercise.strict_clippy,
                skip_check_unsolved: exercise.skip_check_unsolved,
            });
        }

        if !valid_relative_path(&manifest.cargo.manifest)
            || !valid_relative_path(&manifest.cargo.lockfile)
            || !inventory.contains(&manifest.cargo.manifest)
            || !inventory.contains(&manifest.cargo.lockfile)
        {
            return Err(CurriculumError("invalid Cargo infrastructure paths".into()));
        }

        Ok(Self {
            root,
            identity: CurriculumIdentity {
                rustlings_version: RUSTLINGS_VERSION.into(),
                upstream_commit: UPSTREAM_COMMIT.into(),
            },
            exercises,
            file_digests,
            cargo_manifest: manifest.cargo.manifest,
            cargo_lockfile: manifest.cargo.lockfile,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn identity(&self) -> &CurriculumIdentity {
        &self.identity
    }

    pub fn exercises(&self) -> &[Exercise] {
        &self.exercises
    }

    pub fn exercise(&self, id: &str) -> Option<&Exercise> {
        self.exercises.iter().find(|exercise| exercise.id == id)
    }

    pub fn source_bytes(&self, id: &str) -> Result<Vec<u8>, CurriculumError> {
        let exercise = self
            .exercise(id)
            .ok_or_else(|| CurriculumError(format!("unknown exercise: {id}")))?;
        self.verified_bytes(&exercise.source)
    }

    pub fn readme(&self, id: &str) -> Result<String, CurriculumError> {
        let exercise = self
            .exercise(id)
            .ok_or_else(|| CurriculumError(format!("unknown exercise: {id}")))?;
        String::from_utf8(self.verified_bytes(&exercise.readme)?)
            .map_err(|error| CurriculumError(format!("invalid README text: {error}")))
    }

    pub fn cargo_manifest_bytes(&self) -> Result<Vec<u8>, CurriculumError> {
        self.verified_bytes(&self.cargo_manifest)
    }

    pub fn cargo_lockfile_bytes(&self) -> Result<Vec<u8>, CurriculumError> {
        self.verified_bytes(&self.cargo_lockfile)
    }

    fn verified_bytes(&self, relative: &str) -> Result<Vec<u8>, CurriculumError> {
        let path = self.root.join(relative);
        require_regular_file(&path)?;
        let bytes = fs::read(&path).map_err(|error| fail(&path, error))?;
        if digest(&bytes) != self.file_digests[relative] {
            return Err(CurriculumError(format!(
                "curriculum digest mismatch: {relative}"
            )));
        }
        Ok(bytes)
    }
}

pub(crate) fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn valid_relative_path(path: &str) -> bool {
    !path.is_empty()
        && !path.contains(['\\', ':'])
        && !path.contains("//")
        && Path::new(path)
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

fn collect_files(
    root: &Path,
    directory: &Path,
    files: &mut HashSet<String>,
) -> Result<(), CurriculumError> {
    for entry in fs::read_dir(directory).map_err(|error| fail(directory, error))? {
        let entry = entry.map_err(|error| CurriculumError(error.to_string()))?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).map_err(|error| fail(&path, error))?;
        if metadata.file_type().is_symlink() {
            return Err(CurriculumError(format!(
                "curriculum symlink: {}",
                path.display()
            )));
        }
        if metadata.is_dir() {
            collect_files(root, &path, files)?;
        } else if metadata.is_file() {
            let relative = path
                .strip_prefix(root)
                .map_err(|error| CurriculumError(error.to_string()))?
                .components()
                .map(|component| component.as_os_str().to_str())
                .collect::<Option<Vec<_>>>()
                .ok_or_else(|| CurriculumError("non-UTF-8 curriculum path".into()))?
                .join("/");
            files.insert(relative);
        } else {
            return Err(CurriculumError(format!(
                "non-regular curriculum path: {}",
                path.display()
            )));
        }
    }
    Ok(())
}

fn require_directory(path: &Path) -> Result<(), CurriculumError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| fail(path, error))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(CurriculumError(format!(
            "not a real directory: {}",
            path.display()
        )));
    }
    Ok(())
}

fn require_regular_file(path: &Path) -> Result<(), CurriculumError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| fail(path, error))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(CurriculumError(format!(
            "not a regular curriculum file: {}",
            path.display()
        )));
    }
    Ok(())
}

fn fail(path: &Path, error: impl fmt::Display) -> CurriculumError {
    CurriculumError(format!("{}: {error}", path.display()))
}
