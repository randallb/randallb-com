use std::{fs, process::Command};

#[test]
fn ordinary_sms_inspection_preserves_existing_identity_and_history_without_initialization() {
    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path().join("workspace");
    fs::create_dir_all(&workspace).unwrap();
    let manifest = workspace.join("CWR.toml");
    fs::write(&manifest, "version = '2026-09'\n").unwrap();
    let state = temp.path().join("state");
    let runtime = temp.path().join("runtime");
    let inspect = |input: &std::path::Path| {
        Command::new(env!("CARGO_BIN_EXE_cwp-runtime"))
            .arg(input)
            .arg("--existing-runtime-selection")
            .env("XDG_STATE_HOME", &state)
            .env("XDG_RUNTIME_DIR", &runtime)
            .output()
            .unwrap()
    };
    assert!(!inspect(&manifest).status.success());
    assert!(!workspace.join(".cwr").exists());
    assert!(!state.exists());
    assert!(!runtime.exists());
    fs::create_dir(workspace.join(".cwr")).unwrap();
    let identity = "{\"version\":1,\"workspace_id\":\"5f0dc30e-41bd-4f99-a8e4-0fb66caef76f\"}\n";
    fs::write(workspace.join(".cwr/workspace.json"), identity).unwrap();
    let result = inspect(&manifest);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let selection: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(
        selection["workspace_path"],
        fs::canonicalize(&workspace).unwrap().to_str().unwrap()
    );
    assert!(
        !workspace.join(".cwr/workspace.toml").exists(),
        "inspection migrated an identity"
    );
    assert!(!state.exists());
    assert!(!runtime.exists());
    let database = std::path::Path::new(selection["database_path"].as_str().unwrap());
    fs::create_dir_all(database.parent().unwrap()).unwrap();
    fs::write(database, "existing conversation/history sentinel").unwrap();
    let config = temp.path().join("runtime.toml");
    fs::write(
        &config,
        format!(
            "version = 1\nworkspace = {:?}\n",
            manifest.to_str().unwrap()
        ),
    )
    .unwrap();
    let selected_again = inspect(&config);
    assert!(selected_again.status.success());
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&selected_again.stdout).unwrap(),
        selection
    );
    assert_eq!(
        fs::read_to_string(database).unwrap(),
        "existing conversation/history sentinel"
    );
    assert_eq!(
        fs::read_to_string(workspace.join(".cwr/workspace.json")).unwrap(),
        identity
    );
    fs::write(&config, format!("version = 1\nworkspace = {:?}\nstate_directory = './elsewhere'\nruntime_root_ref = 'cwp://example.test/new'\n", manifest.to_str().unwrap())).unwrap();
    assert!(!inspect(&config).status.success());
    assert!(!temp.path().join("elsewhere").exists());
}
