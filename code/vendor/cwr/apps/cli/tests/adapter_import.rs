use std::{
    os::unix::fs::PermissionsExt,
    path::Path,
    process::{Command, Stdio},
};

use serde_json::Value;

const CODEX_ADAPTER_REF: &str = "cwp://org.coworkerprotocol/2026-09/adapters/codex";

fn import_command(state: &Path, adapter_ref: &str) -> Command {
    let adapter = state.join("adapter");
    std::fs::write(
        &adapter,
        r#"#!/usr/bin/env python3
import json, os, pathlib, sys
data = pathlib.Path(os.environ["CWR_ADAPTER_DATA_DIRECTORY"])
assert not data.exists(), "import transport initialized the destination"
assert sys.argv[1] == "--import-home", "import transport called configure"
print(json.dumps({"source_home": sys.argv[2], "provider_home": str(data / "provider-home"),
    "dry_run": "--dry-run" in sys.argv, "replace": "--replace" in sys.argv,
    "imported": ["plugins/cache/fixture/skill/SKILL.md"], "conflicts": []}))
"#,
    )
    .unwrap();
    std::fs::set_permissions(&adapter, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_cwr"));
    command
        .args(["adapter", "import", adapter_ref])
        .env_clear()
        .env("HOME", state.join("home"))
        .env("PATH", "/usr/bin:/bin")
        // The fake Python adapter must not populate macOS bytecode caches in HOME.
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env("CWR_ADAPTER_DATA_ROOT", state.join("new-adapters"))
        .env("CWR_CODEX_ADAPTER_EXECUTABLE", adapter)
        .current_dir(state)
        .stdin(Stdio::null());
    command
}

#[test]
fn import_preview_dispatches_to_adapter_without_initializing_storage_or_workspace() {
    let state = tempfile::tempdir().unwrap();
    let source = state.path().join("source with spaces");
    std::fs::create_dir(&source).unwrap();
    std::fs::write(source.join("original"), "unchanged").unwrap();
    let output = import_command(state.path(), CODEX_ADAPTER_REF)
        .arg("--source")
        .arg(&source)
        .args(["--dry-run", "--replace"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["source_home"], source.to_str().unwrap());
    assert_eq!(report["dry_run"], true);
    assert_eq!(report["replace"], true);
    assert!(!state.path().join("new-adapters").exists());
    assert!(!state.path().join("home").exists());
    assert!(!state.path().join("CWR.toml").exists());
    assert_eq!(
        std::fs::read_to_string(source.join("original")).unwrap(),
        "unchanged"
    );
}

#[test]
fn import_rejects_unsupported_adapter_before_running_its_executable() {
    let state = tempfile::tempdir().unwrap();
    let output = import_command(
        state.path(),
        "cwp://org.coworkerprotocol/2026-09/adapters/claude",
    )
    .args(["--source", "/fictional/source", "--dry-run"])
    .output()
    .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Adapter import is unavailable"));
    assert!(output.stdout.is_empty());
    assert!(!state.path().join("new-adapters").exists());
}

#[test]
fn import_without_flags_does_not_request_replacement_or_preview() {
    let state = tempfile::tempdir().unwrap();
    let output = import_command(state.path(), CODEX_ADAPTER_REF)
        .args(["--source", "/fictional/source"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["dry_run"], false);
    assert_eq!(report["replace"], false);
}

#[test]
fn import_failure_reports_the_reason_without_multiline_source_excerpts() {
    let state = tempfile::tempdir().unwrap();
    let mut command = import_command(state.path(), CODEX_ADAPTER_REF);
    std::fs::write(
        state.path().join("adapter"),
        "#!/bin/sh\nprintf 'source configuration is invalid at line 3\\nprivate configuration excerpt\\n' >&2\nexit 1\n",
    )
    .unwrap();
    let output = command
        .args(["--source", "/fictional/source", "--dry-run"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let diagnostic = String::from_utf8_lossy(&output.stderr);
    assert!(diagnostic.contains("source configuration is invalid at line 3"));
    assert!(!diagnostic.contains("private configuration excerpt"));
    assert!(output.stdout.is_empty());
    assert!(!state.path().join("new-adapters").exists());
}
