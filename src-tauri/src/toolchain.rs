use crate::process::{ProcessOutcome, ProcessRunner, ProcessSpec};
use std::{
    collections::{BTreeMap, HashSet},
    env,
    ffi::{OsStr, OsString},
    fmt, fs,
    path::{Path, PathBuf},
    time::Duration,
};

const MINIMUM_RUST_MINOR: u32 = 88;
const PROBE_DEADLINE: Duration = Duration::from_secs(10);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RustVersion {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Toolchain {
    cargo: PathBuf,
    rustc: PathBuf,
    root: PathBuf,
    rustc_version: RustVersion,
}

impl Toolchain {
    pub async fn discover() -> Result<Self, ToolchainError> {
        discover_candidates(candidate_directories(), probe_environment()).await
    }

    pub fn cargo(&self) -> &Path {
        &self.cargo
    }

    pub fn rustc(&self) -> &Path {
        &self.rustc
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn rustc_version(&self) -> &RustVersion {
        &self.rustc_version
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ToolchainError {
    CargoMissing,
    CargoNotExecutable(PathBuf),
    CargoUnusable(PathBuf),
    RustcMissing(PathBuf),
    RustcNotExecutable(PathBuf),
    RustcUnusable(PathBuf),
    InconsistentRoot { cargo: PathBuf, rustc: PathBuf },
    RustcVersionUnrecognized(String),
    RustcTooOld(RustVersion),
    ClippyMissing(PathBuf),
}

impl fmt::Display for ToolchainError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CargoMissing => formatter.write_str("Cargo was not found"),
            Self::CargoNotExecutable(path) => {
                write!(formatter, "Cargo is not executable: {}", path.display())
            }
            Self::CargoUnusable(path) => {
                write!(formatter, "Cargo could not be run: {}", path.display())
            }
            Self::RustcMissing(root) => {
                write!(
                    formatter,
                    "rustc was not found beside Cargo in {}",
                    root.display()
                )
            }
            Self::RustcNotExecutable(path) => {
                write!(formatter, "rustc is not executable: {}", path.display())
            }
            Self::RustcUnusable(path) => {
                write!(formatter, "rustc could not be run: {}", path.display())
            }
            Self::InconsistentRoot { cargo, rustc } => write!(
                formatter,
                "Cargo and rustc resolve to different toolchain roots: {} and {}",
                cargo.display(),
                rustc.display()
            ),
            Self::RustcVersionUnrecognized(version) => {
                write!(formatter, "unrecognized rustc version: {version}")
            }
            Self::RustcTooOld(version) => write!(
                formatter,
                "rustc {}.{}.{} is older than 1.{MINIMUM_RUST_MINOR}",
                version.major, version.minor, version.patch
            ),
            Self::ClippyMissing(cargo) => {
                write!(
                    formatter,
                    "Cargo Clippy is unavailable through {}",
                    cargo.display()
                )
            }
        }
    }
}

impl std::error::Error for ToolchainError {}

fn candidate_directories() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(path) = env::var_os("PATH") {
        candidates.extend(env::split_paths(&path));
    }
    if let Some(home) = env::var_os("HOME") {
        candidates.push(PathBuf::from(home).join(".cargo/bin"));
    }
    candidates.push(PathBuf::from("/opt/homebrew/bin"));
    candidates.push(PathBuf::from("/usr/local/bin"));

    let mut seen = HashSet::new();
    candidates.retain(|candidate| seen.insert(candidate.clone()));
    candidates
}

fn probe_environment() -> BTreeMap<OsString, OsString> {
    ["PATH", "HOME", "CARGO_HOME", "RUSTUP_HOME"]
        .into_iter()
        .filter_map(|key| env::var_os(key).map(|value| (OsString::from(key), value)))
        .collect()
}

async fn discover_candidates(
    candidates: Vec<PathBuf>,
    environment: BTreeMap<OsString, OsString>,
) -> Result<Toolchain, ToolchainError> {
    let mut first_error = None;
    let mut found_cargo = false;
    for candidate in candidates {
        let cargo_candidate = candidate.join("cargo");
        if fs::symlink_metadata(&cargo_candidate).is_err() {
            continue;
        }
        found_cargo = true;
        match inspect_candidate(&candidate, &environment).await {
            Ok(toolchain) => return Ok(toolchain),
            Err(error) if first_error.is_none() => first_error = Some(error),
            Err(_) => {}
        }
    }
    Err(first_error.unwrap_or(if found_cargo {
        ToolchainError::CargoUnusable(PathBuf::from("cargo"))
    } else {
        ToolchainError::CargoMissing
    }))
}

async fn inspect_candidate(
    candidate: &Path,
    environment: &BTreeMap<OsString, OsString>,
) -> Result<Toolchain, ToolchainError> {
    let cargo_candidate = candidate.join("cargo");
    let rustc_candidate = candidate.join("rustc");
    let cargo = canonical_tool(&cargo_candidate)
        .map_err(|_| ToolchainError::CargoNotExecutable(cargo_candidate.clone()))?;
    if fs::symlink_metadata(&rustc_candidate).is_err() {
        return Err(ToolchainError::RustcMissing(candidate.to_owned()));
    }
    let rustc = canonical_tool(&rustc_candidate)
        .map_err(|_| ToolchainError::RustcNotExecutable(rustc_candidate.clone()))?;
    let (cargo, rustc) = resolve_rustup_proxy(candidate, cargo, rustc, environment).await?;
    let cargo_root = cargo.parent().expect("canonical executable has a parent");
    let rustc_root = rustc.parent().expect("canonical executable has a parent");
    if cargo_root != rustc_root {
        return Err(ToolchainError::InconsistentRoot { cargo, rustc });
    }

    let cargo_version = probe(&cargo, [OsStr::new("--version")], environment).await;
    if !cargo_version.success || !cargo_version.stdout.starts_with("cargo ") {
        return Err(ToolchainError::CargoUnusable(cargo));
    }
    let rustc_probe = probe(&rustc, [OsStr::new("--version")], environment).await;
    if !rustc_probe.success {
        return Err(ToolchainError::RustcUnusable(rustc));
    }
    let rustc_version = parse_rustc_version(&rustc_probe.stdout)?;
    if rustc_version.major < 1
        || (rustc_version.major == 1 && rustc_version.minor < MINIMUM_RUST_MINOR)
    {
        return Err(ToolchainError::RustcTooOld(rustc_version));
    }
    let clippy = probe(
        &cargo,
        [OsStr::new("clippy"), OsStr::new("--version")],
        environment,
    )
    .await;
    if !clippy.success || !clippy.stdout.starts_with("clippy ") {
        return Err(ToolchainError::ClippyMissing(cargo));
    }

    Ok(Toolchain {
        root: cargo_root.to_owned(),
        cargo,
        rustc,
        rustc_version,
    })
}

fn canonical_tool(path: &Path) -> Result<PathBuf, ()> {
    crate::process::canonical_executable(path).map_err(|_| ())
}

async fn resolve_rustup_proxy(
    candidate: &Path,
    cargo: PathBuf,
    rustc: PathBuf,
    environment: &BTreeMap<OsString, OsString>,
) -> Result<(PathBuf, PathBuf), ToolchainError> {
    let rustup = candidate.join("rustup");
    if cargo != rustc || canonical_tool(&rustup).ok().as_ref() != Some(&cargo) {
        return Ok((cargo, rustc));
    }
    let (cargo_probe, rustc_probe) = tokio::join!(
        probe(
            &rustup,
            [OsStr::new("which"), OsStr::new("cargo")],
            environment,
        ),
        probe(
            &rustup,
            [OsStr::new("which"), OsStr::new("rustc")],
            environment,
        ),
    );
    if !cargo_probe.success {
        return Err(ToolchainError::CargoUnusable(cargo));
    }
    if !rustc_probe.success {
        return Err(ToolchainError::RustcUnusable(rustc));
    }
    let cargo_path = canonical_tool(Path::new(cargo_probe.stdout.trim()))
        .map_err(|_| ToolchainError::CargoUnusable(cargo))?;
    let rustc_path = canonical_tool(Path::new(rustc_probe.stdout.trim()))
        .map_err(|_| ToolchainError::RustcUnusable(rustc))?;
    Ok((cargo_path, rustc_path))
}

struct Probe {
    success: bool,
    stdout: String,
}

async fn probe<'a, I>(
    executable: &Path,
    arguments: I,
    environment: &BTreeMap<OsString, OsString>,
) -> Probe
where
    I: IntoIterator<Item = &'a OsStr>,
{
    let spec = ProcessSpec::new(executable, executable.parent().unwrap_or(Path::new("/")))
        .args(arguments)
        .deadline(PROBE_DEADLINE);
    let spec = environment
        .iter()
        .fold(spec, |spec, (key, value)| spec.env(key, value));
    let result = match ProcessRunner::new().start(spec).await {
        Ok(started) => started.wait().await.ok(),
        Err(_) => None,
    };
    match result {
        Some(result) => Probe {
            success: matches!(
                result.outcome,
                ProcessOutcome::Exited {
                    code: Some(0),
                    signal: None
                }
            ),
            stdout: String::from_utf8_lossy(&result.stdout).trim().to_owned(),
        },
        None => Probe {
            success: false,
            stdout: String::new(),
        },
    }
}

fn parse_rustc_version(output: &str) -> Result<RustVersion, ToolchainError> {
    let value = output
        .split_whitespace()
        .nth(1)
        .ok_or_else(|| ToolchainError::RustcVersionUnrecognized(output.into()))?;
    let core = value.split('-').next().unwrap_or(value);
    let mut components = core.split('.');
    let version = RustVersion {
        major: components
            .next()
            .and_then(|part| part.parse().ok())
            .ok_or_else(|| ToolchainError::RustcVersionUnrecognized(output.into()))?,
        minor: components
            .next()
            .and_then(|part| part.parse().ok())
            .ok_or_else(|| ToolchainError::RustcVersionUnrecognized(output.into()))?,
        patch: components
            .next()
            .and_then(|part| part.parse().ok())
            .ok_or_else(|| ToolchainError::RustcVersionUnrecognized(output.into()))?,
    };
    if components.next().is_some() || !output.starts_with("rustc ") {
        return Err(ToolchainError::RustcVersionUnrecognized(output.into()));
    }
    Ok(version)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::{
        fs,
        os::unix::fs::{symlink, PermissionsExt},
        sync::atomic::{AtomicU64, Ordering},
    };

    static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

    struct TestDir(PathBuf);

    impl TestDir {
        fn new() -> Self {
            let path = env::temp_dir().join(format!(
                "lustlings-toolchain-{}-{}",
                std::process::id(),
                NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }

        fn tool(&self, name: &str, body: &str) {
            let path = self.0.join(name);
            fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn cargo_script(clippy: bool) -> String {
        format!(
            "if [ \"$1\" = \"--version\" ]; then echo 'cargo 1.88.0'; exit 0; fi\nif [ \"$1\" = \"clippy\" ] && [ \"$2\" = \"--version\" ]; then {} fi\nexit 1",
            if clippy {
                "echo 'clippy 0.1.88'; exit 0;"
            } else {
                "exit 1;"
            }
        )
    }

    fn rustc_script(version: &str) -> String {
        format!("echo 'rustc {version} (fake 2026-01-01)'")
    }

    async fn discover(directory: &TestDir) -> Result<Toolchain, ToolchainError> {
        discover_candidates(vec![directory.0.clone()], BTreeMap::new()).await
    }

    #[tokio::test]
    async fn accepts_same_root_rust_1_88_and_clippy() {
        let directory = TestDir::new();
        directory.tool("cargo", &cargo_script(true));
        directory.tool("rustc", &rustc_script("1.88.0"));
        let toolchain = discover(&directory).await.unwrap();
        assert_eq!(toolchain.root(), directory.0.canonicalize().unwrap());
        assert_eq!(toolchain.rustc_version().minor, 88);
        assert!(toolchain.cargo().is_absolute());
        assert!(toolchain.rustc().is_absolute());
    }

    #[tokio::test]
    async fn discovers_homebrew_rustup_init_proxies() {
        let proxy = TestDir::new();
        let toolchain = TestDir::new();
        toolchain.tool("cargo", &cargo_script(true));
        toolchain.tool("rustc", &rustc_script("1.88.0"));
        proxy.tool(
            "rustup-init",
            &format!(
                "case \"$1:$2\" in\nwhich:cargo) echo '{}'; exit 0;;\nwhich:rustc) echo '{}'; exit 0;;\nesac\nexit 1",
                toolchain.0.join("cargo").display(),
                toolchain.0.join("rustc").display()
            ),
        );
        for name in ["cargo", "rustc", "rustup"] {
            symlink(proxy.0.join("rustup-init"), proxy.0.join(name)).unwrap();
        }

        let discovered = discover(&proxy).await.unwrap();
        assert_eq!(discovered.root(), toolchain.0.canonicalize().unwrap());
    }

    #[tokio::test]
    async fn distinguishes_missing_non_executable_old_and_missing_clippy() {
        let empty = TestDir::new();
        assert_eq!(discover(&empty).await, Err(ToolchainError::CargoMissing));

        let non_executable = TestDir::new();
        fs::write(non_executable.0.join("cargo"), "not executable").unwrap();
        assert!(matches!(
            discover(&non_executable).await,
            Err(ToolchainError::CargoNotExecutable(_))
        ));

        let no_rustc = TestDir::new();
        no_rustc.tool("cargo", &cargo_script(true));
        assert!(matches!(
            discover(&no_rustc).await,
            Err(ToolchainError::RustcMissing(_))
        ));

        let old = TestDir::new();
        old.tool("cargo", &cargo_script(true));
        old.tool("rustc", &rustc_script("1.87.0"));
        assert!(matches!(
            discover(&old).await,
            Err(ToolchainError::RustcTooOld(RustVersion { minor: 87, .. }))
        ));

        let no_clippy = TestDir::new();
        no_clippy.tool("cargo", &cargo_script(false));
        no_clippy.tool("rustc", &rustc_script("1.88.0"));
        assert!(matches!(
            discover(&no_clippy).await,
            Err(ToolchainError::ClippyMissing(_))
        ));
    }

    #[tokio::test]
    async fn discovers_the_current_system_toolchain() {
        let toolchain = Toolchain::discover().await.unwrap();
        assert!(toolchain.rustc_version().major > 1 || toolchain.rustc_version().minor >= 88);
        assert_eq!(toolchain.cargo().parent(), toolchain.rustc().parent());
    }
}
