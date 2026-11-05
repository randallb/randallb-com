use super::*;

#[test]
fn resident_reconciles_changed_adapter_storage_before_reuse() {
    fn start(directory: &std::path::Path, storage: &std::path::Path) {
        let (mut child, mut input, output) =
            support::start_runtime_with_codex_and_storage(directory, None, Some(storage));
        let _ = send(
            &mut input,
            &output,
            HostCommand::InspectWorkspace {
                version: HOST_COMMAND_VERSION,
                request_id: "inspect-storage".into(),
            },
        );
        assert!(matches!(
            send(
                &mut input,
                &output,
                HostCommand::SelectActor {
                    version: HOST_COMMAND_VERSION,
                    request_id: "select-storage".into(),
                    actor_ref: crate::test_refs::actor_ref("person").as_str().into(),
                }
            ),
            HostEvent::ActorSelected { .. }
        ));
        drop(input);
        assert!(child.0.wait().unwrap().success());
    }
    let temporary = tempfile::tempdir().unwrap();
    let first = temporary.path().join("first-adapter-data");
    let second = temporary.path().join("second-adapter-data");
    start(temporary.path(), &first);
    let runtime = runtime_directory(temporary.path());
    let original_pid = fs::read_to_string(runtime.join("resident-runtime.pid")).unwrap();
    for storage in [&first, &second] {
        let result = Command::new(env!("CARGO_BIN_EXE_cwp-runtime"))
            .arg(temporary.path().join("workspace/CWR.toml"))
            .arg("--ensure-resident-environment")
            .env("WORKLOOP_CONFIGURATION", r#"{}"#)
            .env("XDG_STATE_HOME", temporary.path().join("state-home"))
            .env("XDG_RUNTIME_DIR", runtime_home(temporary.path()))
            .env("CWR_ADAPTER_DATA_ROOT", storage)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        if storage == &first {
            assert_eq!(
                fs::read_to_string(runtime.join("resident-runtime.pid")).unwrap(),
                original_pid
            );
        }
    }
    assert!(!runtime.join("resident-runtime.pid").exists());
    start(temporary.path(), &second);
    let marker: serde_json::Value =
        serde_json::from_slice(&fs::read(runtime.join("resident-environment.json")).unwrap())
            .unwrap();
    assert_eq!(
        marker["adapter_storage"]["data_root"],
        second.to_str().unwrap()
    );
    stop_resident(temporary.path());
}
