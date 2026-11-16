//! Boot diagnostics must exist even when resident configuration cannot load.
#[test]
fn resident_configuration_failure_is_saved_before_runtime_start() {
    let root = tempfile::tempdir().unwrap();
    let configuration = root.path().join("broken.toml");
    std::fs::write(&configuration, "not valid toml [").unwrap();
    let status = std::process::Command::new(env!("CARGO_BIN_EXE_cwp-runtime"))
        .arg(&configuration)
        .env("CWR_RESIDENT_DIAGNOSTIC_STATE_DIRECTORY", root.path())
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(!status.success());
    let mut store =
        cwr::cwpdb::NativeStore::open_read_only(root.path().join("diagnostics/wlpdb.sqlite3"))
            .unwrap();
    let page = store.diagnostic_records(None, 100).unwrap();
    assert!(!page.truncated);
    assert!(page.records.iter().any(|record| record.value["message"]
        .as_str()
        .is_some_and(|message| message.contains("resident failed"))));
    assert!(!root.path().join("resident-runtime.log").exists());
}
