use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    fmt, fs, io,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::{Child, Command},
    sync::{mpsc, oneshot, Mutex},
    task::JoinHandle,
    time::{sleep, timeout},
};

pub const MAX_OUTPUT_RECORD_BYTES: usize = 1024 * 1024;
pub const MAX_RAW_OUTPUT_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_RETURNED_OUTPUT_BYTES: usize = 8 * 1024 * 1024;
pub const DEFAULT_DEADLINE: Duration = Duration::from_secs(60);
const OUTPUT_SHUTDOWN_GRACE: Duration = Duration::from_millis(250);
const GROUP_POLL_INTERVAL: Duration = Duration::from_millis(10);
static NEXT_RUN_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct RunId(u64);

#[derive(Clone, Default)]
pub struct CancellationToken(Arc<AtomicBool>);

impl CancellationToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

impl RunId {
    pub fn get(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Debug)]
pub struct ProcessSpec {
    executable: PathBuf,
    cwd: PathBuf,
    arguments: Vec<OsString>,
    environment: BTreeMap<OsString, OsString>,
    deadline: Option<Duration>,
}

impl ProcessSpec {
    pub fn new(executable: impl Into<PathBuf>, cwd: impl Into<PathBuf>) -> Self {
        Self {
            executable: executable.into(),
            cwd: cwd.into(),
            arguments: Vec::new(),
            environment: BTreeMap::new(),
            deadline: None,
        }
    }

    pub fn arg(mut self, argument: impl AsRef<OsStr>) -> Self {
        self.arguments.push(argument.as_ref().to_owned());
        self
    }

    pub fn args<I, S>(mut self, arguments: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.arguments.extend(
            arguments
                .into_iter()
                .map(|argument| argument.as_ref().to_owned()),
        );
        self
    }

    pub fn env(mut self, key: impl AsRef<OsStr>, value: impl AsRef<OsStr>) -> Self {
        self.environment
            .insert(key.as_ref().to_owned(), value.as_ref().to_owned());
        self
    }

    pub fn deadline(mut self, deadline: Duration) -> Self {
        self.deadline = Some(deadline);
        self
    }
}

#[derive(Clone, Debug)]
pub struct ProcessConfig {
    pub default_deadline: Duration,
    pub termination_grace: Duration,
}

impl Default for ProcessConfig {
    fn default() -> Self {
        Self {
            default_deadline: DEFAULT_DEADLINE,
            termination_grace: Duration::from_millis(250),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CancelResult {
    Requested,
    NotActive,
    IdMismatch,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProcessOutcome {
    Exited {
        code: Option<i32>,
        signal: Option<i32>,
    },
    Cancelled,
    TimedOut,
    OutputLimit,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcessResult {
    pub run_id: RunId,
    pub outcome: ProcessOutcome,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub output_truncated: bool,
}

#[derive(Debug, Eq, PartialEq)]
pub enum StartError {
    Busy { active_run_id: RunId },
    Cancelled,
    InvalidSpec(String),
    Spawn(String),
    Unsupported,
}

impl fmt::Display for StartError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Busy { active_run_id } => {
                write!(
                    formatter,
                    "process runner is busy with run {}",
                    active_run_id.get()
                )
            }
            Self::Cancelled => formatter.write_str("process start was cancelled"),
            Self::InvalidSpec(message) | Self::Spawn(message) => formatter.write_str(message),
            Self::Unsupported => formatter.write_str("process groups are supported on Unix only"),
        }
    }
}

impl std::error::Error for StartError {}

#[derive(Debug)]
pub struct WaitError;

impl fmt::Display for WaitError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("process manager stopped before returning a result")
    }
}

impl std::error::Error for WaitError {}

#[derive(Clone, Copy)]
enum Control {
    Cancel,
    Shutdown,
}

struct Active {
    id: RunId,
    cancellation: CancellationToken,
    control: mpsc::Sender<Control>,
}

struct RunnerInner {
    active: Mutex<Option<Active>>,
    config: ProcessConfig,
}

#[derive(Clone)]
pub struct ProcessRunner {
    inner: Arc<RunnerInner>,
}

pub struct StartedRun {
    id: RunId,
    result: oneshot::Receiver<ProcessResult>,
}

impl StartedRun {
    pub fn id(&self) -> RunId {
        self.id
    }

    pub async fn wait(self) -> Result<ProcessResult, WaitError> {
        self.result.await.map_err(|_| WaitError)
    }
}

impl Default for ProcessRunner {
    fn default() -> Self {
        Self::new()
    }
}

impl ProcessRunner {
    pub fn new() -> Self {
        Self::with_config(ProcessConfig::default())
    }

    pub fn with_config(config: ProcessConfig) -> Self {
        Self {
            inner: Arc::new(RunnerInner {
                active: Mutex::new(None),
                config,
            }),
        }
    }

    pub async fn start(&self, spec: ProcessSpec) -> Result<StartedRun, StartError> {
        self.start_cancellable(spec, CancellationToken::new()).await
    }

    pub async fn start_cancellable(
        &self,
        spec: ProcessSpec,
        cancellation: CancellationToken,
    ) -> Result<StartedRun, StartError> {
        let mut active = self.inner.active.lock().await;
        if cancellation.is_cancelled() {
            return Err(StartError::Cancelled);
        }
        if let Some(active) = active.as_ref() {
            return Err(StartError::Busy {
                active_run_id: active.id,
            });
        }

        #[cfg(not(unix))]
        {
            let _ = spec;
            return Err(StartError::Unsupported);
        }

        #[cfg(unix)]
        {
            let executable = canonical_executable(&spec.executable)?;
            let cwd = canonical_directory(&spec.cwd)?;
            let deadline = spec.deadline.unwrap_or(self.inner.config.default_deadline);
            let mut command = Command::new(executable);
            use std::os::unix::process::CommandExt;
            command.as_std_mut().arg0(&spec.executable);
            command
                .args(spec.arguments)
                .current_dir(cwd)
                .env_clear()
                .envs(spec.environment)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            set_process_group(&mut command);
            let mut child = command
                .spawn()
                .map_err(|error| StartError::Spawn(format!("failed to spawn process: {error}")))?;
            let pid = child
                .id()
                .ok_or_else(|| StartError::Spawn("spawned process has no PID".into()))?;
            let pgid = i32::try_from(pid)
                .map_err(|_| StartError::Spawn("spawned PID does not fit pid_t".into()))?;
            let stdout = child
                .stdout
                .take()
                .ok_or_else(|| StartError::Spawn("spawned process has no stdout pipe".into()))?;
            let stderr = child
                .stderr
                .take()
                .ok_or_else(|| StartError::Spawn("spawned process has no stderr pipe".into()))?;
            let id = next_run_id();
            let (control_sender, control_receiver) = mpsc::channel(1);
            let (result_sender, result_receiver) = oneshot::channel();
            *active = Some(Active {
                id,
                cancellation,
                control: control_sender,
            });
            let inner = Arc::clone(&self.inner);
            tokio::spawn(async move {
                let result = manage_process(
                    id,
                    pgid,
                    child,
                    stdout,
                    stderr,
                    control_receiver,
                    deadline,
                    inner.config.termination_grace,
                )
                .await;
                release_run(&inner, id).await;
                let _ = result_sender.send(result);
            });
            Ok(StartedRun {
                id,
                result: result_receiver,
            })
        }
    }

    pub async fn cancel(&self, id: RunId) -> CancelResult {
        let active = self.inner.active.lock().await;
        let Some(active) = active.as_ref() else {
            return CancelResult::NotActive;
        };
        if active.id != id {
            return CancelResult::IdMismatch;
        }
        active.cancellation.cancel();
        let _ = active.control.try_send(Control::Cancel);
        CancelResult::Requested
    }

    pub async fn cancel_token(&self, cancellation: &CancellationToken) -> CancelResult {
        cancellation.cancel();
        let active = self.inner.active.lock().await;
        let Some(active) = active.as_ref() else {
            return CancelResult::NotActive;
        };
        if !Arc::ptr_eq(&active.cancellation.0, &cancellation.0) {
            return CancelResult::IdMismatch;
        }
        let _ = active.control.try_send(Control::Cancel);
        CancelResult::Requested
    }

    pub async fn shutdown(&self) {
        let mut requested = None;
        loop {
            let active_guard = self.inner.active.lock().await;
            let Some(active) = active_guard.as_ref() else {
                return;
            };
            if requested != Some(active.id) {
                let _ = active.control.try_send(Control::Shutdown);
                requested = Some(active.id);
            }
            drop(active_guard);
            sleep(GROUP_POLL_INTERVAL).await;
        }
    }
}

fn next_run_id() -> RunId {
    let id = NEXT_RUN_ID.fetch_add(1, Ordering::Relaxed);
    assert_ne!(id, 0, "process run ID space exhausted");
    RunId(id)
}

#[cfg(unix)]
fn set_process_group(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    command.as_std_mut().process_group(0);
}

pub(crate) fn canonical_executable(path: &Path) -> Result<PathBuf, StartError> {
    let canonical = fs::canonicalize(path).map_err(|error| {
        StartError::InvalidSpec(format!("invalid executable {}: {error}", path.display()))
    })?;
    let metadata = fs::metadata(&canonical).map_err(|error| {
        StartError::InvalidSpec(format!("invalid executable {}: {error}", path.display()))
    })?;
    if !metadata.is_file() {
        return Err(StartError::InvalidSpec(format!(
            "executable is not a regular file: {}",
            path.display()
        )));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return Err(StartError::InvalidSpec(format!(
                "file is not executable: {}",
                path.display()
            )));
        }
    }
    Ok(canonical)
}

fn canonical_directory(path: &PathBuf) -> Result<PathBuf, StartError> {
    let canonical = fs::canonicalize(path).map_err(|error| {
        StartError::InvalidSpec(format!(
            "invalid working directory {}: {error}",
            path.display()
        ))
    })?;
    if !fs::metadata(&canonical)
        .map_err(|error| StartError::InvalidSpec(error.to_string()))?
        .is_dir()
    {
        return Err(StartError::InvalidSpec(format!(
            "working directory is not a directory: {}",
            path.display()
        )));
    }
    Ok(canonical)
}

#[cfg(unix)]
#[allow(clippy::too_many_arguments)]
async fn manage_process<Out, Err>(
    run_id: RunId,
    pgid: i32,
    mut child: Child,
    stdout: Out,
    stderr: Err,
    mut control: mpsc::Receiver<Control>,
    deadline: Duration,
    termination_grace: Duration,
) -> ProcessResult
where
    Out: AsyncRead + Unpin + Send + 'static,
    Err: AsyncRead + Unpin + Send + 'static,
{
    let shared = Arc::new(OutputState::default());
    let (limit_sender, mut limit_receiver) = mpsc::unbounded_channel();
    let stdout_task = tokio::spawn(read_output(
        stdout,
        Arc::clone(&shared),
        limit_sender.clone(),
    ));
    let stderr_task = tokio::spawn(read_output(
        stderr,
        Arc::clone(&shared),
        limit_sender.clone(),
    ));
    let deadline_timer = sleep(deadline);
    tokio::pin!(deadline_timer);

    let event = tokio::select! {
        status = child.wait() => Event::Exited(status),
        control = control.recv() => match control {
            Some(Control::Cancel) => Event::Cancelled,
            Some(Control::Shutdown) | None => Event::Shutdown,
        },
        _ = &mut deadline_timer => Event::TimedOut,
        _ = limit_receiver.recv() => Event::OutputLimit,
    };

    let mut outcome = match event {
        Event::Exited(status) => {
            terminate_remaining_group(pgid, termination_grace).await;
            exit_outcome(status)
        }
        Event::Cancelled => {
            terminate_and_reap(&mut child, pgid, termination_grace).await;
            ProcessOutcome::Cancelled
        }
        Event::Shutdown => {
            terminate_and_reap(&mut child, pgid, termination_grace).await;
            ProcessOutcome::Cancelled
        }
        Event::TimedOut => {
            terminate_and_reap(&mut child, pgid, termination_grace).await;
            ProcessOutcome::TimedOut
        }
        Event::OutputLimit => {
            terminate_and_reap(&mut child, pgid, termination_grace).await;
            ProcessOutcome::OutputLimit
        }
    };

    let stdout = finish_output_task(stdout_task).await;
    let stderr = finish_output_task(stderr_task).await;
    if matches!(outcome, ProcessOutcome::Exited { .. })
        && shared.limit_exceeded.load(Ordering::Relaxed)
    {
        outcome = ProcessOutcome::OutputLimit;
    }
    ProcessResult {
        run_id,
        outcome,
        stdout,
        stderr,
        output_truncated: shared.truncated.load(Ordering::Relaxed),
    }
}

#[cfg(unix)]
enum Event {
    Exited(io::Result<std::process::ExitStatus>),
    Cancelled,
    Shutdown,
    TimedOut,
    OutputLimit,
}

#[cfg(unix)]
fn exit_outcome(status: io::Result<std::process::ExitStatus>) -> ProcessOutcome {
    use std::os::unix::process::ExitStatusExt;
    match status {
        Ok(status) => ProcessOutcome::Exited {
            code: status.code(),
            signal: status.signal(),
        },
        Err(_) => ProcessOutcome::Exited {
            code: None,
            signal: None,
        },
    }
}

#[derive(Default)]
struct OutputState {
    raw: AtomicUsize,
    captured: AtomicUsize,
    truncated: AtomicBool,
    limit_exceeded: AtomicBool,
}

async fn read_output<R>(
    mut stream: R,
    shared: Arc<OutputState>,
    limit: mpsc::UnboundedSender<()>,
) -> Vec<u8>
where
    R: AsyncRead + Unpin,
{
    let mut output = Vec::new();
    let mut buffer = [0_u8; 8192];
    let mut record_bytes = 0_usize;
    loop {
        let count = match stream.read(&mut buffer).await {
            Ok(0) | Err(_) => return output,
            Ok(count) => count,
        };
        let previous_raw = shared.raw.fetch_add(count, Ordering::Relaxed);
        if previous_raw > MAX_RAW_OUTPUT_BYTES.saturating_sub(count) {
            shared.truncated.store(true, Ordering::Relaxed);
            shared.limit_exceeded.store(true, Ordering::Relaxed);
            let _ = limit.send(());
            return output;
        }
        for byte in &buffer[..count] {
            if *byte == b'\n' {
                record_bytes = 0;
            } else {
                record_bytes += 1;
                if record_bytes > MAX_OUTPUT_RECORD_BYTES {
                    shared.truncated.store(true, Ordering::Relaxed);
                    shared.limit_exceeded.store(true, Ordering::Relaxed);
                    let _ = limit.send(());
                    return output;
                }
            }
        }
        let capture = reserve_capture(&shared.captured, count);
        output.extend_from_slice(&buffer[..capture]);
        if capture < count {
            shared.truncated.store(true, Ordering::Relaxed);
        }
    }
}

fn reserve_capture(captured: &AtomicUsize, requested: usize) -> usize {
    let mut current = captured.load(Ordering::Relaxed);
    loop {
        let reserved = requested.min(MAX_RETURNED_OUTPUT_BYTES.saturating_sub(current));
        match captured.compare_exchange_weak(
            current,
            current + reserved,
            Ordering::Relaxed,
            Ordering::Relaxed,
        ) {
            Ok(_) => return reserved,
            Err(actual) => current = actual,
        }
    }
}

async fn finish_output_task(mut task: JoinHandle<Vec<u8>>) -> Vec<u8> {
    match timeout(OUTPUT_SHUTDOWN_GRACE, &mut task).await {
        Ok(Ok(output)) => output,
        Ok(Err(_)) => Vec::new(),
        Err(_) => {
            task.abort();
            task.await.unwrap_or_default()
        }
    }
}

#[cfg(unix)]
async fn terminate_and_reap(child: &mut Child, pgid: i32, grace: Duration) {
    let started = Instant::now();
    signal_group(pgid, libc::SIGTERM);
    let _ = timeout(grace, child.wait()).await;
    wait_for_group_until(pgid, grace.saturating_sub(started.elapsed())).await;
    if group_exists(pgid) {
        signal_group(pgid, libc::SIGKILL);
    }
    let _ = child.wait().await;
    wait_for_group_absence(pgid).await;
}

#[cfg(unix)]
async fn terminate_remaining_group(pgid: i32, grace: Duration) {
    if !group_exists(pgid) {
        return;
    }
    signal_group(pgid, libc::SIGTERM);
    wait_for_group_until(pgid, grace).await;
    if group_exists(pgid) {
        signal_group(pgid, libc::SIGKILL);
        wait_for_group_absence(pgid).await;
    }
}

#[cfg(unix)]
async fn wait_for_group_until(pgid: i32, duration: Duration) {
    let deadline = Instant::now() + duration;
    while group_exists(pgid) && Instant::now() < deadline {
        sleep(GROUP_POLL_INTERVAL).await;
    }
}

#[cfg(unix)]
async fn wait_for_group_absence(pgid: i32) {
    while group_exists(pgid) {
        signal_group(pgid, libc::SIGKILL);
        sleep(GROUP_POLL_INTERVAL).await;
    }
}

#[cfg(unix)]
fn signal_group(pgid: i32, signal: i32) {
    unsafe {
        libc::kill(-pgid, signal);
    }
}

#[cfg(unix)]
fn group_exists(pgid: i32) -> bool {
    if unsafe { libc::kill(-pgid, 0) } == 0 {
        return true;
    }
    io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
}

async fn release_run(inner: &RunnerInner, id: RunId) {
    let mut active = inner.active.lock().await;
    if active.as_ref().is_some_and(|active| active.id == id) {
        *active = None;
    }
}
