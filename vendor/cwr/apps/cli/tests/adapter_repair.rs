use std::{
    os::unix::fs::{symlink, PermissionsExt},
    path::PathBuf,
    process::{Command, Output, Stdio},
};

use serde_json::{json, Value};

const CODEX_ADAPTER_REF: &str = "cwp://org.coworkerprotocol/2026-09/adapters/codex";
const CLAUDE_ADAPTER_REF: &str = "cwp://org.coworkerprotocol/2026-09/adapters/claude";

fn fake_command(state: &tempfile::TempDir, arguments: &[&str]) -> Command {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../apps/workloop-desktop/testdata/fake_codex_login.sh");
    let home = state.path().join("host-home");
    std::fs::create_dir_all(&home).unwrap();
    let adapter = state.path().join("adapter");
    std::fs::write(
        &adapter,
        r#"#!/usr/bin/env python3
import json, os, pathlib, sys
assert sys.argv[1:] == ["--configure"]
data = pathlib.Path(os.environ["CWR_ADAPTER_DATA_DIRECTORY"])
home = data / "provider-home"
home.mkdir(mode=0o700, exist_ok=True)
print(json.dumps({"data_directory": str(data), "provider_home": str(home)}))
"#,
    )
    .unwrap();
    std::fs::set_permissions(&adapter, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_cwr"));
    command
        .args(arguments)
        .env_clear()
        .env("HOME", home)
        .env("USER", "fake-user")
        .env("PATH", "/usr/bin:/bin")
        .env("WORKLOOP_STATE_HOME", state.path())
        .env("CWR_ADAPTER_DATA_ROOT", state.path().join("adapters"))
        .env("CWR_CODEX_SOURCE_HOME", state.path().join("empty-source"))
        .env("CWR_CODEX_EXECUTABLE", &fixture)
        .env("CWR_CODEX_ADAPTER_EXECUTABLE", &adapter)
        .stdin(Stdio::null());
    command
}

fn fake_claude_command(state: &tempfile::TempDir, arguments: &[&str]) -> Command {
    let home = state.path().join("host-home");
    std::fs::create_dir_all(&home).unwrap();
    let executable = state.path().join("claude");
    std::fs::write(
        &executable,
        "#!/bin/sh\n[ \"$*\" = \"auth status --text\" ] || exit 2\nexit 0\n",
    )
    .unwrap();
    let mut permissions = executable.metadata().unwrap().permissions();
    permissions.set_mode(0o700);
    std::fs::set_permissions(&executable, permissions).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_cwr"));
    command
        .args(arguments)
        .env_clear()
        .env("HOME", home)
        .env("USER", "fake-user")
        .env("PATH", "/usr/bin:/bin")
        .env("WORKLOOP_STATE_HOME", state.path())
        .env("CWR_CLAUDE_EXECUTABLE", &executable)
        .env("CWR_CLAUDE_ADAPTER_EXECUTABLE", executable)
        .stdin(Stdio::null());
    command
}

fn successful_output(command: &mut Command) -> Output {
    let output = command.output().expect("cwr runs");
    assert!(
        output.status.success(),
        "cwr exited with {:?}: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn single_json(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).expect("one JSON value")
}

#[test]
fn cli_status_repair_status_uses_isolated_fake_codex_without_credentials() {
    let state = tempfile::tempdir().unwrap();
    let before = successful_output(&mut fake_command(
        &state,
        &["adapter", "status", CODEX_ADAPTER_REF],
    ));
    let before = single_json(&before);
    assert_eq!(before["status"], "setup_required");
    assert_eq!(before["code"], "provider_login_required");
    assert_eq!(before["repair"]["kind"], "provider_login");

    let repair = successful_output(&mut fake_command(
        &state,
        &["adapter", "repair", CODEX_ADAPTER_REF, "provider_login"],
    ));
    let events = String::from_utf8(repair.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0]["event"], "auth_url");
    assert_eq!(
        events[0]["auth_url"],
        "https://auth.example.invalid/authorize"
    );
    assert_eq!(events[1], json!({"event": "completed"}));

    let after = successful_output(&mut fake_command(
        &state,
        &["adapter", "status", CODEX_ADAPTER_REF],
    ));
    let after = single_json(&after);
    assert_eq!(after["status"], "ready");
    assert!(after.get("repair").is_none());

    let repeated = successful_output(&mut fake_command(
        &state,
        &["adapter", "repair", CODEX_ADAPTER_REF, "provider_login"],
    ));
    assert_eq!(single_json(&repeated), json!({"event": "completed"}));
}

#[test]
fn signed_in_codex_with_missing_adapter_companion_still_needs_setup() {
    let state = tempfile::tempdir().unwrap();
    let managed_home = state.path().join("adapters/codex/provider-home");
    std::fs::create_dir_all(&managed_home).unwrap();
    std::fs::write(managed_home.join("signed-in"), b"").unwrap();
    let mut command = fake_command(&state, &["adapter", "status", CODEX_ADAPTER_REF]);
    command.env(
        "CWR_CODEX_ADAPTER_EXECUTABLE",
        state.path().join("missing-adapter"),
    );
    let status = single_json(&successful_output(&mut command));
    assert_eq!(status["status"], "setup_required");
    assert_eq!(status["code"], "adapter_executable_missing");
    assert!(status.get("repair").is_none());
}

#[test]
fn claude_status_checks_companion_and_safe_managed_home_without_repair_offer() {
    let ready_state = tempfile::tempdir().unwrap();
    let ready = single_json(&successful_output(&mut fake_claude_command(
        &ready_state,
        &["adapter", "status", CLAUDE_ADAPTER_REF],
    )));
    assert_eq!(ready["status"], "ready");
    assert!(ready.get("repair").is_none());

    let missing_state = tempfile::tempdir().unwrap();
    let mut missing =
        fake_claude_command(&missing_state, &["adapter", "status", CLAUDE_ADAPTER_REF]);
    missing.env(
        "CWR_CLAUDE_ADAPTER_EXECUTABLE",
        missing_state.path().join("missing-companion"),
    );
    let missing = single_json(&successful_output(&mut missing));
    assert_eq!(missing["status"], "setup_required");
    assert_eq!(missing["code"], "adapter_executable_missing");
    assert!(missing.get("repair").is_none());

    let unsafe_state = tempfile::tempdir().unwrap();
    let providers = unsafe_state.path().join("provider-homes");
    let target = unsafe_state.path().join("unsafe-target");
    std::fs::create_dir_all(&providers).unwrap();
    std::fs::create_dir(&target).unwrap();
    symlink(&target, providers.join("claude")).unwrap();
    let unsafe_home = single_json(&successful_output(&mut fake_claude_command(
        &unsafe_state,
        &["adapter", "status", CLAUDE_ADAPTER_REF],
    )));
    assert_eq!(unsafe_home["status"], "temporarily_unavailable");
    assert_eq!(unsafe_home["code"], "provider_home_unavailable");
    assert!(unsafe_home.get("repair").is_none());

    let refused = fake_claude_command(
        &ready_state,
        &["adapter", "repair", CLAUDE_ADAPTER_REF, "provider_login"],
    )
    .output()
    .unwrap();
    assert!(!refused.status.success());
    assert_eq!(single_json(&refused)["code"], "adapter_repair_unavailable");
}
