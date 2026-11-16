use serde_json::Value;
use std::{fs, os::unix::fs::PermissionsExt, process::Command};

#[test]
fn startup_diagnostics_are_opt_in_and_do_not_change_stdout() {
    let run = |enabled| {
        Command::new(env!("CARGO_BIN_EXE_cwr"))
            .env("CWR_STARTUP_TIMING", enabled)
            .arg("--help")
            .output()
            .unwrap()
    };
    let ordinary = run("0");
    let measured = run("1");
    assert!(ordinary.status.success() && measured.status.success());
    assert_eq!(ordinary.stdout, measured.stdout);
    assert!(ordinary.stderr.is_empty());
    let trace: Value = serde_json::from_slice(&measured.stderr).unwrap();
    assert_eq!(trace["checkpoint"], "cli_startup_timing");
    assert_eq!(trace["success"], true);
    assert!(trace["spans"]
        .as_array()
        .unwrap()
        .iter()
        .any(|s| s["stage"] == "cli_preparation"));
}

#[test]
fn failed_bridge_preserves_elapsed_inspection_without_recording_request_content() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("CWR.toml"), "").unwrap();
    let bridge = temp.path().join("bridge");
    fs::write(&bridge, "#!/bin/sh\nsleep 0.05\nexit 1\n").unwrap();
    fs::set_permissions(&bridge, fs::Permissions::from_mode(0o700)).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_cwr"))
        .current_dir(temp.path())
        .env("CWR_STARTUP_TIMING", "1")
        .env_remove("CWR_WORKSPACE")
        .env_remove("CWR_ACTOR_REF")
        .env("CWR_DEV_RUNTIME_EXECUTABLE", &bridge)
        .args(["exec", "private timing test input"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    let trace: Value = stderr
        .lines()
        .find_map(|l| serde_json::from_str(l).ok())
        .unwrap();
    assert_eq!(trace["success"], false);
    let inspection = trace["spans"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["stage"] == "workspace_inspection")
        .unwrap_or_else(|| panic!("missing inspection: {stderr}"));
    assert_eq!(inspection["completed"], false);
    assert!(inspection["duration_ms"].as_u64().unwrap() >= 40);
    let encoded = trace.to_string();
    assert!(!encoded.contains("private timing test input"));
    assert!(!encoded.contains(temp.path().to_str().unwrap()));
}
