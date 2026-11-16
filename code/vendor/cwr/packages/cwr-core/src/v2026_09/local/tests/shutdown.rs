#[test]
fn managed_shutdown_kills_and_reaps_a_long_running_adapter() {
    let _environment_lock = SH_ADAPTER_ENV_LOCK.lock().unwrap();
    struct EnvironmentGuard {
        previous: Option<std::ffi::OsString>,
    }

    impl Drop for EnvironmentGuard {
        fn drop(&mut self) {
            if let Some(previous) = self.previous.take() {
                std::env::set_var("CWR_SH_ADAPTER_EXECUTABLE", previous);
            } else {
                std::env::remove_var("CWR_SH_ADAPTER_EXECUTABLE");
            }
        }
    }

    let temporary = tempfile::tempdir().expect("temporary runtime");
    let adapter = temporary.path().join("long-running-adapter");
    let adapter_pid = temporary.path().join("adapter.pid");
    fs::write(
        &adapter,
        format!(
            "#!/bin/sh\nIFS= read -r invocation\nprintf '%s' \"$$\" > '{}'\nexec /bin/sleep 30\n",
            adapter_pid.display()
        ),
    )
    .expect("write long-running adapter");
    let mut permissions = fs::metadata(&adapter)
        .expect("read adapter permissions")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&adapter, permissions).expect("make adapter executable");
    let environment = EnvironmentGuard {
        previous: std::env::var_os("CWR_SH_ADAPTER_EXECUTABLE"),
    };
    std::env::set_var("CWR_SH_ADAPTER_EXECUTABLE", &adapter);

    let workspace = temporary.path().join("workspace");
    copy_workspace(
        fixture_workspace()
            .parent()
            .expect("fixture workspace root"),
        &workspace,
    );
    fs::write(workspace.join("coworkers/helper/integration.toml"),
        "type = 'cwp://org.coworkerprotocol/2026-09/schemas/integration'\n[adapter]\nref = 'cwp://org.coworkerprotocol/2026-09/adapters/sh'\n").unwrap();
    fs::write(workspace.join("coworkers/helper/ROLE.toml"),
        "type = 'cwp://org.coworkerprotocol/2026-09/schemas/role'\n[[adapters]]\nref = 'cwp://org.coworkerprotocol/2026-09/adapters/sh'\n").unwrap();
    let person = load_default_local_acting_actor_ref(workspace.join("CWR.toml")).unwrap();
    let runtime = LocalRuntime::open_with_state_path(
        workspace.join("CWR.toml"),
        temporary.path().join("conversation-admission.json"),
        LocalHostIdentity::new(
            person.as_str(),
            "cwp://org.coworkerprotocol.local/integrations/shutdown-test",
        ),
    )
    .expect("open managed runtime");
    let admission = runtime
        .submit_text_as(
            "cwp://org.coworkerprotocol.local/integrations/shutdown-test",
            "shutdown-test-message",
            person.as_str(),
            None,
            "@helper keep working",
        )
        .expect("admit long-running work");
    let invocation_ref = admission
        .invocation_ref()
        .expect("long-running work has an invocation")
        .to_owned();
    let execution_runtime = runtime.clone();
    let execution = thread::spawn(move || execution_runtime.execute_admission(&admission, |_| {}));

    let marker_deadline = Instant::now() + Duration::from_secs(3);
    let pid = loop {
        if let Some(pid) = fs::read_to_string(&adapter_pid)
            .ok()
            .and_then(|contents| contents.parse::<i32>().ok())
        {
            break pid;
        }
        if Instant::now() >= marker_deadline {
            let _ = runtime.shutdown(Duration::from_secs(2));
            let _ = execution.join();
            panic!("long-running adapter did not publish its pid");
        }
        thread::sleep(Duration::from_millis(10));
    };

    let stopped = runtime
        .shutdown(Duration::from_secs(3))
        .expect("shut down managed runtime");
    assert!(stopped.contains(&invocation_ref));
    execution
        .join()
        .expect("execution worker did not panic")
        .expect("execution settled after stop");

    let state = runtime.lock().expect("inspect settled runtime");
    assert!(state.active.is_empty());
    assert!(state.shutting_down);
    assert_eq!(
        state
            .admission
            .invocation_record(&invocation_ref)
            .expect("retained invocation")
            .state,
        ActionInvocationState::Stopped
    );
    drop(state);
    assert_eq!(
        // SAFETY: signal 0 only queries the captured child PID and retains no process handle.
        unsafe { libc::kill(pid, 0) },
        -1,
        "adapter process still exists"
    );
    assert_eq!(
        io::Error::last_os_error().raw_os_error(),
        Some(libc::ESRCH),
        "adapter process was not reaped"
    );
    drop(environment);
}

fn copy_workspace(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).expect("create copied workspace");
    for entry in fs::read_dir(source).expect("read fixture workspace") {
        let entry = entry.expect("read fixture workspace entry");
        if entry.file_name() == ".cwr" {
            continue;
        }
        let target = destination.join(entry.file_name());
        if entry.file_type().expect("read fixture entry type").is_dir() {
            copy_workspace(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).expect("copy fixture workspace file");
        }
    }
}

#[test]
fn managed_shutdown_stops_connector_owned_work_without_selected_actor_authority() {
    let temporary = tempfile::tempdir().expect("temporary runtime");
    let runtime = LocalRuntime::open_with_state_path(
        fixture_workspace(),
        temporary.path().join("conversation-admission.json"),
        fixture_host_identity(),
    )
    .expect("open managed runtime");
    let admission = runtime
        .admit_ingress(
            ConversationIngress {
                owner_context_ref: None,
                file_refs: Vec::new(),
                integration_ref: "cwp://org.coworkerprotocol.local/integrations/test".into(),
                provider_event_ref: "unauthorized-shutdown-work".into(),
                author_actor_ref: crate::v2026_09::test_fixture::actor_ref("helper").into(),
                conversation: ConversationIngressConversation::Create,
                addressing: ConversationIngressAddressing::Addressed {
                    actor_refs: vec![crate::v2026_09::test_fixture::actor_ref("helper").into()],
                },
                text: "keep working".into(),
            },
            Some(crate::v2026_09::test_fixture::binding_ref("helper")),
        )
        .expect("admit work owned by another actor");
    let invocation_ref = admission.invocation_ref().unwrap().to_owned();
    let control = ProcessAdapterStopControl::new();
    {
        let mut state = runtime.lock().unwrap();
        state
            .admission
            .claim_invocation(&invocation_ref, current_unix_ms())
            .unwrap();
        state.active.insert(invocation_ref.clone(), control.clone());
    }
    let drain_runtime = runtime.clone();
    let drain_control = control.clone();
    let drain_ref = invocation_ref.clone();
    let drain = thread::spawn(move || {
        while !drain_control.is_requested() {
            thread::sleep(Duration::from_millis(1));
        }
        drain_runtime.lock().unwrap().active.remove(&drain_ref);
        drain_runtime.settle_stopped_execution(&drain_ref).unwrap();
    });

    let stopped = runtime
        .shutdown(Duration::from_secs(1))
        .expect("managed lifecycle owner stops connector-owned work");

    drain.join().expect("simulated execution drained");
    assert!(stopped.contains(&invocation_ref));
    assert!(control.is_requested());
    let state = runtime.lock().unwrap();
    assert!(state.shutting_down);
    assert!(state.active.is_empty());
    assert_eq!(
        state
            .admission
            .invocation_record(&invocation_ref)
            .expect("retained invocation")
            .state,
        ActionInvocationState::Stopped
    );
}

#[test]
fn managed_shutdown_terminalizes_admitted_work_before_it_is_claimed() {
    let temporary = tempfile::tempdir().expect("temporary runtime");
    let state_path = temporary.path().join("conversation-admission.json");
    let runtime = LocalRuntime::open_with_state_path(
        fixture_workspace(),
        &state_path,
        fixture_host_identity(),
    )
    .expect("open managed runtime");
    let admission = runtime
        .admit_ingress(
            ConversationIngress {
                owner_context_ref: None,
                file_refs: Vec::new(),
                integration_ref: "cwp://org.coworkerprotocol.local/integrations/test".into(),
                provider_event_ref: "queued-shutdown-work".into(),
                author_actor_ref: crate::v2026_09::test_fixture::actor_ref("helper").into(),
                conversation: ConversationIngressConversation::Create,
                addressing: ConversationIngressAddressing::Addressed {
                    actor_refs: vec![crate::v2026_09::test_fixture::actor_ref("helper").into()],
                },
                text: "queued work".into(),
            },
            Some(crate::v2026_09::test_fixture::binding_ref("helper")),
        )
        .expect("admit queued work");
    let invocation_ref = admission.invocation_ref().unwrap().to_owned();

    let stopped = runtime
        .shutdown(Duration::from_secs(1))
        .expect("shutdown terminalizes queued work");
    assert!(stopped.contains(&invocation_ref));
    assert_eq!(
        runtime
            .lock()
            .unwrap()
            .admission
            .invocation_record(&invocation_ref)
            .expect("retained invocation")
            .state,
        ActionInvocationState::Stopped
    );
    let error = runtime
        .submit_text_as(
            "cwp://org.coworkerprotocol.local/integrations/test",
            "after-shutdown",
            crate::v2026_09::test_fixture::actor_ref("helper"),
            None,
            "@helper more work",
        )
        .unwrap_err();
    assert!(error.to_string().contains("runtime is shutting down"));

    drop(runtime);
    let reopened = LocalRuntime::open_with_state_path(
        fixture_workspace(),
        state_path,
        fixture_host_identity(),
    )
    .expect("reopen managed runtime");
    let executed = reopened
        .execute_pending(|_| {})
        .expect("terminal queued work is not resumed");
    assert!(executed.is_empty());
}

#[test]
fn drain_fences_new_work_and_preserves_it_across_shutdown() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("admission.json");
    let runtime =
        LocalRuntime::open_with_state_path(fixture_workspace(), &path, fixture_host_identity())
            .unwrap();
    assert_eq!(runtime.control_drain("begin").unwrap(), 0);
    let admission = runtime
        .admit_ingress(
            ConversationIngress {
                owner_context_ref: None,
                file_refs: Vec::new(),
                integration_ref: "cwp://org.coworkerprotocol.local/integrations/test".into(),
                provider_event_ref: "during-drain".into(),
                author_actor_ref: crate::v2026_09::test_fixture::actor_ref("helper").into(),
                conversation: ConversationIngressConversation::Create,
                addressing: ConversationIngressAddressing::Addressed {
                    actor_refs: vec![crate::v2026_09::test_fixture::actor_ref("helper").into()],
                },
                text: "wait for restart".into(),
            },
            Some(crate::v2026_09::test_fixture::binding_ref("helper")),
        )
        .unwrap();
    let invocation_ref = admission.invocation_ref().unwrap();
    assert!(runtime
        .claim_execution(invocation_ref, &ProcessAdapterStopControl::default())
        .unwrap()
        .is_none());
    assert_eq!(runtime.control_drain("status").unwrap(), 0);
    runtime.control_drain("finish").unwrap();
    assert!(runtime.shutdown(Duration::from_secs(1)).unwrap().is_empty());
    assert!(runtime
        .lock()
        .unwrap()
        .admission
        .invocation_record(invocation_ref)
        .unwrap()
        .is_runnable());
    drop(runtime);
    let reopened =
        LocalRuntime::open_with_state_path(fixture_workspace(), &path, fixture_host_identity())
            .unwrap();
    assert!(reopened
        .lock()
        .unwrap()
        .admission
        .invocation_record(invocation_ref)
        .unwrap()
        .is_runnable());
}

#[test]
fn cancel_drain_allows_queued_work_to_be_claimed_again() {
    let temporary = tempfile::tempdir().unwrap();
    let runtime = LocalRuntime::open_with_state_path(
        fixture_workspace(),
        temporary.path().join("admission.json"),
        fixture_host_identity(),
    )
    .unwrap();
    let admission = runtime
        .admit_ingress(
            ConversationIngress {
                owner_context_ref: None,
                file_refs: Vec::new(),
                integration_ref: "cwp://org.coworkerprotocol.local/integrations/test".into(),
                provider_event_ref: "before-drain".into(),
                author_actor_ref: crate::v2026_09::test_fixture::actor_ref("helper").into(),
                conversation: ConversationIngressConversation::Create,
                addressing: ConversationIngressAddressing::Addressed {
                    actor_refs: vec![crate::v2026_09::test_fixture::actor_ref("helper").into()],
                },
                text: "queued work".into(),
            },
            Some(crate::v2026_09::test_fixture::binding_ref("helper")),
        )
        .unwrap();
    let invocation_ref = admission.invocation_ref().unwrap();
    assert_eq!(runtime.control_drain("begin").unwrap(), 0);
    assert!(runtime
        .claim_execution(invocation_ref, &ProcessAdapterStopControl::default())
        .unwrap()
        .is_none());
    runtime.control_drain("cancel").unwrap();
    assert!(runtime
        .claim_execution(invocation_ref, &ProcessAdapterStopControl::default())
        .unwrap()
        .is_some());
    assert_eq!(runtime.control_drain("begin").unwrap(), 1);
    assert!(runtime.control_drain("finish").is_err());
    runtime.lock().unwrap().active.clear();
}

#[test]
fn finished_drain_can_be_canceled_before_shutdown() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("admission.json");
    let runtime =
        LocalRuntime::open_with_state_path(fixture_workspace(), &path, fixture_host_identity())
            .unwrap();
    runtime.control_drain("begin").unwrap();
    runtime.control_drain("finish").unwrap();
    runtime.control_drain("cancel").unwrap();
    let state = runtime.lock().unwrap();
    assert!(!state.shutting_down);
    assert!(!state.preserve_pending);
    assert!(state.draining.is_none());
}

#[test]
fn unfinished_work_includes_queued_work_during_drain() {
    let temporary = tempfile::tempdir().unwrap();
    let runtime = LocalRuntime::open_with_state_path(
        fixture_workspace(),
        temporary.path().join("admission.json"),
        fixture_host_identity(),
    )
    .unwrap();
    assert!(!runtime.has_unfinished_work().unwrap());
    let admission = runtime.admit_text(None, "@helper queued work").unwrap();
    assert!(runtime.has_unfinished_work().unwrap());
    runtime.control_drain("begin").unwrap();
    assert!(runtime.has_unfinished_work().unwrap());
    runtime.control_drain("cancel").unwrap();
    runtime
        .stop_managed_invocation(admission.invocation_ref().unwrap())
        .unwrap();
    assert!(!runtime.has_unfinished_work().unwrap());
}
