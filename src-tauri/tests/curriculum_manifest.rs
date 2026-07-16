use app_lib::curriculum::Curriculum;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs,
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

const RESOURCE_DIR: &str = "resources/rustlings-6.5.0";
const IDS: [&str; 8] = [
    "intro1",
    "intro2",
    "variables1",
    "variables2",
    "variables3",
    "variables4",
    "variables5",
    "variables6",
];
const UPSTREAM_COMMIT: &str = "2af9e89ba536fad01aa828b06e0ac2174bad0f6d";

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    schema_version: u8,
    rustlings_version: String,
    upstream: Upstream,
    files: Vec<FileRecord>,
    exercises: Vec<Exercise>,
    cargo: CargoInfrastructure,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Upstream {
    repository: String,
    tag: String,
    commit: String,
    metadata_path: String,
    license: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileRecord {
    path: String,
    sha256: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Exercise {
    id: String,
    source: String,
    readme: String,
    hint: String,
    hint_sha256: String,
    test: bool,
    strict_clippy: bool,
    skip_check_unsolved: bool,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CargoInfrastructure {
    manifest: String,
    lockfile: String,
}

#[derive(Deserialize)]
struct InfoFile {
    exercises: Vec<InfoExercise>,
}

#[derive(Deserialize)]
struct InfoExercise {
    name: String,
    hint: String,
    test: bool,
    #[serde(default)]
    strict_clippy: bool,
    #[serde(default)]
    skip_check_unsolved: bool,
}

fn resource_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(RESOURCE_DIR)
}

struct TemporaryResources(PathBuf);

impl TemporaryResources {
    fn copy_bundled() -> Self {
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "rustlings-desktop-curriculum-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        ));
        copy_directory(&resource_root(), &path);
        Self(path)
    }
}

impl Drop for TemporaryResources {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn copy_directory(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let file_type = entry.file_type().unwrap();
        let destination = destination.join(entry.file_name());
        if file_type.is_dir() {
            copy_directory(&entry.path(), &destination);
        } else {
            assert!(file_type.is_file());
            fs::copy(entry.path(), destination).unwrap();
        }
    }
}

fn assert_production_load_rejects(root: &Path, value: &serde_json::Value, case: &str) {
    fs::write(
        root.join("manifest.json"),
        serde_json::to_vec(value).unwrap(),
    )
    .unwrap();
    assert!(Curriculum::load(root).is_err(), "accepted {case}");
}

fn parse_manifest(bytes: &[u8]) -> Result<Manifest, String> {
    serde_json::from_slice(bytes).map_err(|error| error.to_string())
}

fn sha256(bytes: &[u8]) -> String {
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

fn validate_manifest(manifest: &Manifest, root: &Path) -> Result<(), String> {
    if manifest.schema_version != 1
        || manifest.rustlings_version != "6.5.0"
        || manifest.upstream.repository != "https://github.com/rust-lang/rustlings"
        || manifest.upstream.tag != "v6.5.0"
        || manifest.upstream.commit != UPSTREAM_COMMIT
        || manifest.upstream.metadata_path != "rustlings-macros/info.toml"
        || manifest.upstream.license != "MIT"
    {
        return Err("invalid curriculum identity".into());
    }

    let actual_ids: Vec<_> = manifest
        .exercises
        .iter()
        .map(|exercise| exercise.id.as_str())
        .collect();
    if actual_ids != IDS {
        return Err("exercise allowlist or order mismatch".into());
    }

    let mut inventory = HashSet::new();
    for file in &manifest.files {
        if !valid_relative_path(&file.path) || !inventory.insert(file.path.clone()) {
            return Err(format!(
                "invalid or duplicate inventory path: {}",
                file.path
            ));
        }
        if file.sha256.len() != 64 || !file.sha256.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(format!("invalid digest: {}", file.path));
        }
        let path = root.join(&file.path);
        let metadata = fs::symlink_metadata(&path).map_err(|error| error.to_string())?;
        if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
            return Err(format!("not a regular file: {}", file.path));
        }
        let bytes = fs::read(&path).map_err(|error| error.to_string())?;
        if sha256(&bytes) != file.sha256 {
            return Err(format!("digest mismatch: {}", file.path));
        }
        #[cfg(unix)]
        if std::os::unix::fs::PermissionsExt::mode(&metadata.permissions()) & 0o111 != 0 {
            return Err(format!("executable resource: {}", file.path));
        }
    }

    for exercise in &manifest.exercises {
        let directory = if exercise.id.starts_with("intro") {
            "00_intro"
        } else {
            "01_variables"
        };
        let expected_source = format!("exercises/{directory}/{}.rs", exercise.id);
        let expected_readme = format!("exercises/{directory}/README.md");
        if !valid_relative_path(&exercise.source)
            || !valid_relative_path(&exercise.readme)
            || exercise.source != expected_source
            || exercise.readme != expected_readme
            || !inventory.contains(exercise.source.as_str())
            || !inventory.contains(exercise.readme.as_str())
            || sha256(exercise.hint.as_bytes()) != exercise.hint_sha256
        {
            return Err(format!("invalid exercise metadata: {}", exercise.id));
        }
    }
    if !valid_relative_path(&manifest.cargo.manifest)
        || !valid_relative_path(&manifest.cargo.lockfile)
        || !inventory.contains(manifest.cargo.manifest.as_str())
        || !inventory.contains(manifest.cargo.lockfile.as_str())
    {
        return Err("invalid Cargo infrastructure paths".into());
    }

    let mut actual_files = HashSet::new();
    collect_files(root, root, &mut actual_files)?;
    actual_files.remove("manifest.json");
    if actual_files != inventory {
        return Err("resource inventory contains an unlisted or missing file".into());
    }

    Ok(())
}

fn collect_files(root: &Path, dir: &Path, files: &mut HashSet<String>) -> Result<(), String> {
    for entry in fs::read_dir(dir).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let metadata = fs::symlink_metadata(entry.path()).map_err(|error| error.to_string())?;
        if metadata.file_type().is_symlink() {
            return Err(format!("resource symlink: {}", entry.path().display()));
        }
        if metadata.is_dir() {
            collect_files(root, &entry.path(), files)?;
        } else if metadata.is_file() {
            let relative = entry
                .path()
                .strip_prefix(root)
                .map_err(|error| error.to_string())?
                .to_str()
                .ok_or("non-UTF-8 resource path")?
                .to_owned();
            files.insert(relative);
        } else {
            return Err(format!("non-regular resource: {}", entry.path().display()));
        }
    }
    Ok(())
}

fn load_valid_manifest() -> Manifest {
    let root = resource_root();
    Curriculum::load(&root).expect("bundled manifest must pass the production loader");
    let bytes = fs::read(root.join("manifest.json")).expect("bundled manifest must exist");
    let manifest = parse_manifest(&bytes).expect("bundled manifest must match the strict schema");
    validate_manifest(&manifest, &root).expect("bundled manifest must pass its inventory contract");
    manifest
}

#[test]
fn bundled_curriculum_matches_the_pinned_upstream_subset() {
    let root = resource_root();
    let manifest = load_valid_manifest();

    let audited_files = [
        (
            "Cargo.lock",
            "a0bd590f1d49be90648370fbbff4c1d2c4fd1078a005ab8d3d40d1a52d197bf5",
        ),
        (
            "Cargo.toml",
            "e757ce9a7fc3eb81135fe96d4cfcf92181286b94812b0f5a82123a83a3409161",
        ),
        (
            "LICENSE",
            "73a94c6d29e8563b3d7bf89a118572f01b4a0a3c984b15c02b95ce2c93f71b2f",
        ),
        (
            "exercises/00_intro/README.md",
            "ecbba05fa26428e1f2b254b535acc45df0374673dbb1dab6d86b3f0588e69fe2",
        ),
        (
            "exercises/00_intro/intro1.rs",
            "bebc067ccb4a2f3cbc08d47ca427e53a7820bbafe4f77a52c3b9c4cd082dc915",
        ),
        (
            "exercises/00_intro/intro2.rs",
            "7dc5b4ec2a1eac95533a0270e25001cc26f7b039ca00370be94a5c5ce902528b",
        ),
        (
            "exercises/01_variables/README.md",
            "80729d162a0008147958d4d3d886e537eae46bdc10cb175bdf12e2885b403c28",
        ),
        (
            "exercises/01_variables/variables1.rs",
            "d79238495871f0b0dfbfceb39841e6c952ba384eaf127801ecd86130ddf2e4f8",
        ),
        (
            "exercises/01_variables/variables2.rs",
            "9dc52f662da4804960ae0082d542125e2c01c182045cfaab42d03ae899e085cc",
        ),
        (
            "exercises/01_variables/variables3.rs",
            "8a05af8deaff421343204affa11228de4f0f30a22399bdfb25b17854e57f1efd",
        ),
        (
            "exercises/01_variables/variables4.rs",
            "5f7806aa7d66c2d981915cf8ff923796417e77f6eb3b3dae1f16afbbf824a943",
        ),
        (
            "exercises/01_variables/variables5.rs",
            "19aa4cffdcb813f4057504e01b47f333c05120453e0e416c37e9a48d19673382",
        ),
        (
            "exercises/01_variables/variables6.rs",
            "810becc2766849107c3bae8a49b8c32a5822141a6545571c0f8027a62d015bdb",
        ),
        (
            "info.toml",
            "4956fc59679892686a530a520f197f2d458ce0c8f1dbbee9e9b794d9b451cdec",
        ),
    ];
    for (path, expected) in audited_files {
        assert_eq!(
            sha256(&fs::read(root.join(path)).unwrap()),
            expected,
            "{path}"
        );
    }

    let info: InfoFile =
        toml::from_str(&fs::read_to_string(root.join("info.toml")).unwrap()).unwrap();
    assert_eq!(info.exercises.len(), IDS.len());
    for ((metadata, exercise), id) in info.exercises.iter().zip(&manifest.exercises).zip(IDS) {
        assert_eq!(metadata.name, id);
        assert_eq!(exercise.id, id);
        assert_eq!(exercise.hint, metadata.hint);
        assert_eq!(exercise.test, metadata.test);
        assert_eq!(exercise.strict_clippy, metadata.strict_clippy);
        assert_eq!(exercise.skip_check_unsolved, metadata.skip_check_unsolved);
    }
}

#[test]
fn production_loader_rejects_malformed_duplicate_and_unsafe_manifests() {
    let root = TemporaryResources::copy_bundled();
    let bytes = fs::read(root.0.join("manifest.json")).unwrap();
    let original: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

    let mut changed = original.clone();
    changed["unexpected"] = true.into();
    assert_production_load_rejects(&root.0, &changed, "unknown manifest field");
    changed = original.clone();
    changed["exercises"][0]["unexpected"] = true.into();
    assert_production_load_rejects(&root.0, &changed, "unknown exercise field");
    changed = original.clone();
    changed["exercises"][0]["test"] = 1.into();
    assert_production_load_rejects(&root.0, &changed, "malformed exercise field");

    changed = original.clone();
    changed["exercises"][1]["id"] = "intro1".into();
    assert_production_load_rejects(&root.0, &changed, "duplicate exercise ID");
    changed = original.clone();
    changed["exercises"][0]["id"] = "surprise".into();
    assert_production_load_rejects(&root.0, &changed, "unknown exercise ID");

    for path in [
        "/tmp/intro1.rs",
        "../intro1.rs",
        "C:/intro1.rs",
        "exercises\\intro1.rs",
        "exercises//intro1.rs",
        "./intro1.rs",
    ] {
        changed = original.clone();
        changed["exercises"][0]["source"] = path.into();
        assert_production_load_rejects(&root.0, &changed, path);
    }
}

#[test]
fn production_loader_rejects_identity_and_every_content_digest_change() {
    let root = TemporaryResources::copy_bundled();
    let bytes = fs::read(root.0.join("manifest.json")).unwrap();
    let original: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

    let mut changed = original.clone();
    changed["upstream"]["commit"] = "0af9e89ba536fad01aa828b06e0ac2174bad0f6d".into();
    assert_production_load_rejects(&root.0, &changed, "changed upstream commit");

    for index in 0..original["files"].as_array().unwrap().len() {
        changed = original.clone();
        let digest = changed["files"][index]["sha256"].as_str().unwrap();
        let replacement = if digest.starts_with('0') { "1" } else { "0" };
        changed["files"][index]["sha256"] = format!("{replacement}{}", &digest[1..]).into();
        let path = changed["files"][index]["path"].as_str().unwrap();
        assert_production_load_rejects(&root.0, &changed, &format!("changed digest for {path}"));
    }
    for index in 0..original["exercises"].as_array().unwrap().len() {
        changed = original.clone();
        changed["exercises"][index]["hint"] =
            format!("{}!", changed["exercises"][index]["hint"].as_str().unwrap()).into();
        let id = changed["exercises"][index]["id"].as_str().unwrap();
        assert_production_load_rejects(&root.0, &changed, &format!("changed hint for {id}"));
    }
}

#[test]
fn production_loader_rejects_self_consistent_resource_and_manifest_tampering() {
    let root = TemporaryResources::copy_bundled();
    let manifest_path = root.0.join("manifest.json");
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
    let resource = manifest["files"][0]["path"].as_str().unwrap();
    let changed_bytes = b"self-consistent tamper";
    fs::write(root.0.join(resource), changed_bytes).unwrap();
    manifest["files"][0]["sha256"] = sha256(changed_bytes).into();

    assert_production_load_rejects(
        &root.0,
        &manifest,
        "self-consistent resource and manifest tamper",
    );
}

#[test]
fn cargo_infrastructure_is_dependency_free_and_has_only_the_eight_bins() {
    let root = resource_root();
    let manifest = load_valid_manifest();
    let cargo: toml::Value =
        toml::from_str(&fs::read_to_string(root.join(&manifest.cargo.manifest)).unwrap()).unwrap();
    let package = cargo["package"].as_table().unwrap();
    assert_eq!(package["name"].as_str(), Some("exercises"));
    assert_eq!(package["edition"].as_str(), Some("2024"));
    assert_eq!(package["publish"].as_bool(), Some(false));
    assert!(cargo.get("dependencies").is_none());
    assert!(cargo.get("build-dependencies").is_none());
    assert!(package.get("build").is_none());
    assert!(cargo.get("workspace").is_none());
    assert!(cargo.get("patch").is_none());
    assert!(cargo.get("source").is_none());

    let bins = cargo["bin"].as_array().unwrap();
    assert_eq!(bins.len(), IDS.len());
    for (bin, id) in bins.iter().zip(IDS) {
        assert_eq!(bin["name"].as_str(), Some(id));
        assert_eq!(
            bin["path"].as_str(),
            Some(
                format!(
                    "exercises/{}/{}.rs",
                    if id.starts_with("intro") {
                        "00_intro"
                    } else {
                        "01_variables"
                    },
                    id
                )
                .as_str()
            )
        );
    }

    let lock = fs::read_to_string(root.join(&manifest.cargo.lockfile)).unwrap();
    assert_eq!(lock, "# This file is automatically @generated by Cargo.\n# It is not intended for manual editing.\nversion = 4\n\n[[package]]\nname = \"exercises\"\nversion = \"0.0.0\"\n");
}

#[test]
fn resources_exclude_solutions_executables_and_cargo_configuration() {
    let root = resource_root();
    let manifest = load_valid_manifest();
    for file in &manifest.files {
        let path = Path::new(&file.path);
        let name = path.file_name().unwrap().to_string_lossy();
        assert!(!file.path.contains("solutions"));
        assert!(!name.contains("_sol"));
        assert_ne!(name, "build.rs");
        assert_ne!(file.path, ".cargo/config");
        assert_ne!(file.path, ".cargo/config.toml");
    }
    assert!(!root.join("rustlings").exists());
}

#[test]
fn license_notice_and_tauri_dev_resources_are_present() {
    let root = resource_root();
    load_valid_manifest();
    let license = fs::read_to_string(root.join("LICENSE")).unwrap();
    assert!(license.starts_with("The MIT License (MIT)"));
    let notice =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../THIRD_PARTY_NOTICES.md"))
            .unwrap();
    assert!(notice.contains("Rustlings 6.5.0"));
    assert!(notice.contains("https://github.com/rust-lang/rustlings"));
    assert!(notice.contains(UPSTREAM_COMMIT));

    let executable = std::env::current_exe().unwrap();
    let profile_dir = executable.parent().unwrap().parent().unwrap();
    let resolved = profile_dir.join(RESOURCE_DIR).join("manifest.json");
    assert!(
        resolved.is_file(),
        "Tauri build did not copy the configured dev resource to {}",
        resolved.display()
    );
    assert!(parse_manifest(&fs::read(resolved).unwrap()).is_ok());
}
