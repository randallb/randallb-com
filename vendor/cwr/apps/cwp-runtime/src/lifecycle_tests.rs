use super::*;
use crate::config::ProcessCommunicationAdapter;

#[test]
fn runtime_surfaces_an_unexpected_communication_adapter_exit() {
    let mut adapters = vec![Command::new("sh")
        .args(["-c", "exit 7"])
        .spawn()
        .expect("start failing adapter")];
    let temporary = tempfile::tempdir().unwrap();
    let workspace = temporary.path().join("CWR.toml");
    std::fs::write(&workspace, "").unwrap();
    let config = RuntimeConfig {
        configured_runtime: None,
        workspace,
        state_directory: PathBuf::from("."),
        runtime_directory: PathBuf::from("."),
        workspace_id: None,
        runtime_root_ref: "cwp://example.test/runtime".into(),
        acting_actor_ref: Some(crate::test_refs::actor_ref("person").as_str().into()),
        socket_path: PathBuf::from("runtime.sock"),
        host_socket_path: PathBuf::from("host.sock"),
        config_directory: PathBuf::from("."),
        communication_adapters: Vec::new(),
        expected_communication_adapters: 0,
        max_concurrent_conversations: std::num::NonZeroUsize::new(16).unwrap(),
    };

    let error = supervise_runtime(
        &config,
        RuntimeMode::Service,
        crate::test_refs::actor_ref("person").as_str(),
        &mut adapters,
        None,
        None,
        || Ok(false),
    )
    .expect_err("adapter exit must fail the runtime");
    assert!(error.to_string().contains("exited unexpectedly"));
}

#[test]
fn adapter_startup_failure_stops_adapters_that_already_started() {
    let temporary = tempfile::tempdir().expect("temporary adapter directory");
    let config = RuntimeConfig {
        configured_runtime: None,
        workspace: PathBuf::from("."),
        state_directory: temporary.path().into(),
        runtime_directory: temporary.path().into(),
        workspace_id: None,
        runtime_root_ref: "cwp://example.test/runtime".into(),
        acting_actor_ref: Some(crate::test_refs::actor_ref("person").as_str().into()),
        socket_path: temporary.path().join("runtime.sock"),
        host_socket_path: temporary.path().join("host.sock"),
        config_directory: temporary.path().into(),
        communication_adapters: vec![
            ProcessCommunicationAdapter {
                command: PathBuf::from("/bin/sleep"),
                args: vec!["30".into()],
            },
            ProcessCommunicationAdapter {
                command: temporary.path().join("missing-adapter"),
                args: Vec::new(),
            },
        ],
        expected_communication_adapters: 2,
        max_concurrent_conversations: std::num::NonZeroUsize::new(16).unwrap(),
    };
    let mut first_pid = None;
    let error = spawn_communication_adapters_with(&config, |command| {
        let child = command.spawn()?;
        first_pid.get_or_insert_with(|| child.id());
        Ok(child)
    })
    .expect_err("missing second adapter must fail startup");
    assert_eq!(
        error.downcast_ref::<std::io::Error>().unwrap().kind(),
        std::io::ErrorKind::NotFound
    );

    let status = Command::new("kill")
        .args(["-0", &first_pid.expect("first adapter PID").to_string()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .expect("inspect first adapter process");
    assert!(
        !status.success(),
        "first adapter must be stopped and reaped"
    );
}

#[test]
fn runtime_stops_when_a_retired_shared_folder_marker_appears() {
    let temporary = tempfile::tempdir().unwrap();
    let workspace = temporary.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    std::fs::write(workspace.join("CWR.toml"), "").unwrap();
    std::fs::create_dir(workspace.join(".cwr")).unwrap();
    let config = RuntimeConfig {
        configured_runtime: None,
        workspace: workspace.join("CWR.toml"),
        state_directory: temporary.path().into(),
        runtime_directory: temporary.path().into(),
        workspace_id: None,
        runtime_root_ref: "cwp://example.test/runtime".into(),
        acting_actor_ref: Some(crate::test_refs::actor_ref("person").as_str().into()),
        socket_path: temporary.path().join("runtime.sock"),
        host_socket_path: temporary.path().join("host.sock"),
        config_directory: temporary.path().into(),
        communication_adapters: Vec::new(),
        expected_communication_adapters: 0,
        max_concurrent_conversations: std::num::NonZeroUsize::new(16).unwrap(),
    };
    let marker = workspace.join(".cwr/shared-folder.json");
    let writer = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(25));
        std::fs::write(marker, "{}").unwrap();
    });

    let error = supervise_runtime(
        &config,
        RuntimeMode::Service,
        crate::test_refs::actor_ref("person").as_str(),
        &mut [],
        None,
        None,
        || Ok(false),
    )
    .expect_err("the runtime must stop when a retired marker appears");
    writer.join().unwrap();
    assert!(error
        .to_string()
        .contains("shared-folder workspaces are no longer supported"));
}

#[test]
fn retirement_receipt_requires_every_cleanup_result_to_succeed() {
    let temporary = tempfile::tempdir().unwrap();
    let receipt = cwr::v2026_09::local::LocalResidentRetirement {
        version: 1,
        nonce: uuid::Uuid::new_v4().to_string(),
        runtime_ref: "cwp://local.fixture/runtimes/retirement".into(),
        owner_pid: std::process::id() as i32,
        // SAFETY: geteuid takes no pointers and only reads process identity.
        owner_uid: unsafe { libc::geteuid() },
    };
    for failure in 0..4 {
        let error = || std::io::Error::other("injected cleanup failure").into();
        assert!(finish_retirement(
            temporary.path(),
            if failure == 0 { Err(error()) } else { Ok(()) },
            if failure == 1 {
                Err(error())
            } else {
                Ok(Vec::new())
            },
            if failure == 2 { Err(error()) } else { Ok(()) },
            if failure == 3 {
                Err(error())
            } else {
                Ok(Some(receipt.clone()))
            },
        )
        .is_err());
        assert!(!temporary
            .path()
            .join("resident-retirement-v1.json")
            .exists());
    }
    finish_retirement(
        temporary.path(),
        Ok(()),
        Ok(Vec::new()),
        Ok(()),
        Ok(Some(receipt.clone())),
    )
    .unwrap();
    assert!(temporary
        .path()
        .join("resident-retirement-v1.json")
        .exists());
    assert!(
        !cwr::v2026_09::local::completed_local_resident_retirement(temporary.path(), &receipt)
            .unwrap()
    );
}

#[test]
fn checked_adapter_cleanup_reaps_live_and_already_exited_children() {
    let mut children = vec![
        Command::new("/bin/sleep").arg("30").spawn().unwrap(),
        Command::new("/bin/sh")
            .args(["-c", "exit 7"])
            .spawn()
            .unwrap(),
    ];
    children[1].wait().unwrap();
    stop_communication_adapters(&mut children).unwrap();
    for child in &mut children {
        assert!(child.try_wait().unwrap().is_some());
    }
}
