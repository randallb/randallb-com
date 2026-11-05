//! Credential-free exercise of the trusted Codex setup mode and system home.

use std::{
    fs,
    io::{BufRead, BufReader, Write},
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    process::{Child, ChildStdin, Command, ExitStatus, Stdio},
    sync::mpsc::{self, Receiver},
    thread,
    time::{Duration, Instant},
};

use serde_json::{json, Value};
use tempfile::TempDir;

const FAKE_CODEX: &str = r#"#!/bin/sh
set -eu
codex_home="${CODEX_HOME:-$HOME/.codex}"

if [ "$#" -eq 2 ] && [ "$1" = login ] && [ "$2" = status ]; then
  if [ -f "$codex_home/signed-in" ]; then exit 0; fi
  exit 1
fi

if [ "$#" -ne 2 ] || [ "$1" != app-server ] || [ "$2" != --stdio ]; then
  exit 2
fi

IFS= read -r mode < "$HOME/mode"
printf '%s\n' "${CODEX_HOME-unset}" > "$HOME/observed-codex-home"
while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*)
      printf '%s\n' '{"id":"cwr-login-initialize","result":{}}'
      ;;
    *'"method":"account/read"'*'"id":"cwr-login-account-before"'*|*'"id":"cwr-login-account-before"'*'"method":"account/read"'*)
      printf '%s\n' '{"id":"cwr-login-account-before","result":{"account":null,"requiresOpenaiAuth":true}}'
      ;;
    *'"method":"account/login/start"'*)
      printf '%s\n' '{"id":"cwr-login-start","result":{"type":"chatgpt","loginId":"fake-login","authUrl":"https://auth.example.invalid/authorize"}}'
      if [ "$mode" = success ]; then
        printf '%s\n' ready > "$codex_home/signed-in"
        printf '%s\n' '{"method":"account/login/completed","params":{"loginId":"fake-login","success":true}}'
      elif [ "$mode" = failure ]; then
        printf '%s\n' '{"method":"account/login/completed","params":{"loginId":"fake-login","success":false,"detail":"fake-private-provider-detail"}}'
      fi
      ;;
    *'"method":"account/read"'*'"id":"cwr-login-account-after"'*|*'"id":"cwr-login-account-after"'*'"method":"account/read"'*)
      printf '%s\n' '{"id":"cwr-login-account-after","result":{"account":{"type":"chatgpt"},"requiresOpenaiAuth":true}}'
      ;;
    *'"method":"account/login/cancel"'*)
      printf '%s\n' cancelled > "$codex_home/cancelled"
      printf '%s\n' '{"id":"cwr-login-cancel","result":{}}'
      ;;
  esac
done
"#;

struct Fixture {
    _temporary: TempDir,
    home: PathBuf,
    state: PathBuf,
    decoy: PathBuf,
    source: PathBuf,
    executable: PathBuf,
}

impl Fixture {
    fn new(mode: &str) -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let home = temporary.path().join("home");
        let state = temporary.path().join("state");
        let decoy = temporary.path().join("unmanaged-codex-home");
        let source = temporary.path().join("empty-import-source");
        let executable = temporary.path().join("fake-codex");
        for directory in [&home, &state, &decoy, &source, &home.join(".codex")] {
            fs::create_dir(directory).unwrap();
        }
        fs::write(home.join("mode"), format!("{mode}\n")).unwrap();
        // An already signed-in caller home must not count as Workloop's home
        // when initialization explicitly selects a different, empty source.
        fs::write(decoy.join("signed-in"), "legacy-only\n").unwrap();
        fs::write(&executable, FAKE_CODEX).unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        Self {
            _temporary: temporary,
            home,
            state,
            decoy,
            source,
            executable,
        }
    }

    fn system_home(&self) -> PathBuf {
        self.home.join(".codex")
    }

    fn command(&self, mode: &str) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cwr-adapter-codex"));
        command
            .arg(mode)
            .current_dir(&self.home)
            .env_clear()
            .env("HOME", &self.home)
            .env("USER", "fixture")
            .env("PATH", "/usr/bin:/bin")
            .env("WORKLOOP_STATE_HOME", &self.state)
            .env("CWR_ADAPTER_DATA_ROOT", self.state.join("adapters"))
            .env(
                "CWR_CODEX_ADAPTER_EXECUTABLE",
                env!("CARGO_BIN_EXE_cwr-adapter-codex"),
            )
            .env("CWR_CODEX_EXECUTABLE", &self.executable)
            .env("CWR_CODEX_SOURCE_HOME", &self.source)
            .env("CODEX_HOME", &self.decoy);
        command
    }

    fn assert_source_and_caller_unchanged(&self) {
        assert_eq!(fs::read_dir(&self.source).unwrap().count(), 0);
        assert_eq!(fs::read_dir(&self.decoy).unwrap().count(), 1);
        assert_eq!(
            fs::read_to_string(self.decoy.join("signed-in")).unwrap(),
            "legacy-only\n"
        );
    }

    fn status(&self) -> Value {
        let output = self.command("--setup-status").output().unwrap();
        assert!(output.status.success(), "setup-status failed: {output:?}");
        serde_json::from_slice(&output.stdout).unwrap()
    }

    fn login(&self) -> RunningLogin {
        let mut child = self
            .command("--setup-login")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (sender, events) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                if sender.send(line.unwrap()).is_err() {
                    return;
                }
            }
        });
        RunningLogin {
            child,
            stdin,
            events,
        }
    }
}

struct RunningLogin {
    child: Child,
    stdin: ChildStdin,
    events: Receiver<String>,
}

impl RunningLogin {
    fn next_event(&self) -> Value {
        let line = self
            .events
            .recv_timeout(Duration::from_secs(10))
            .expect("adapter did not emit a setup event");
        serde_json::from_str(&line).unwrap()
    }

    fn finish(&mut self) -> ExitStatus {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status;
            }
            assert!(Instant::now() < deadline, "adapter setup mode hung");
            thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for RunningLogin {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn login_makes_the_system_home_ready() {
    let fixture = Fixture::new("success");
    let before = fixture.status();
    assert_eq!(before["status"], "setup_required");
    assert_eq!(before["code"], "provider_login_required");
    assert_eq!(
        before["repair"],
        json!({"kind": "provider_login", "label": "Sign in"})
    );

    let mut login = fixture.login();
    assert_eq!(
        login.next_event(),
        json!({
            "event": "auth_url", "auth_url": "https://auth.example.invalid/authorize"
        })
    );
    assert_eq!(login.next_event(), json!({"event": "completed"}));
    assert!(login.finish().success());

    assert_eq!(
        fs::read_to_string(fixture.home.join("observed-codex-home"))
            .unwrap()
            .trim(),
        "unset"
    );
    let system_home = fixture.system_home();
    assert!(system_home.join("signed-in").exists());
    assert_eq!(
        fs::metadata(&system_home).unwrap().permissions().mode() & 0o777,
        0o755
    );
    let after = fixture.status();
    assert_eq!(after["status"], "ready");
    assert_eq!(after["code"], "ready");
    fixture.assert_source_and_caller_unchanged();
}

#[test]
fn cancel_reaches_codex_and_does_not_mark_the_home_ready() {
    let fixture = Fixture::new("cancel");
    let mut login = fixture.login();
    assert_eq!(login.next_event()["event"], "auth_url");
    login
        .stdin
        .write_all(b"{\"command\":\"cancel\"}\n")
        .unwrap();
    login.stdin.flush().unwrap();
    assert_eq!(login.next_event(), json!({"event": "cancelled"}));
    assert!(login.finish().success());
    assert!(fixture.system_home().join("cancelled").exists());
    assert_eq!(fixture.status()["code"], "provider_login_required");
    fixture.assert_source_and_caller_unchanged();
}

#[test]
fn failed_login_stays_signed_out_without_exposing_provider_output() {
    let fixture = Fixture::new("failure");
    let mut login = fixture.login();
    assert_eq!(login.next_event()["event"], "auth_url");
    let error = login.next_event();
    assert_eq!(error["event"], "error");
    assert_eq!(error["code"], "provider_login_failed");
    assert!(error["message"].as_str().unwrap().contains("try again"));
    assert!(!error.to_string().contains("fake-private-provider-detail"));
    assert!(!login.finish().success());
    assert!(!fixture.system_home().join("signed-in").exists());
    assert_eq!(fixture.status()["code"], "provider_login_required");
    fixture.assert_source_and_caller_unchanged();
}
