#[test]
fn generation_restart_probe_requires_an_explicit_legacy_rejection() {
    use std::io::{BufRead as _, BufReader, Write as _};
    use std::os::unix::net::UnixStream;
    for (response, expected) in [
        (
            Some(
                json!({"kind": "drain_status", "request_id": "dev-generation-probe", "remaining": 0}),
            ),
            Some(true),
        ),
        (
            Some(
                json!({"kind": "command_failed", "request_id": "dev-generation-probe", "error": "unknown runtime drain operation"}),
            ),
            Some(false),
        ),
        (
            Some(
                json!({"kind": "command_failed", "request_id": "dev-generation-probe", "error": "runtime shutdown has already begun"}),
            ),
            None,
        ),
        (None, None),
    ] {
        let (client, server) = UnixStream::pair().unwrap();
        let responder = thread::spawn(move || {
            let mut server = BufReader::new(server);
            let mut line = String::new();
            server.read_line(&mut line).unwrap();
            let command: Value = serde_json::from_str(&line).unwrap();
            assert_eq!(command["kind"], "drain");
            assert_eq!(command["operation"], "supports-generation-restart");
            if let Some(response) = response {
                serde_json::to_writer(server.get_mut(), &response).unwrap();
                server.get_mut().write_all(b"\n").unwrap();
            }
        });
        assert_eq!(
            supports_generation_restart(&mut BufReader::new(client)).ok(),
            expected
        );
        responder.join().unwrap();
    }
}

#[test]
fn generation_drain_allows_active_work_to_start_its_children() {
    let temporary = tempfile::tempdir().unwrap();
    let runtime = LocalRuntime::open_with_state_path(
        fixture_workspace(),
        temporary.path().join("cwpdb.sqlite3"),
        fixture_host_identity(),
    )
    .unwrap();
    let admission = runtime.admit_text(None, "@helper coordinate").unwrap();
    let parent_ref = admission.invocation_ref().unwrap().to_owned();
    let caller = {
        let mut state = runtime.lock().unwrap();
        let ConversationInvocationClaimOutcome::Claimed(claimed) =
            state.admission.claim_invocation(&parent_ref, 10).unwrap()
        else {
            panic!("claim parent")
        };
        claimed.claim.invocation
    };
    assert_eq!(runtime.control_drain("begin_generation").unwrap(), 1);
    let call = AdapterCall {
        invocation_ref: parent_ref.clone(),
        call_id: "during-drain".into(),
        operation: START_ACTION_HOST_OPERATION.into(),
        input: serde_json::to_value(StartActionInput {
            activation: "@cwr--list--actions".into(),
            input: json!({}),
        })
        .unwrap(),
    };
    let AdapterHostCallOutcome::Returned(result) = runtime
        .handle_start_action(&call, &caller, &Rc::new(RefCell::new(VecDeque::new())))
        .unwrap();
    let child: StartActionResult = serde_json::from_value(result).unwrap();
    assert!(runtime
        .claim_execution(&child.invocation_ref, &ProcessAdapterStopControl::default())
        .unwrap()
        .is_some());
    assert_eq!(runtime.control_drain("status").unwrap(), 2);
    assert!(runtime.control_drain("restart").is_err());
    // These claims have no worker; release their synthetic active controls.
    runtime.lock().unwrap().active.clear();
    runtime.shutdown(Duration::from_secs(1)).unwrap();
}

#[test]
fn committed_generation_drain_cannot_be_cancelled_or_admit_more_work() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("cwpdb.sqlite3");
    let runtime =
        LocalRuntime::open_with_state_path(fixture_workspace(), &path, fixture_host_identity())
            .unwrap();
    let queued = runtime.admit_text(None, "@helper queued").unwrap();
    assert_eq!(runtime.control_drain("begin_generation").unwrap(), 0);
    assert!(runtime.control_drain("begin_generation").is_err());
    runtime.control_drain("restart").unwrap();
    assert!(runtime.control_drain("cancel").is_err());
    assert!(runtime.admit_text(None, "@helper too late").is_err());
    assert!(runtime
        .claim_execution(
            queued.invocation_ref().unwrap(),
            &ProcessAdapterStopControl::default()
        )
        .unwrap()
        .is_none());
    assert!(runtime.shutdown(Duration::from_secs(1)).unwrap().is_empty());
    drop(runtime);
    let reopened =
        LocalRuntime::open_with_state_path(fixture_workspace(), &path, fixture_host_identity())
            .unwrap();
    assert!(reopened
        .claim_execution(
            queued.invocation_ref().unwrap(),
            &ProcessAdapterStopControl::default()
        )
        .unwrap()
        .is_some());
    reopened.lock().unwrap().active.clear();
    reopened.shutdown(Duration::from_secs(1)).unwrap();
}
