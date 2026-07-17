#![cfg(unix)]

use app_lib::{
    curriculum::Curriculum,
    workspace::{Workspace, WorkspaceOwner},
};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

struct TestDir(PathBuf);

impl TestDir {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "lustlings-u2-mode-child-{}-{nonce}",
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

#[test]
fn files_and_directories_remain_private_with_umask_000() {
    const CHILD: &str = "U2_PRIVATE_MODE_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let status = Command::new("sh")
            .arg("-c")
            .arg("umask 000; exec \"$0\" --exact files_and_directories_remain_private_with_umask_000 --nocapture")
            .arg(std::env::current_exe().unwrap())
            .env(CHILD, "1")
            .status()
            .unwrap();
        assert!(status.success());
        return;
    }

    use std::os::unix::fs::PermissionsExt;
    let app_data = TestDir::new();
    let curriculum =
        Curriculum::load(Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/rustlings-6.5.0"))
            .unwrap();
    let workspace =
        Workspace::open(WorkspaceOwner::acquire(&app_data.0).unwrap(), &curriculum).unwrap();
    for directory in [
        app_data.0.as_path(),
        workspace.root(),
        workspace.answers_dir(),
        workspace.state_path().parent().unwrap(),
        workspace.generated_dir(),
    ] {
        assert_eq!(
            fs::metadata(directory).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }
    for file in [
        app_data.0.join(".workspace-owner.lock"),
        workspace.root().join("Cargo.toml"),
        workspace.root().join("Cargo.lock"),
        workspace.answers_dir().join("intro1.rs"),
        workspace.state_path().to_owned(),
    ] {
        assert_eq!(
            fs::metadata(file).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}
