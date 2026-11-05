//! Trusted, out-of-band Codex sign-in for a provider that is not ready to run work.
//!
//! The caller speaks a deliberately small JSONL protocol: stdout reports an
//! `auth_url` followed by `completed`, `cancelled`, or `error`; stdin accepts
//! `{ "command": "cancel" }`. Account details and credentials stay in Codex.

use std::{
    ffi::{OsStr, OsString},
    io::{self, BufRead, BufReader, Read, Write},
    os::unix::process::CommandExt as _,
    path::PathBuf,
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc::{self, Receiver, SyncSender},
    thread,
    time::{Duration, Instant},
};

use serde_json::{json, Value};

use super::{
    codex_executable, codex_readiness, codex_readiness_inherited_group, terminate_probe_group,
    write_codex_request,
};

const INITIALIZE_ID: &str = "cwr-login-initialize";
const BEFORE_ID: &str = "cwr-login-account-before";
const START_ID: &str = "cwr-login-start";
const AFTER_ID: &str = "cwr-login-account-after";
const CANCEL_ID: &str = "cwr-login-cancel";
const MAX_SERVER_LINE_BYTES: usize = 64 * 1024;
const MAX_CANCEL_LINE_BYTES: usize = 1024;
const MAX_QUEUED_EVENTS: usize = 32;
const MAX_SERVER_EVENTS: usize = 512;
const MAX_PENDING_COMPLETIONS: usize = 16;

/// Run the trusted Codex sign-in ceremony outside ordinary adapter invocation.
///
/// This starts `codex app-server --stdio` under the user's system Codex home.
/// The caller should open the emitted HTTPS `auth_url` in a browser and may send
/// `{ "command": "cancel" }` on `input`. Every expected outcome emits one
/// terminal JSONL event. An error event also returns an error so a CLI wrapper
/// can exit unsuccessfully without exposing provider output on stderr.
pub fn run_codex_login_repair<R: BufRead + Send + 'static, W: Write>(
    input: R,
    output: W,
) -> io::Result<()> {
    let options = LoginRepairOptions::from_host();
    run_with_options(input, output, options)
}

struct LoginRepairOptions {
    executable: PathBuf,
    home: OsString,
    user: Option<OsString>,
    path: OsString,
    codex_home: Option<PathBuf>,
    host_process_group: bool,
    request_timeout: Duration,
    login_timeout: Duration,
}

impl LoginRepairOptions {
    fn from_host() -> Result<Self, Failure> {
        let home = std::env::var_os("HOME")
            .filter(|value| !value.is_empty())
            .ok_or(Failure::HomeMissing)?;
        let path = std::env::var_os("PATH").unwrap_or_default();
        let executable = codex_executable(
            std::env::var_os("CWR_CODEX_EXECUTABLE"),
            Some(path.as_os_str()),
        )
        .ok_or(Failure::ExecutableMissing)?;
        Ok(Self {
            executable,
            home,
            user: std::env::var_os("USER").filter(|value| !value.is_empty()),
            path,
            codex_home: None,
            host_process_group: std::env::var_os("CWR_SETUP_HOST_PROCESS_GROUP").as_deref()
                == Some(OsStr::new("1")),
            request_timeout: Duration::from_secs(5),
            login_timeout: Duration::from_secs(5 * 60),
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Failure {
    HomeMissing,
    ExecutableMissing,
    StartFailed,
    Protocol,
    RequestTimeout,
    InvalidAuthUrl,
    LoginFailed,
    LoginTimeout,
    LoginUnverified,
    Cancelled,
}

impl Failure {
    fn code(self) -> &'static str {
        match self {
            Self::HomeMissing => "provider_home_missing",
            Self::ExecutableMissing => "provider_executable_missing",
            Self::StartFailed => "provider_start_failed",
            Self::Protocol => "provider_protocol_error",
            Self::RequestTimeout => "provider_request_timeout",
            Self::InvalidAuthUrl => "provider_auth_url_invalid",
            Self::LoginFailed => "provider_login_failed",
            Self::LoginTimeout => "provider_login_timeout",
            Self::LoginUnverified => "provider_login_unverified",
            Self::Cancelled => "provider_login_cancelled",
        }
    }

    fn message(self) -> &'static str {
        match self {
            Self::HomeMissing => "Codex needs a home directory before sign-in can start.",
            Self::ExecutableMissing => "Codex is missing or is not executable.",
            Self::StartFailed => "Workloop couldn't start Codex sign-in.",
            Self::Protocol | Self::RequestTimeout => "Codex didn't respond to the sign-in request.",
            Self::InvalidAuthUrl => "Codex returned an invalid sign-in URL.",
            Self::LoginFailed => "Codex sign-in did not complete. Please try again.",
            Self::LoginTimeout => "Codex sign-in timed out. Please try again.",
            Self::LoginUnverified => "Codex sign-in could not be verified. Please try again.",
            Self::Cancelled => "Codex sign-in was cancelled.",
        }
    }
}

enum RunError {
    Failure(Failure),
    Output(io::Error),
}

impl From<Failure> for RunError {
    fn from(value: Failure) -> Self {
        Self::Failure(value)
    }
}

#[derive(Clone, Copy)]
enum Outcome {
    Completed,
    Cancelled,
}

fn run_with_options<R: BufRead + Send + 'static, W: Write>(
    input: R,
    mut output: W,
    options: Result<LoginRepairOptions, Failure>,
) -> io::Result<()> {
    let result = options
        .map_err(RunError::Failure)
        .and_then(|options| run_inner(input, &mut output, &options));
    match result {
        Ok(Outcome::Completed) => write_event(&mut output, &json!({"event": "completed"})),
        Ok(Outcome::Cancelled) | Err(RunError::Failure(Failure::Cancelled)) => {
            write_event(&mut output, &json!({"event": "cancelled"}))
        }
        Err(RunError::Failure(failure)) => {
            write_event(
                &mut output,
                &json!({
                    "event": "error", "code": failure.code(), "message": failure.message()
                }),
            )?;
            Err(io::Error::other(failure.code()))
        }
        Err(RunError::Output(error)) => Err(error),
    }
}

fn run_inner<R: BufRead + Send + 'static, W: Write>(
    input: R,
    output: &mut W,
    options: &LoginRepairOptions,
) -> Result<Outcome, RunError> {
    let mut server = AppServer::start(input, options)?;
    server.send(&json!({
        "id": INITIALIZE_ID,
        "method": "initialize",
        "params": {"clientInfo": {
            "name": "cwr", "title": "CWR", "version": env!("CARGO_PKG_VERSION")
        }}
    }))?;
    server.receive_response(INITIALIZE_ID, options.request_timeout, true)?;
    server.send(&json!({"method": "initialized", "params": {}}))?;

    let before = read_account(&mut server, BEFORE_ID, options.request_timeout, true)?;
    if account_ready(&before)? {
        return if login_status_ready(options) {
            Ok(Outcome::Completed)
        } else {
            Err(Failure::LoginUnverified.into())
        };
    }

    server.send(&json!({
        "id": START_ID, "method": "account/login/start", "params": {"type": "chatgpt"}
    }))?;
    let start = server.receive_response(START_ID, options.request_timeout, true)?;
    let start = start.get("result").ok_or(Failure::Protocol)?;
    if start.get("type").and_then(Value::as_str) != Some("chatgpt") {
        return Err(Failure::Protocol.into());
    }
    let login_id = start
        .get("loginId")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .ok_or(Failure::Protocol)?;
    let auth_url = start
        .get("authUrl")
        .and_then(Value::as_str)
        .filter(|url| valid_auth_url(url))
        .ok_or(Failure::InvalidAuthUrl)?;
    write_event(output, &json!({"event": "auth_url", "auth_url": auth_url}))
        .map_err(RunError::Output)?;

    let deadline = Instant::now() + options.login_timeout;
    loop {
        let next = server
            .pending_completions
            .iter()
            .position(|message| is_completion(message, login_id))
            .map(|index| Ok(Event::Server(server.pending_completions.remove(index))))
            .unwrap_or_else(|| server.next_event(deadline, Failure::LoginTimeout));
        let event = match next {
            Ok(event) => event,
            Err(Failure::LoginTimeout) => {
                server.cancel_login(login_id);
                return Err(Failure::LoginTimeout.into());
            }
            Err(failure) => return Err(failure.into()),
        };
        match event {
            Event::Cancel => {
                server.cancel_login(login_id);
                return Ok(Outcome::Cancelled);
            }
            Event::Server(message) if is_completion(&message, login_id) => {
                if message["params"]["success"].as_bool() != Some(true) {
                    return Err(Failure::LoginFailed.into());
                }
                let after = read_account(&mut server, AFTER_ID, options.request_timeout, false)?;
                let login_ready = account_ready(&after)? && login_status_ready(options);
                return if login_ready {
                    Ok(Outcome::Completed)
                } else {
                    Err(Failure::LoginUnverified.into())
                };
            }
            Event::Server(_) => {}
        }
    }
}

fn login_status_ready(options: &LoginRepairOptions) -> bool {
    let check: fn(_, _, _, _, _, _) -> _ = if options.host_process_group {
        codex_readiness_inherited_group
    } else {
        codex_readiness
    };
    check(
        Some(options.home.clone()),
        options.user.clone(),
        Some(options.executable.clone().into_os_string()),
        Some(options.path.clone()),
        options.codex_home.as_deref(),
        options.request_timeout,
    )
    .is_ready()
}

fn read_account(
    server: &mut AppServer,
    id: &str,
    timeout: Duration,
    honor_cancel: bool,
) -> Result<Value, Failure> {
    server.send(&json!({
        "id": id, "method": "account/read", "params": {"refreshToken": false}
    }))?;
    let response = server.receive_response(id, timeout, honor_cancel)?;
    response.get("result").cloned().ok_or(Failure::Protocol)
}

fn account_ready(result: &Value) -> Result<bool, Failure> {
    let account = result.get("account").ok_or(Failure::Protocol)?;
    let requires_auth = result
        .get("requiresOpenaiAuth")
        .and_then(Value::as_bool)
        .ok_or(Failure::Protocol)?;
    Ok(account.is_object() || !requires_auth)
}

fn valid_auth_url(url: &str) -> bool {
    if url.len() > 4096 {
        return false;
    }
    reqwest::Url::parse(url).is_ok_and(|parsed| {
        parsed.scheme() == "https"
            && parsed.host_str().is_some()
            && parsed.username().is_empty()
            && parsed.password().is_none()
    })
}

fn is_completion(message: &Value, login_id: &str) -> bool {
    message.get("method").and_then(Value::as_str) == Some("account/login/completed")
        && message["params"]["loginId"].as_str() == Some(login_id)
}

fn write_event(output: &mut impl Write, event: &Value) -> io::Result<()> {
    serde_json::to_writer(&mut *output, event)?;
    output.write_all(b"\n")?;
    output.flush()
}

enum WireEvent {
    ServerLine(io::Result<String>),
    ServerClosed,
    Cancel,
}
enum Event {
    Server(Value),
    Cancel,
}

struct AppServer {
    child: Child,
    stdin: ChildStdin,
    events: Receiver<WireEvent>,
    host_process_group: bool,
    server_events_seen: usize,
    pending_completions: Vec<Value>,
}

impl AppServer {
    fn start<R: BufRead + Send + 'static>(
        input: R,
        options: &LoginRepairOptions,
    ) -> Result<Self, Failure> {
        let mut command = Command::new(&options.executable);
        command
            .env_clear()
            .env("HOME", &options.home)
            .env("PATH", &options.path)
            .args(["app-server", "--stdio"])
            .current_dir(&options.home)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        if let Some(codex_home) = &options.codex_home {
            command.env("CODEX_HOME", codex_home);
        }
        if let Some(user) = &options.user {
            command.env("USER", user);
        }
        // Desktop starts the CWR setup CLI in a detached process group and
        // owns forced cleanup. A standalone CLI invocation owns a separate
        // provider group so normal teardown also reaches provider children.
        if !options.host_process_group {
            command.process_group(0);
        }
        let mut child = command.spawn().map_err(|_| Failure::StartFailed)?;
        let Some(stdin) = child.stdin.take() else {
            terminate_app_server(&mut child, options.host_process_group);
            return Err(Failure::StartFailed);
        };
        let Some(stdout) = child.stdout.take() else {
            terminate_app_server(&mut child, options.host_process_group);
            return Err(Failure::StartFailed);
        };
        let (sender, events) = mpsc::sync_channel(MAX_QUEUED_EVENTS);
        spawn_server_reader(stdout, sender.clone());
        spawn_cancel_reader(input, sender);
        Ok(Self {
            child,
            stdin,
            events,
            host_process_group: options.host_process_group,
            server_events_seen: 0,
            pending_completions: Vec::new(),
        })
    }

    fn send(&mut self, message: &Value) -> Result<(), Failure> {
        write_codex_request(&mut self.stdin, message).map_err(|_| Failure::Protocol)
    }

    fn next_event(&mut self, deadline: Instant, timeout: Failure) -> Result<Event, Failure> {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or(timeout)?;
        match self.events.recv_timeout(remaining) {
            Ok(WireEvent::ServerLine(Ok(line))) => {
                self.server_events_seen += 1;
                if self.server_events_seen > MAX_SERVER_EVENTS {
                    return Err(Failure::Protocol);
                }
                serde_json::from_str(&line)
                    .map(Event::Server)
                    .map_err(|_| Failure::Protocol)
            }
            Ok(WireEvent::Cancel) => Ok(Event::Cancel),
            Ok(WireEvent::ServerLine(Err(_)) | WireEvent::ServerClosed) => Err(Failure::Protocol),
            Err(mpsc::RecvTimeoutError::Timeout) => Err(timeout),
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(Failure::Protocol),
        }
    }

    fn receive_response(
        &mut self,
        id: &str,
        timeout: Duration,
        honor_cancel: bool,
    ) -> Result<Value, Failure> {
        let deadline = Instant::now() + timeout;
        loop {
            match self.next_event(deadline, Failure::RequestTimeout)? {
                Event::Cancel if honor_cancel => return Err(Failure::Cancelled),
                Event::Cancel => {}
                Event::Server(message) if message.get("id").and_then(Value::as_str) == Some(id) => {
                    if message.get("error").is_some() || message.get("result").is_none() {
                        return Err(Failure::Protocol);
                    }
                    return Ok(message);
                }
                Event::Server(message)
                    if message.get("method").and_then(Value::as_str)
                        == Some("account/login/completed") =>
                {
                    if self.pending_completions.len() >= MAX_PENDING_COMPLETIONS {
                        return Err(Failure::Protocol);
                    }
                    self.pending_completions.push(message)
                }
                Event::Server(_) => {}
            }
        }
    }

    fn cancel_login(&mut self, login_id: &str) {
        if self
            .send(&json!({
                "id": CANCEL_ID, "method": "account/login/cancel", "params": {"loginId": login_id}
            }))
            .is_ok()
        {
            let _ = self.receive_response(CANCEL_ID, Duration::from_millis(250), false);
        }
    }
}

impl Drop for AppServer {
    fn drop(&mut self) {
        terminate_app_server(&mut self.child, self.host_process_group);
    }
}

fn terminate_app_server(child: &mut Child, host_process_group: bool) {
    if host_process_group {
        // This child shares the CWR CLI's group. Killing that group here
        // would also kill the CLI before its terminal setup event.
        let _ = child.kill();
        let _ = child.wait();
    } else {
        terminate_probe_group(child);
    }
}

fn spawn_server_reader(stdout: impl io::Read + Send + 'static, sender: SyncSender<WireEvent>) {
    thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        loop {
            match read_line_bounded(&mut reader, MAX_SERVER_LINE_BYTES) {
                Ok(Some(line)) => {
                    if sender.send(WireEvent::ServerLine(Ok(line))).is_err() {
                        return;
                    }
                }
                Ok(None) => {
                    let _ = sender.send(WireEvent::ServerClosed);
                    return;
                }
                Err(_) => {
                    let _ = sender.send(WireEvent::ServerLine(Err(io::Error::from(
                        io::ErrorKind::InvalidData,
                    ))));
                    return;
                }
            }
        }
    });
}

fn spawn_cancel_reader<R: BufRead + Send + 'static>(mut input: R, sender: SyncSender<WireEvent>) {
    thread::spawn(move || {
        while let Ok(Some(line)) = read_line_bounded(&mut input, MAX_CANCEL_LINE_BYTES) {
            if serde_json::from_str::<Value>(&line)
                .ok()
                .as_ref()
                .and_then(|value| value.get("command"))
                .and_then(Value::as_str)
                == Some("cancel")
            {
                let _ = sender.send(WireEvent::Cancel);
                return;
            }
        }
    });
}

fn read_line_bounded(reader: &mut impl BufRead, max_bytes: usize) -> io::Result<Option<String>> {
    let mut bytes = Vec::new();
    let read = reader
        .take((max_bytes + 1) as u64)
        .read_until(b'\n', &mut bytes)?;
    if read == 0 {
        return Ok(None);
    }
    if bytes.len() > max_bytes {
        return Err(io::Error::from(io::ErrorKind::InvalidData));
    }
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|_| io::Error::from(io::ErrorKind::InvalidData))
}

#[cfg(test)]
mod tests;
