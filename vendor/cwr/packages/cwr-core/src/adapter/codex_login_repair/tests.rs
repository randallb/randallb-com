use super::*;
use std::{
    fs,
    io::{BufReader, Cursor},
    os::unix::{fs::PermissionsExt as _, net::UnixStream},
    sync::{Arc, Mutex},
};

const SIGNED_OUT_START: &str = r#"#!/bin/sh
if [ "$1" = "login" ] && [ "$2" = "status" ]; then
  [ -f "$CODEX_HOME/signed-in" ] && exit 0
  exit 1
fi
[ "$1" = "app-server" ] && [ "$2" = "--stdio" ] || exit 20
[ "$CODEX_HOME" = "$HOME/managed" ] || exit 21
[ -z "$OPENAI_API_KEY" ] && [ -z "$CODEX_API_KEY" ] || exit 22
IFS= read -r initialize || exit 23
case "$initialize" in *'"method":"initialize"'*) ;; *) exit 24;; esac
printf '%s\n' '{"id":"cwr-login-initialize","result":{}}'
IFS= read -r initialized || exit 25
case "$initialized" in *'"method":"initialized"'*) ;; *) exit 26;; esac
IFS= read -r account_before || exit 27
case "$account_before" in *'"method":"account/read"'*) ;; *) exit 28;; esac
printf '%s\n' '{"id":"cwr-login-account-before","result":{"account":null,"requiresOpenaiAuth":true}}'
IFS= read -r start || exit 29
case "$start" in *'"method":"account/login/start"'*'"type":"chatgpt"'*) ;; *) exit 30;; esac
printf '%s\n' '{"id":"cwr-login-start","result":{"type":"chatgpt","loginId":"login-123","authUrl":"https://chatgpt.com/auth?redirect_uri=http%3A%2F%2Flocalhost%2Fcallback"}}'
"#;

fn fake_codex(body: &str) -> (tempfile::TempDir, LoginRepairOptions) {
    let mut script = String::from(SIGNED_OUT_START);
    script.push_str(body);
    fake_codex_script(&script)
}

fn fake_codex_script(script: &str) -> (tempfile::TempDir, LoginRepairOptions) {
    let temp = tempfile::tempdir().unwrap();
    let executable = temp.path().join("codex");
    fs::write(&executable, script).unwrap();
    let mut permissions = fs::metadata(&executable).unwrap().permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(&executable, permissions).unwrap();
    fs::create_dir(temp.path().join("managed")).unwrap();
    let options = LoginRepairOptions {
        executable,
        home: temp.path().as_os_str().to_owned(),
        user: Some(OsString::from("fake-user")),
        path: OsString::from("/usr/bin:/bin"),
        codex_home: Some(temp.path().join("managed")),
        host_process_group: true,
        request_timeout: Duration::from_secs(5),
        login_timeout: Duration::from_secs(5),
    };
    (temp, options)
}

fn events(output: &[u8]) -> Vec<Value> {
    String::from_utf8(output.to_vec())
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn process_group(pid: i32) -> i32 {
    // SAFETY: getpgid reads the process group of a valid or exited PID.
    unsafe { libc::getpgid(pid) }
}

fn current_process_group() -> i32 {
    // SAFETY: getpgrp reads the current process group and has no preconditions.
    unsafe { libc::getpgrp() }
}

fn process_exists(pid: i32) -> bool {
    // SAFETY: kill with signal 0 only checks whether the PID is still present.
    unsafe { libc::kill(pid, 0) == 0 }
}

#[test]
fn app_server_inherits_setup_process_group_and_is_reaped_on_drop() {
    let (_temp, options) = fake_codex("");
    let server = AppServer::start(Cursor::new(Vec::<u8>::new()), &options).unwrap();
    let pid = i32::try_from(server.child.id()).unwrap();
    assert_eq!(process_group(pid), current_process_group());
    drop(server);
    assert!(!process_exists(pid));
}

#[test]
fn standalone_app_server_owns_a_group_and_reaps_provider_children() {
    let (_temp, mut options) = fake_codex_script(
        r#"#!/bin/sh
[ "$1" = "app-server" ] && [ "$2" = "--stdio" ] || exit 20
/bin/sleep 30 >/dev/null 2>&1 &
printf '%s' "$!" > "$CODEX_HOME/provider-pid"
IFS= read -r initialize
"#,
    );
    options.host_process_group = false;
    let marker = options.codex_home.as_ref().unwrap().join("provider-pid");
    let server = AppServer::start(Cursor::new(Vec::<u8>::new()), &options).unwrap();
    let app_server_pid = i32::try_from(server.child.id()).unwrap();
    assert_eq!(process_group(app_server_pid), app_server_pid);
    let deadline = Instant::now() + Duration::from_secs(5);
    let provider_pid = loop {
        if let Ok(contents) = fs::read_to_string(&marker) {
            if let Ok(pid) = contents.parse::<i32>() {
                break pid;
            }
        }
        assert!(
            Instant::now() < deadline,
            "mock provider did not record PID"
        );
        thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(process_group(provider_pid), app_server_pid);
    drop(server);
    assert!(!process_exists(app_server_pid));
    let deadline = Instant::now() + Duration::from_secs(2);
    while process_exists(provider_pid) && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    assert!(!process_exists(provider_pid));
}

#[test]
fn final_login_status_probe_inherits_setup_group_and_is_reaped() {
    let (_temp, mut options) = fake_codex_script(
        r#"#!/bin/sh
[ "$1" = "login" ] && [ "$2" = "status" ] || exit 20
printf '%s' "$$" > "$CODEX_HOME/status-pid"
exec /bin/sleep 10
"#,
    );
    let marker = options.codex_home.as_ref().unwrap().join("status-pid");
    options.request_timeout = Duration::from_secs(2);
    let status = thread::spawn(move || login_status_ready(&options));
    let deadline = Instant::now() + Duration::from_secs(5);
    let pid = loop {
        if let Ok(contents) = fs::read_to_string(&marker) {
            if let Ok(pid) = contents.parse::<i32>() {
                break pid;
            }
        }
        if status.is_finished() {
            panic!(
                "mock status probe ended before recording PID: {}",
                status.join().unwrap()
            );
        }
        assert!(Instant::now() < deadline, "mock status probe did not start");
        thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(process_group(pid), current_process_group());
    assert!(!status.join().unwrap());
    assert!(!process_exists(pid));
}

#[test]
fn signed_out_login_succeeds_without_real_credentials_in_managed_home() {
    let (_temp, options) = fake_codex(
        r#"
: > "$CODEX_HOME/signed-in"
printf '%s\n' '{"method":"account/login/completed","params":{"loginId":"login-123","success":true,"error":null}}'
IFS= read -r account_after || exit 31
case "$account_after" in *'"method":"account/read"'*) ;; *) exit 32;; esac
printf '%s\n' '{"id":"cwr-login-account-after","result":{"account":{"type":"chatgpt","email":"secret@example.com"},"requiresOpenaiAuth":true}}'
"#,
    );
    let mut output = Vec::new();
    run_with_options(Cursor::new(Vec::<u8>::new()), &mut output, Ok(options)).unwrap();
    let events = events(&output);
    assert_eq!(events.len(), 2);
    assert_eq!(events[0]["event"], "auth_url");
    assert!(events[0]["auth_url"]
        .as_str()
        .unwrap()
        .starts_with("https://chatgpt.com/"));
    assert_eq!(events[1], json!({"event":"completed"}));
    assert!(!String::from_utf8(output)
        .unwrap()
        .contains("secret@example.com"));
}

#[test]
fn app_server_account_does_not_complete_when_cli_login_status_stays_blocked() {
    let (_temp, options) = fake_codex(
        r#"
printf '%s\n' '{"method":"account/login/completed","params":{"loginId":"login-123","success":true,"error":null}}'
IFS= read -r account_after || exit 31
printf '%s\n' '{"id":"cwr-login-account-after","result":{"account":{"type":"chatgpt"},"requiresOpenaiAuth":true}}'
"#,
    );
    let mut output = Vec::new();
    assert!(run_with_options(Cursor::new(Vec::<u8>::new()), &mut output, Ok(options)).is_err());
    let events = events(&output);
    assert_eq!(events[0]["event"], "auth_url");
    assert_eq!(events[1]["code"], "provider_login_unverified");
}

#[test]
fn stale_initial_account_does_not_complete_when_cli_status_is_signed_out() {
    let script = SIGNED_OUT_START.replace("\"account\":null", "\"account\":{\"type\":\"chatgpt\"}");
    let (_temp, options) = fake_codex_script(&script);
    let mut output = Vec::new();
    assert!(run_with_options(Cursor::new(Vec::<u8>::new()), &mut output, Ok(options)).is_err());
    let events = events(&output);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["code"], "provider_login_unverified");
}

struct CancelOnUrl {
    bytes: Arc<Mutex<Vec<u8>>>,
    cancel: Option<UnixStream>,
}

impl Write for CancelOnUrl {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut bytes = self.bytes.lock().unwrap();
        bytes.extend_from_slice(buf);
        if bytes.ends_with(b"\n")
            && bytes
                .windows(b"\"event\":\"auth_url\"".len())
                .any(|slice| slice == b"\"event\":\"auth_url\"")
        {
            if let Some(mut cancel) = self.cancel.take() {
                cancel.write_all(b"{\"command\":\"cancel\"}\n")?;
            }
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn cancel_sends_app_server_cancel_and_reports_cancelled() {
    let (_temp, options) = fake_codex(
        r#"
IFS= read -r cancel || exit 31
case "$cancel" in *'"method":"account/login/cancel"'*'"loginId":"login-123"'*) ;; *) exit 32;; esac
printf '%s\n' '{"id":"cwr-login-cancel","result":{}}'
"#,
    );
    let (reader, writer) = UnixStream::pair().unwrap();
    let bytes = Arc::new(Mutex::new(Vec::new()));
    let output = CancelOnUrl {
        bytes: Arc::clone(&bytes),
        cancel: Some(writer),
    };
    run_with_options(BufReader::new(reader), output, Ok(options)).unwrap();
    let events = events(&bytes.lock().unwrap());
    assert_eq!(events[0]["event"], "auth_url");
    assert_eq!(events[1], json!({"event":"cancelled"}));
}

#[test]
fn provider_failure_has_only_a_fixed_message() {
    let (_temp, options) = fake_codex(
        r#"
printf '%s\n' '{"method":"account/login/completed","params":{"loginId":"login-123","success":false,"error":"secret upstream account detail"}}'
"#,
    );
    let mut output = Vec::new();
    assert!(run_with_options(Cursor::new(Vec::<u8>::new()), &mut output, Ok(options)).is_err());
    let events = events(&output);
    assert_eq!(events[0]["event"], "auth_url");
    assert_eq!(events[1]["event"], "error");
    assert_eq!(events[1]["code"], "provider_login_failed");
    assert!(!String::from_utf8(output)
        .unwrap()
        .contains("secret upstream"));
}

#[test]
fn stalled_login_times_out_and_stops_child() {
    let (_temp, mut options) = fake_codex("/bin/sleep 10\n");
    options.login_timeout = Duration::from_millis(30);
    let mut output = Vec::new();
    let started = Instant::now();
    assert!(run_with_options(Cursor::new(Vec::<u8>::new()), &mut output, Ok(options)).is_err());
    assert!(started.elapsed() < Duration::from_secs(5));
    let events = events(&output);
    assert_eq!(events[0]["event"], "auth_url", "{events:?}");
    assert_eq!(events[1]["code"], "provider_login_timeout");
}

#[test]
fn success_notification_without_account_is_not_accepted() {
    let (_temp, options) = fake_codex(
        r#"
printf '%s\n' '{"method":"account/login/completed","params":{"loginId":"login-123","success":true,"error":null}}'
IFS= read -r account_after || exit 31
printf '%s\n' '{"id":"cwr-login-account-after","result":{"account":null,"requiresOpenaiAuth":true}}'
"#,
    );
    let mut output = Vec::new();
    assert!(run_with_options(Cursor::new(Vec::<u8>::new()), &mut output, Ok(options)).is_err());
    let events = events(&output);
    assert_eq!(events[1]["code"], "provider_login_unverified");
}

#[test]
fn non_https_auth_url_is_rejected() {
    assert!(!valid_auth_url("http://chatgpt.com/auth"));
    assert!(!valid_auth_url("file:///tmp/auth"));
    assert!(!valid_auth_url("https://user:pass@chatgpt.com/auth"));
    assert!(valid_auth_url("https://chatgpt.com/auth"));
}

#[test]
fn oversized_app_server_line_is_bounded_and_reports_fixed_protocol_error() {
    let (_temp, options) = fake_codex("printf '%70000s\\n' ''\n/bin/sleep 10\n");
    let mut output = Vec::new();
    assert!(run_with_options(Cursor::new(Vec::<u8>::new()), &mut output, Ok(options)).is_err());
    let events = events(&output);
    assert_eq!(events[0]["event"], "auth_url");
    assert_eq!(events[1]["code"], "provider_protocol_error");
    assert!(output.len() < 1024);

    let mut oversized = Cursor::new(vec![b'x'; MAX_SERVER_LINE_BYTES + 100]);
    assert!(read_line_bounded(&mut oversized, MAX_SERVER_LINE_BYTES).is_err());
    assert_eq!(oversized.position(), (MAX_SERVER_LINE_BYTES + 1) as u64);
}

#[test]
fn high_rate_app_server_notifications_stop_at_event_budget() {
    let (_temp, mut options) = fake_codex(
        r#"
i=0
while [ "$i" -lt 700 ]; do
  printf '%s\n' '{"method":"provider/noise","params":{"detail":"secret fixture text"}}'
  i=$((i + 1))
done
/bin/sleep 10
"#,
    );
    options.login_timeout = Duration::from_secs(2);
    let mut output = Vec::new();
    assert!(run_with_options(Cursor::new(Vec::<u8>::new()), &mut output, Ok(options)).is_err());
    let events = events(&output);
    assert_eq!(events[0]["event"], "auth_url");
    assert_eq!(events[1]["code"], "provider_protocol_error");
    assert!(!String::from_utf8(output)
        .unwrap()
        .contains("secret fixture text"));
}

#[test]
fn pending_completion_notifications_have_a_fixed_limit() {
    let (_temp, options) = fake_codex_script(
        r#"#!/bin/sh
[ "$1" = "app-server" ] && [ "$2" = "--stdio" ] || exit 20
IFS= read -r initialize || exit 21
i=0
while [ "$i" -lt 20 ]; do
  printf '{"method":"account/login/completed","params":{"loginId":"noise-%s","success":true}}\n' "$i"
  i=$((i + 1))
done
printf '%s\n' '{"id":"cwr-login-initialize","result":{}}'
/bin/sleep 10
"#,
    );
    let mut output = Vec::new();
    assert!(run_with_options(Cursor::new(Vec::<u8>::new()), &mut output, Ok(options)).is_err());
    assert_eq!(events(&output)[0]["code"], "provider_protocol_error");
}
