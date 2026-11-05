#[test]
fn archive_cancels_a_real_local_adapter_without_retaining_failed_history() {
    use crate::v2026_09::upstream::{UpstreamCoworker, UpstreamEvent};
    let _environment_lock = SH_ADAPTER_ENV_LOCK.lock().unwrap();
    struct AdapterEnvironment(Option<std::ffi::OsString>);
    impl Drop for AdapterEnvironment {
        fn drop(&mut self) {
            match self.0.take() {
                Some(previous) => std::env::set_var("CWR_SH_ADAPTER_EXECUTABLE", previous),
                None => std::env::remove_var("CWR_SH_ADAPTER_EXECUTABLE"),
            }
        }
    }
    let temporary = tempfile::tempdir().unwrap();
    let adapter = temporary.path().join("archive-adapter");
    let marker = temporary.path().join("adapter.pid");
    fs::write(
        &adapter,
        format!(
            "#!/bin/sh\nIFS= read -r invocation\nprintf '%s' \"$$\" > '{}'\nexec /bin/sleep 30\n",
            marker.display()
        ),
    )
    .unwrap();
    let mut permissions = fs::metadata(&adapter).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&adapter, permissions).unwrap();
    let _environment = AdapterEnvironment(std::env::var_os("CWR_SH_ADAPTER_EXECUTABLE"));
    std::env::set_var("CWR_SH_ADAPTER_EXECUTABLE", &adapter);
    let workspace = temporary.path().join("workspace");
    copy_workspace(fixture_workspace().parent().unwrap(), &workspace);
    fs::write(workspace.join("coworkers/helper/integration.toml"),
        "type = 'cwp://org.coworkerprotocol/2026-09/schemas/integration'\n[adapter]\nref = 'cwp://org.coworkerprotocol/2026-09/adapters/sh'\n").unwrap();
    fs::write(workspace.join("coworkers/helper/ROLE.toml"),
        "type = 'cwp://org.coworkerprotocol/2026-09/schemas/role'\n[[adapters]]\nref = 'cwp://org.coworkerprotocol/2026-09/adapters/sh'\n").unwrap();
    let person = load_default_local_acting_actor_ref(workspace.join("CWR.toml")).unwrap();
    let runtime = LocalRuntime::open_with_state_path(
        workspace.join("CWR.toml"),
        temporary.path().join("runtime.sqlite"),
        LocalHostIdentity::new(&person, "cwp://example.test/ingress"),
    )
    .unwrap();
    let (sender, upstream_events) = std::sync::mpsc::channel();
    let (deadline, _deadline_events) = std::sync::mpsc::sync_channel(1);
    let actor = "cwp://example.test/actors/provider";
    let session = runtime
        .register_upstream_provider(
            "connection",
            vec![UpstreamCoworker {
                actor_ref: actor.into(),
                coworker_ref: "cwp://example.test/coworkers/provider".into(),
                handle: "@provider".into(),
                name: "Provider".into(),
                actions: vec![],
            }],
            Arc::new(move |event| {
                sender
                    .send(event)
                    .map_err(|error| invalid_error(error.to_string()))
            }),
            deadline,
        )
        .unwrap();
    runtime
        .enable_upstream_coworker("connection", &session, actor, &person, true)
        .unwrap();
    let local = runtime.admit_text(None, "@helper local work").unwrap();
    let local_ref = local.invocation_ref().unwrap().to_owned();
    let local_runtime = runtime.clone();
    let (local_sender, local_done) = std::sync::mpsc::channel();
    let local_worker = thread::spawn(move || {
        local_sender
            .send(local_runtime.execute_admission(&local, |_| {}))
            .unwrap()
    });
    let deadline = Instant::now() + Duration::from_secs(3);
    while !marker.exists() {
        assert!(Instant::now() < deadline, "local adapter did not start");
        thread::sleep(Duration::from_millis(10));
    }
    let conversation = runtime
        .lock()
        .unwrap()
        .admission
        .invocation_record(&local_ref)
        .unwrap()
        .conversation_ref
        .clone()
        .unwrap();
    let upstream = runtime
        .submit_text_in_upstream_scope(
            "cwp://example.test/ingress",
            "archive-provider",
            &person,
            Some(&conversation),
            "@provider work",
            Some("connection"),
        )
        .unwrap();
    let upstream_ref = upstream.invocation_ref().unwrap().to_owned();
    let upstream_runtime = runtime.clone();
    // Exercise concurrent local/provider execution, as independently delegated
    // work can occupy separate worker groups beneath one archive boundary.
    let upstream_worker = thread::spawn(move || {
        upstream_runtime.execute_requested_ref(
            upstream.invocation_ref().unwrap(),
            &AdapterModelOutputMailbox::discard(),
            &mut |_| {},
        )
    });
    assert!(matches!(
        upstream_events
            .recv_timeout(Duration::from_secs(2))
            .unwrap(),
        UpstreamEvent::WorkRequested { .. }
    ));
    let archive_runtime = runtime.clone();
    let archive_conversation = conversation.clone();
    let archive_worker = thread::spawn(move || {
        archive_runtime.set_conversation_archived_as(Some(&archive_conversation), &person, true)
    });
    assert!(matches!(
        upstream_events
            .recv_timeout(Duration::from_secs(2))
            .unwrap(),
        UpstreamEvent::StopRequested { .. }
    ));
    let deadline = Instant::now() + Duration::from_secs(3);
    while runtime.lock().unwrap().active.contains_key(&local_ref) {
        assert!(
            Instant::now() < deadline,
            "local process did not stop before acknowledgement"
        );
        thread::sleep(Duration::from_millis(10));
    }
    assert!(matches!(
        local_done.recv_timeout(Duration::from_millis(50)),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout)
    ));
    assert_eq!(
        runtime
            .lock()
            .unwrap()
            .admission
            .invocation_record(&local_ref)
            .unwrap()
            .state,
        ActionInvocationState::Running
    );
    runtime
        .acknowledge_upstream_stop("connection", &upstream_ref)
        .unwrap();
    archive_worker.join().unwrap().unwrap();
    let local_turn = local_done
        .recv_timeout(Duration::from_secs(2))
        .unwrap()
        .unwrap();
    local_worker.join().unwrap();
    upstream_worker.join().unwrap().unwrap();
    assert_eq!(
        local_turn.invocation.unwrap().state,
        ActionInvocationState::Stopped
    );
    let state = runtime.lock().unwrap();
    let record = state.admission.invocation_record(&local_ref).unwrap();
    assert_eq!(record.state, ActionInvocationState::Stopped);
    assert!(!record.attempt_active);
    assert!(
        state
            .admission
            .conversation(&conversation)
            .unwrap()
            .archived
    );
}
