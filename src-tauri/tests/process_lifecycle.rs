#![cfg(unix)]

use app_lib::process::{
    CancelResult, ProcessConfig, ProcessOutcome, ProcessRunner, ProcessSpec, StartError,
    MAX_RETURNED_OUTPUT_BYTES,
};
use std::{
    env, fs,
    io::{self, Write},
    path::{Path, PathBuf},
    process::Command,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

struct TestDir(PathBuf);

impl TestDir {
    fn new(label: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = env::temp_dir().join(format!(
            "lustlings-u3-{label}-{}-{nonce}",
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

fn fixture_spec(mode: &str, directory: &Path) -> ProcessSpec {
    ProcessSpec::new(
        env::current_exe().unwrap().canonicalize().unwrap(),
        directory,
    )
    .args(["--ignored", "--exact", "process_fixture", "--nocapture"])
    .env("U3_PROCESS_FIXTURE", mode)
    .env("U3_PID_FILE", directory.join("pids"))
}

fn fast_runner() -> ProcessRunner {
    ProcessRunner::with_config(ProcessConfig {
        default_deadline: Duration::from_secs(5),
        termination_grace: Duration::from_millis(100),
    })
}

fn recorded_pids(path: &Path, count: usize) -> Vec<i32> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let pids: Vec<_> = fs::read_to_string(path)
            .unwrap_or_default()
            .lines()
            .filter_map(|line| line.parse().ok())
            .collect();
        if pids.len() >= count {
            return pids;
        }
        assert!(
            Instant::now() < deadline,
            "fixture did not record {count} PIDs"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

fn process_exists(pid: i32) -> bool {
    if unsafe { libc::kill(pid, 0) } == 0 {
        return true;
    }
    io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
}

fn assert_processes_gone(pids: &[i32]) {
    let deadline = Instant::now() + Duration::from_secs(2);
    for pid in pids {
        while process_exists(*pid) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        assert!(!process_exists(*pid), "process {pid} survived cleanup");
    }
}

#[tokio::test]
async fn cancellation_removes_ordinary_child_and_grandchild_before_unlock() {
    let directory = TestDir::new("cancel-group");
    let runner = fast_runner();
    let started = runner
        .start(fixture_spec("ordinary", &directory.0))
        .await
        .unwrap();
    let pids = recorded_pids(&directory.0.join("pids"), 2);

    assert_eq!(runner.cancel(started.id()).await, CancelResult::Requested);
    let result = started.wait().await.unwrap();

    assert_eq!(result.outcome, ProcessOutcome::Cancelled);
    assert_processes_gone(&pids);
    let follow_up = runner
        .start(fixture_spec("exit", &directory.0))
        .await
        .unwrap()
        .wait()
        .await
        .unwrap();
    assert!(matches!(
        follow_up.outcome,
        ProcessOutcome::Exited { code: Some(0), .. }
    ));
}

#[tokio::test]
async fn timeout_removes_the_launched_group_and_releases_the_runner() {
    assert_eq!(
        ProcessConfig::default().default_deadline,
        Duration::from_secs(60)
    );
    let directory = TestDir::new("timeout");
    let runner = fast_runner();
    let started = runner
        .start(fixture_spec("ordinary", &directory.0).deadline(Duration::from_millis(100)))
        .await
        .unwrap();
    let pids = recorded_pids(&directory.0.join("pids"), 2);
    let result = started.wait().await.unwrap();

    assert_eq!(result.outcome, ProcessOutcome::TimedOut);
    assert_processes_gone(&pids);
    runner
        .start(fixture_spec("exit", &directory.0))
        .await
        .unwrap()
        .wait()
        .await
        .unwrap();
}

#[tokio::test]
async fn busy_and_late_cancel_are_scoped_to_the_matching_unique_run_id() {
    let directory = TestDir::new("busy");
    let runner = fast_runner();
    let first = runner
        .start(fixture_spec("ordinary", &directory.0))
        .await
        .unwrap();
    let first_id = first.id();
    recorded_pids(&directory.0.join("pids"), 2);
    assert!(matches!(
        runner.start(fixture_spec("exit", &directory.0)).await,
        Err(StartError::Busy { active_run_id }) if active_run_id == first_id
    ));
    assert_eq!(runner.cancel(first_id).await, CancelResult::Requested);
    first.wait().await.unwrap();

    fs::write(directory.0.join("pids"), []).unwrap();
    let second = runner
        .start(fixture_spec("ordinary", &directory.0))
        .await
        .unwrap();
    let second_id = second.id();
    assert_ne!(first_id, second_id);
    let second_pids = recorded_pids(&directory.0.join("pids"), 2);
    assert_eq!(runner.cancel(first_id).await, CancelResult::IdMismatch);
    assert!(second_pids.iter().all(|pid| process_exists(*pid)));
    assert_eq!(runner.cancel(second_id).await, CancelResult::Requested);
    second.wait().await.unwrap();
    assert_processes_gone(&second_pids);
    assert_eq!(runner.cancel(second_id).await, CancelResult::NotActive);
}

#[tokio::test]
async fn signal_ignore_escalates_to_forced_group_kill() {
    let directory = TestDir::new("signal-ignore");
    let runner = fast_runner();
    let started = runner
        .start(fixture_spec("ignore-term", &directory.0))
        .await
        .unwrap();
    let pids = recorded_pids(&directory.0.join("pids"), 2);
    let before = Instant::now();
    assert_eq!(runner.cancel(started.id()).await, CancelResult::Requested);
    let result = started.wait().await.unwrap();

    assert_eq!(result.outcome, ProcessOutcome::Cancelled);
    assert!(before.elapsed() >= Duration::from_millis(80));
    assert_processes_gone(&pids);
}

#[tokio::test]
async fn early_direct_child_exit_still_cleans_up_its_grandchild() {
    let directory = TestDir::new("early-exit");
    let runner = fast_runner();
    let result = runner
        .start(fixture_spec("early-exit", &directory.0))
        .await
        .unwrap()
        .wait()
        .await
        .unwrap();
    let pids = recorded_pids(&directory.0.join("pids"), 2);

    assert!(matches!(
        result.outcome,
        ProcessOutcome::Exited { code: Some(0), .. }
    ));
    assert_processes_gone(&pids);
}

#[tokio::test]
async fn natural_exit_cancel_races_reap_and_leave_the_runner_reusable() {
    for iteration in 0..8 {
        let directory = TestDir::new(&format!("race-{iteration}"));
        let runner = fast_runner();
        let started = runner
            .start(fixture_spec("brief", &directory.0))
            .await
            .unwrap();
        let pid = recorded_pids(&directory.0.join("pids"), 1);
        if iteration % 2 == 0 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let _ = runner.cancel(started.id()).await;
        let result = started.wait().await.unwrap();
        assert!(matches!(
            result.outcome,
            ProcessOutcome::Exited { code: Some(0), .. } | ProcessOutcome::Cancelled
        ));
        assert_processes_gone(&pid);
        runner
            .start(fixture_spec("exit", &directory.0))
            .await
            .unwrap()
            .wait()
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn concurrent_stream_flood_and_no_newline_records_end_as_output_limit() {
    for mode in [
        "flood",
        "no-newline-stdout",
        "no-newline-stderr",
        "finite-record",
        "finite-aggregate",
    ] {
        let directory = TestDir::new(mode);
        let result = fast_runner()
            .start(fixture_spec(mode, &directory.0))
            .await
            .unwrap()
            .wait()
            .await
            .unwrap();
        assert_eq!(result.outcome, ProcessOutcome::OutputLimit, "{mode}");
        assert!(result.output_truncated, "{mode}");
        assert!(
            result.stdout.len() + result.stderr.len() <= MAX_RETURNED_OUTPUT_BYTES,
            "{mode}"
        );
        assert_processes_gone(&recorded_pids(&directory.0.join("pids"), 1));
    }
}

#[tokio::test]
async fn finite_stdout_and_stderr_are_returned_with_a_minimal_environment() {
    let directory = TestDir::new("output");
    let result = fast_runner()
        .start(fixture_spec("output", &directory.0))
        .await
        .unwrap()
        .wait()
        .await
        .unwrap();

    assert_eq!(
        result.outcome,
        ProcessOutcome::Exited {
            code: Some(7),
            signal: None
        }
    );
    assert!(String::from_utf8_lossy(&result.stdout).contains("stdout:path-cleared=true"));
    assert!(String::from_utf8_lossy(&result.stderr).contains("stderr"));
    assert!(!result.output_truncated);
}

#[tokio::test]
async fn spawn_failure_and_shutdown_cleanup_do_not_hold_the_run_lock() {
    use std::os::unix::fs::PermissionsExt;

    let directory = TestDir::new("spawn-shutdown");
    let invalid = directory.0.join("invalid-executable");
    fs::write(&invalid, b"#!/definitely/missing/u3-interpreter\n").unwrap();
    fs::set_permissions(&invalid, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(matches!(
        fast_runner()
            .start(ProcessSpec::new(&invalid, &directory.0))
            .await,
        Err(StartError::Spawn(_))
    ));

    let runner = fast_runner();
    let started = runner
        .start(fixture_spec("ordinary", &directory.0))
        .await
        .unwrap();
    let pids = recorded_pids(&directory.0.join("pids"), 2);
    runner.shutdown().await;
    assert_eq!(
        started.wait().await.unwrap().outcome,
        ProcessOutcome::Cancelled
    );
    assert_processes_gone(&pids);
    runner
        .start(fixture_spec("exit", &directory.0))
        .await
        .unwrap()
        .wait()
        .await
        .unwrap();
}

#[tokio::test]
async fn setsid_escape_is_characterized_outside_the_group_guarantee() {
    let directory = TestDir::new("setsid-escape");
    let runner = fast_runner();
    let result = runner
        .start(fixture_spec("escape-parent", &directory.0))
        .await
        .unwrap()
        .wait()
        .await
        .unwrap();
    let pids = recorded_pids(&directory.0.join("pids"), 2);
    let escaped = *pids.last().unwrap();

    assert!(matches!(
        result.outcome,
        ProcessOutcome::Exited { code: Some(0), .. }
    ));
    assert!(
        process_exists(escaped),
        "setsid escape unexpectedly remained in the group"
    );
    runner
        .start(fixture_spec("exit", &directory.0))
        .await
        .unwrap()
        .wait()
        .await
        .unwrap();

    unsafe { libc::kill(escaped, libc::SIGKILL) };
    assert_processes_gone(&[escaped]);
}

fn record_pid(path: &Path) {
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .unwrap();
    writeln!(file, "{}", std::process::id()).unwrap();
    file.flush().unwrap();
}

fn spawn_fixture(mode: &str, pid_file: &Path) -> std::process::Child {
    Command::new(env::current_exe().unwrap())
        .args(["--ignored", "--exact", "process_fixture", "--nocapture"])
        .env("U3_PROCESS_FIXTURE", mode)
        .env("U3_PID_FILE", pid_file)
        .spawn()
        .unwrap()
}

fn write_forever(mut output: impl Write, bytes: &[u8]) -> ! {
    loop {
        if output.write_all(bytes).is_err() {
            loop {
                thread::sleep(Duration::from_secs(1));
            }
        }
    }
}

#[test]
#[ignore]
#[allow(clippy::zombie_processes)] // These branches deliberately exercise parent-early-exit cleanup.
fn process_fixture() {
    let mode = env::var("U3_PROCESS_FIXTURE").unwrap();
    let pid_file = PathBuf::from(env::var_os("U3_PID_FILE").unwrap());
    record_pid(&pid_file);

    match mode.as_str() {
        "ordinary" => {
            let mut child = spawn_fixture("grandchild", &pid_file);
            let _ = child.wait();
        }
        "grandchild" => loop {
            thread::sleep(Duration::from_secs(1));
        },
        "ignore-term" => {
            unsafe { libc::signal(libc::SIGTERM, libc::SIG_IGN) };
            let mut child = spawn_fixture("ignore-grandchild", &pid_file);
            let _ = child.wait();
        }
        "ignore-grandchild" => {
            unsafe { libc::signal(libc::SIGTERM, libc::SIG_IGN) };
            loop {
                thread::sleep(Duration::from_secs(1));
            }
        }
        "early-exit" => {
            spawn_fixture("grandchild", &pid_file);
            thread::sleep(Duration::from_millis(50));
        }
        "brief" => thread::sleep(Duration::from_millis(20)),
        "flood" => {
            thread::spawn(|| write_forever(io::stderr().lock(), b"stderr flood\n"));
            write_forever(io::stdout().lock(), b"stdout flood\n");
        }
        "no-newline-stdout" => write_forever(io::stdout().lock(), &[b'x'; 8192]),
        "no-newline-stderr" => write_forever(io::stderr().lock(), &[b'x'; 8192]),
        "finite-record" => {
            let mut stdout = io::stdout().lock();
            for _ in 0..130 {
                stdout.write_all(&[b'x'; 8192]).unwrap();
            }
        }
        "finite-aggregate" => {
            let mut stdout = io::stdout().lock();
            for _ in 0..1152 {
                stdout.write_all(&[b'x'; 8191]).unwrap();
                stdout.write_all(b"\n").unwrap();
            }
        }
        "output" => {
            println!("stdout:path-cleared={}", env::var_os("PATH").is_none());
            eprintln!("stderr");
            std::process::exit(7);
        }
        "escape-parent" => {
            spawn_fixture("escape", &pid_file);
            thread::sleep(Duration::from_millis(50));
        }
        "escape" => {
            assert_ne!(unsafe { libc::setsid() }, -1);
            loop {
                thread::sleep(Duration::from_secs(1));
            }
        }
        "exit" => {}
        other => panic!("unknown fixture mode: {other}"),
    }
}
