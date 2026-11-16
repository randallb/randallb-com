use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(not(test))]
use std::sync::atomic::AtomicU8;

const DEFAULT_SHELL_COMMAND_TIMEOUT_MS: u64 = 10 * 60 * 1_000;
const MAX_SHELL_COMMAND_TIMEOUT_MS: u64 = 60 * 60 * 1_000;
const SHELL_COMMAND_IDLE: u8 = 0;
const SHELL_COMMAND_RUNNING: u8 = 1;
const SHELL_COMMAND_COMPLETING: u8 = 2;
const SHELL_COMMAND_STOPPING: u8 = 3;
#[cfg(not(test))]
static SHELL_COMMAND_STATE: AtomicU8 = AtomicU8::new(SHELL_COMMAND_IDLE);
#[cfg(test)]
type ShellTerminateOverride = fn(u32) -> CwrResult<()>;
#[cfg(test)]
thread_local! {
    static SHELL_COMMAND_TEST_STATE: std::cell::Cell<u8> = const { std::cell::Cell::new(SHELL_COMMAND_IDLE) };
    static SHELL_BEFORE_LIFECYCLE_HOOK: std::cell::Cell<Option<fn()>> = const { std::cell::Cell::new(None) };
    static SHELL_TERMINATE_OVERRIDE: std::cell::Cell<Option<ShellTerminateOverride>> = const { std::cell::Cell::new(None) };
}

struct ShellCommandWait {
    status: std::process::ExitStatus,
    timed_out_after_ms: Option<u64>,
}

struct ShellCommandDeadline {
    cancel: std::sync::mpsc::Sender<()>,
    state: std::sync::Arc<std::sync::Mutex<ShellCommandDeadlineState>>,
    timeout: Duration,
    worker: Option<thread::JoinHandle<()>>,
}

struct ShellCommandDeadlineState {
    process_group: i32,
    active: bool,
    timed_out: bool,
    failure: Option<String>,
}

enum ShellCommandStatus {
    Running,
    Stopped,
    DeadlineFailed(String),
    Exited(std::process::ExitStatus),
}

#[cfg(not(test))]
fn shell_command_state() -> u8 {
    SHELL_COMMAND_STATE.load(Ordering::Acquire)
}

#[cfg(test)]
fn shell_command_state() -> u8 {
    SHELL_COMMAND_TEST_STATE.get()
}

#[cfg(not(test))]
fn claim_shell_command(from: u8, to: u8) -> bool {
    SHELL_COMMAND_STATE
        .compare_exchange(from, to, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
}

#[cfg(test)]
fn claim_shell_command(from: u8, to: u8) -> bool {
    SHELL_COMMAND_TEST_STATE.with(|state| {
        if state.get() == from {
            state.set(to);
            true
        } else {
            false
        }
    })
}

#[cfg(not(test))]
fn request_shell_command_stop() {
    let _ = SHELL_COMMAND_STATE.fetch_update(Ordering::AcqRel, Ordering::Acquire, |state| {
        matches!(state, SHELL_COMMAND_IDLE | SHELL_COMMAND_RUNNING)
            .then_some(SHELL_COMMAND_STOPPING)
    });
}

#[cfg(test)]
fn request_shell_command_stop() {
    SHELL_COMMAND_TEST_STATE.with(|state| {
        if matches!(state.get(), SHELL_COMMAND_IDLE | SHELL_COMMAND_RUNNING) {
            state.set(SHELL_COMMAND_STOPPING);
        }
    });
}

#[cfg(test)]
fn reset_shell_command_state() {
    SHELL_COMMAND_TEST_STATE.set(SHELL_COMMAND_IDLE);
    SHELL_BEFORE_LIFECYCLE_HOOK.set(None);
    SHELL_TERMINATE_OVERRIDE.set(None);
}

#[cfg(not(test))]
fn before_shell_lifecycle_registration() {}

#[cfg(test)]
fn before_shell_lifecycle_registration() {
    SHELL_BEFORE_LIFECYCLE_HOOK.with(|hook| {
        if let Some(hook) = hook.take() {
            hook();
        }
    });
}

#[cfg(not(test))]
fn terminate_registered_shell_process_group(command_pid: u32) -> CwrResult<()> {
    terminate_shell_process_group(command_pid)
}

#[cfg(test)]
fn terminate_registered_shell_process_group(command_pid: u32) -> CwrResult<()> {
    SHELL_TERMINATE_OVERRIDE.with(|terminate| {
        terminate
            .get()
            .unwrap_or(terminate_shell_process_group)(command_pid)
    })
}

struct ShellCommandLifecycle;

impl ShellCommandLifecycle {
    fn begin() -> CwrResult<Self> {
        if !claim_shell_command(SHELL_COMMAND_IDLE, SHELL_COMMAND_RUNNING) {
            return protocol_failure("Shell command stopped");
        }
        Ok(Self)
    }
}

impl Drop for ShellCommandLifecycle {
    fn drop(&mut self) {
        let state = shell_command_state();
        if matches!(state, SHELL_COMMAND_RUNNING | SHELL_COMMAND_COMPLETING) {
            let _ = claim_shell_command(state, SHELL_COMMAND_IDLE);
        }
    }
}

fn shell_is_stopping() -> bool {
    shell_command_state() == SHELL_COMMAND_STOPPING
}

impl ShellCommandDeadline {
    fn start(command_pid: u32, timeout: Duration) -> Self {
        Self::start_with_signal(command_pid, timeout, signal_timed_out_process_group)
    }

    fn start_with_signal(
        command_pid: u32,
        timeout: Duration,
        signal: fn(i32) -> std::io::Result<()>,
    ) -> Self {
        let (cancel, wait) = std::sync::mpsc::channel();
        let state = std::sync::Arc::new(std::sync::Mutex::new(ShellCommandDeadlineState {
            process_group: command_pid as i32,
            active: true,
            timed_out: false,
            failure: None,
        }));
        let worker_state = state.clone();
        let worker = thread::spawn(move || {
            if matches!(
                wait.recv_timeout(timeout),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout)
            ) {
                let mut state = worker_state.lock().unwrap();
                if state.active {
                    // Holding the shared state lock keeps every wait/try_wait
                    // path from reaping the child until this signal attempt is
                    // complete, so the process-group ID cannot be reused.
                    if let Err(error) = signal(state.process_group) {
                        state.failure = Some(format!(
                            "could not terminate timed out shell command: {error}"
                        ));
                    } else {
                        state.timed_out = true;
                    }
                }
            }
        });
        Self {
            cancel,
            state,
            timeout,
            worker: Some(worker),
        }
    }

    fn expired(&self) -> bool {
        self.state.lock().unwrap().timed_out
    }

    fn try_wait(
        &self,
        child: &mut std::process::Child,
    ) -> std::io::Result<ShellCommandStatus> {
        self.try_wait_before_completion(child, || {})
    }

    fn try_wait_before_completion(
        &self,
        child: &mut std::process::Child,
        before_completion: impl FnOnce(),
    ) -> std::io::Result<ShellCommandStatus> {
        let mut state = self.state.lock().unwrap();
        if !child_has_exited(child.id())? {
            return Ok(ShellCommandStatus::Running);
        }
        before_completion();
        if !claim_shell_command(SHELL_COMMAND_RUNNING, SHELL_COMMAND_COMPLETING) {
            return Ok(ShellCommandStatus::Stopped);
        }
        if let Some(failure) = &state.failure {
            return Ok(ShellCommandStatus::DeadlineFailed(failure.clone()));
        }
        state.active = false;
        child.wait().map(ShellCommandStatus::Exited)
    }

    fn join_worker(&mut self) -> CwrResult<()> {
        if let Some(worker) = self.worker.take() {
            worker
                .join()
                .map_err(|_| protocol_error("shell command deadline worker panicked"))?;
        }
        let state = self.state.lock().unwrap();
        if let Some(failure) = &state.failure {
            return protocol_failure(failure.clone());
        }
        Ok(())
    }

    fn cancel_and_join(&mut self) -> CwrResult<()> {
        let _ = self.cancel.send(());
        self.join_worker()
    }
}

fn signal_timed_out_process_group(process_group: i32) -> std::io::Result<()> {
    // SAFETY: the negative child PID intentionally addresses the command's
    // process group while the deadline state still owns its unreaped leader.
    let result = unsafe { libc::kill(-process_group, libc::SIGKILL) };
    let error = std::io::Error::last_os_error();
    if result == 0 || error.raw_os_error() == Some(libc::ESRCH) {
        Ok(())
    } else {
        Err(error)
    }
}

impl Drop for ShellCommandDeadline {
    fn drop(&mut self) {
        let _ = self.cancel_and_join();
    }
}

fn shell_command_timeout(timeout_ms: Option<u64>) -> CwrResult<Duration> {
    let timeout_ms = timeout_ms.unwrap_or(DEFAULT_SHELL_COMMAND_TIMEOUT_MS);
    if !(1..=MAX_SHELL_COMMAND_TIMEOUT_MS).contains(&timeout_ms) {
        return protocol_failure(format!(
            "Shell command timeout_ms must be between 1 and {MAX_SHELL_COMMAND_TIMEOUT_MS}"
        ));
    }
    Ok(Duration::from_millis(timeout_ms))
}

fn wait_for_shell_command(
    child: &mut std::process::Child,
    output_readers: (&ShellOutputCapture, &ShellOutputCapture),
    deadline: &mut ShellCommandDeadline,
    stream_output: bool,
    progress: &std::sync::mpsc::Receiver<ShellProgress>,
    coworker_bridge: Option<&mut ShellCoworkerBridge>,
    events: &mut ShellAdapterEvents<'_, impl Write>,
) -> CwrResult<ShellCommandWait> {
    let (stdout_reader, stderr_reader) = output_readers;
    let timeout = deadline.timeout;
    let started = Instant::now();
    let mut next_report = Duration::from_secs(30);
    let mut coworker_bridge = coworker_bridge;
    loop {
        if shell_is_stopping() {
            return protocol_failure("Shell command stopped");
        }
        if deadline.expired() {
            deadline.join_worker()?;
            return finish_timed_out_shell_command(
                child,
                output_readers,
                timeout,
            );
        }
        drain_shell_progress(progress, events)?;
        if let Some(bridge) = coworker_bridge.as_deref_mut() {
            bridge.poll(events, started + timeout)?;
        }
        if deadline.expired() {
            deadline.join_worker()?;
            return finish_timed_out_shell_command(
                child,
                output_readers,
                timeout,
            );
        }
        if stdout_reader.finished.load(Ordering::Acquire)
            && stderr_reader.finished.load(Ordering::Acquire)
        {
            match deadline
                .try_wait(child)
                .map_err(|error| {
                protocol_error(format!("could not inspect shell command: {error}"))
            })? {
                ShellCommandStatus::Stopped => {
                    return protocol_failure("Shell command stopped");
                }
                ShellCommandStatus::DeadlineFailed(failure) => {
                    return protocol_failure(failure);
                }
                ShellCommandStatus::Exited(status) => {
                    if deadline.expired() {
                        deadline.join_worker()?;
                        return finish_timed_out_shell_command(child, output_readers, timeout);
                    }
                    deadline.cancel_and_join()?;
                    return Ok(ShellCommandWait {
                        status,
                        timed_out_after_ms: None,
                    });
                }
                ShellCommandStatus::Running => {}
            }
        }
        let elapsed = started.elapsed();
        if elapsed >= timeout {
            // The deadline worker is the sole owner of timeout signaling.
            // Joining it before reaping prevents a late signal from ever
            // targeting a process group whose ID has been reused.
            deadline.join_worker()?;
            return finish_timed_out_shell_command(
                child,
                output_readers,
                timeout,
            );
        }
        if stream_output && elapsed >= next_report {
            eprintln!(
                "\ncwr-adapter-sh: command still running command_pid={} elapsed_seconds={}",
                child.id(),
                elapsed.as_secs()
            );
            next_report += Duration::from_secs(30);
        }
        thread::sleep(
            timeout
                .saturating_sub(elapsed)
                .min(Duration::from_millis(100)),
        );
    }
}

fn finish_timed_out_shell_command(
    child: &mut std::process::Child,
    output_readers: (&ShellOutputCapture, &ShellOutputCapture),
    timeout: Duration,
) -> CwrResult<ShellCommandWait> {
    let status = child.wait().map_err(|error| {
        protocol_error(format!("could not reap timed out shell command: {error}"))
    })?;
    let drain_deadline = Instant::now() + Duration::from_millis(100);
    while (!output_readers.0.finished.load(Ordering::Acquire)
        || !output_readers.1.finished.load(Ordering::Acquire))
        && Instant::now() < drain_deadline
    {
        thread::sleep(Duration::from_millis(5));
    }
    Ok(ShellCommandWait {
        status,
        timed_out_after_ms: Some(timeout.as_millis() as u64),
    })
}

fn child_has_exited(pid: u32) -> std::io::Result<bool> {
    // waitid with WNOWAIT observes a waitable child without reaping it, so the
    // deadline worker and the child-owning thread agree on when the PID stops
    // being a valid process-group target.
    let mut info = std::mem::MaybeUninit::<libc::siginfo_t>::zeroed();
    // SAFETY: info points to writable siginfo storage, the PID belongs to the
    // child owned by this thread, and WNOWAIT leaves that child unreaped.
    let result = unsafe {
        libc::waitid(
            libc::P_PID,
            pid as libc::id_t,
            info.as_mut_ptr(),
            libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
        )
    };
    if result != 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: successful waitid initialized the siginfo storage.
    let info = unsafe { info.assume_init() };
    // SAFETY: waitid initialized siginfo with a child-status result.
    Ok(unsafe { info.si_pid() } != 0)
}

extern "C" fn request_shell_stop(_: libc::c_int) {
    request_shell_command_stop();
}

pub fn install_shell_stop_handler() -> CwrResult<()> {
    // SAFETY: the handler only stores to a lock-free atomic. The child-owning
    // thread observes it and terminates the process group before reaping.
    // This runs in the dedicated adapter executable before reader threads start.
    if unsafe {
        libc::signal(
            libc::SIGTERM,
            request_shell_stop as *const () as libc::sighandler_t,
        )
    } == libc::SIG_ERR
    {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}

fn terminate_shell_process_group(command_pid: u32) -> CwrResult<()> {
    // SAFETY: the command is started as leader of its own process group. The
    // adapter retains its Child while addressing that group, and a surviving
    // descendant keeps the process-group ID reserved after the leader exits.
    let result = unsafe { libc::kill(-(command_pid as i32), libc::SIGKILL) };
    if result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
        Ok(())
    } else {
        protocol_failure(format!(
            "could not terminate timed out shell command: {}",
            std::io::Error::last_os_error()
        ))
    }
}
