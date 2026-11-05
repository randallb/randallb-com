#[test]
fn synchronous_nested_action_deadline_stops_the_child_and_returns() {
    struct EnvironmentGuard(Option<std::ffi::OsString>);
    impl Drop for EnvironmentGuard {
        fn drop(&mut self) {
            if let Some(previous) = self.0.take() {
                std::env::set_var("CWR_SH_ADAPTER_EXECUTABLE", previous);
            } else {
                std::env::remove_var("CWR_SH_ADAPTER_EXECUTABLE");
            }
        }
    }

    let _environment_lock = SH_ADAPTER_ENV_LOCK.lock().unwrap();
    let temporary = tempfile::tempdir().unwrap();
    let adapter = temporary.path().join("long-running-adapter");
    fs::write(
        &adapter,
        "#!/bin/sh\nIFS= read -r invocation\nexec /bin/sleep 30\n",
    )
    .unwrap();
    let mut permissions = fs::metadata(&adapter).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&adapter, permissions).unwrap();
    let _environment = EnvironmentGuard(std::env::var_os("CWR_SH_ADAPTER_EXECUTABLE"));
    std::env::set_var("CWR_SH_ADAPTER_EXECUTABLE", &adapter);

    let workspace = temporary.path().join("workspace");
    copy_workspace(fixture_workspace().parent().unwrap(), &workspace);
    fs::write(
        workspace.join("coworkers/helper/integration.toml"),
        "type = 'cwp://org.coworkerprotocol/2026-09/schemas/integration'\n[adapter]\nref = 'cwp://org.coworkerprotocol/2026-09/adapters/sh'\n",
    )
    .unwrap();
    fs::write(
        workspace.join("coworkers/helper/ROLE.toml"),
        "type = 'cwp://org.coworkerprotocol/2026-09/schemas/role'\nname = 'Helper'\n[[adapters]]\nref = 'cwp://org.coworkerprotocol/2026-09/adapters/sh'\n",
    )
    .unwrap();
    let cwr_toml = workspace.join("CWR.toml");
    let person = load_default_local_acting_actor_ref(&cwr_toml).unwrap();
    let runtime = LocalRuntime::open_with_state_path(
        cwr_toml,
        temporary.path().join("runtime.sqlite"),
        LocalHostIdentity::new(
            person,
            "cwp://org.coworkerprotocol.local/integrations/deadline-test",
        ),
    )
    .unwrap();
    let reviewer = add_participation_actor(&runtime, "deadline-reviewer");
    let caller = claimed_participation_parent(&runtime);
    let target = {
        let state = runtime.lock().unwrap();
        exact_action_target(
            &state,
            state
                .graph
                .routes
                .iter()
                .find(|route| route.actor_ref == reviewer)
                .unwrap(),
        )
    };
    let call = AdapterCall {
        invocation_ref: caller.invocation_ref.clone(),
        call_id: "deadline-action".into(),
        operation: INVOKE_ACTION_HOST_OPERATION.into(),
        input: json!({"target":target, "input":{}, "timeout_ms":100}),
    };
    let started = Instant::now();
    let outcome = runtime
        .handle_host_call(
            call,
            &caller,
            &Rc::new(RefCell::new(VecDeque::new())),
            &AdapterModelOutputMailbox::discard(),
            &mut |_| {},
        )
        .unwrap();
    assert!(started.elapsed() < Duration::from_secs(2));
    let AdapterHostCallOutcome::Returned(response) = outcome;
    let response: ActionResponse = serde_json::from_value(response).unwrap();
    assert_eq!(response.status.as_u16(), 499);
    let state = runtime.lock().unwrap();
    assert!(state.active.is_empty());
    assert_eq!(
        state
            .admission
            .invocation_record(&response.invocation_ref)
            .unwrap()
            .state,
        ActionInvocationState::Stopped
    );
}
