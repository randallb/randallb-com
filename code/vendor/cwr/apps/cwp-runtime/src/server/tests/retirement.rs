use super::*;
use crate::server::retirement as resident_retirement;

fn attempt(state: &SharedState, writer: &SharedWriter, nonce: &str) -> CwrResult<()> {
    let expected_runtime_ref = lock_state(state)?.runtime_ref.clone();
    execute_command(
        state,
        writer,
        RuntimeServiceCommand::RetireConfiguredResident {
            version: RUNTIME_SERVICE_VERSION,
            request_id: "retire-fixture".into(),
            nonce: nonce.into(),
            expected_runtime_ref,
            expected_owner_pid: std::process::id() as i32,
        },
    )
}

#[test]
fn retirement_requires_configured_ownership_and_preserves_existing_drains() {
    let temporary = tempfile::tempdir().unwrap();
    let state = runtime_state(&temporary);
    let (_client, server) = wire::wire_pair();
    let writer = client_writer(server).unwrap();
    let nonce = uuid::Uuid::new_v4().to_string();
    assert!(attempt(&state, &writer, &nonce).is_err());
    lock_state(&state).unwrap().retirement_allowed = true;
    assert!(attempt(&state, &writer, "not-a-nonce").is_err());
    let runtime_ref = lock_state(&state).unwrap().runtime_ref.clone();
    for (expected_runtime_ref, expected_owner_pid) in [
        (
            "cwp://local.other/runtime".to_string(),
            std::process::id() as i32,
        ),
        (runtime_ref, std::process::id() as i32 + 1),
    ] {
        assert!(execute_command(
            &state,
            &writer,
            RuntimeServiceCommand::RetireConfiguredResident {
                version: RUNTIME_SERVICE_VERSION,
                request_id: "wrong-owner".into(),
                nonce: nonce.clone(),
                expected_runtime_ref,
                expected_owner_pid,
            }
        )
        .is_err());
        assert!(lock_state(&state).unwrap().committed_retirement.is_none());
    }

    lock_state(&state)
        .unwrap()
        .runtime
        .control_drain("begin")
        .unwrap();
    assert!(attempt(&state, &writer, &nonce).is_err());
    assert!(lock_state(&state).unwrap().committed_retirement.is_none());
    lock_state(&state)
        .unwrap()
        .runtime
        .control_drain("cancel")
        .unwrap();
    lock_state(&state).unwrap().controls_finalizing = true;
    assert!(attempt(&state, &writer, &nonce).is_err());
}

#[test]
fn committed_retirement_cannot_be_cancelled_and_sends_no_generation_notice() {
    let temporary = tempfile::tempdir().unwrap();
    let state = runtime_state(&temporary);
    lock_state(&state).unwrap().retirement_allowed = true;
    let notifications = state.activity.restart_notifications();
    let (client, server) = wire::wire_pair();
    let writer = client_writer(server).unwrap();
    let nonce = uuid::Uuid::new_v4().to_string();
    attempt(&state, &writer, &nonce).unwrap();
    let events: Vec<RuntimeServiceEvent> = BufReader::new(client)
        .lines()
        .map(|line| serde_json::from_str(&line.unwrap()).unwrap())
        .collect();
    assert_eq!(events.len(), 1);
    assert!(
        matches!(&events[0], RuntimeServiceEvent::CommandSucceeded {request_id, ..} if request_id == "retire-fixture")
    );
    assert!(notifications.borrow().is_none());
    let receipt = lock_state(&state)
        .unwrap()
        .committed_retirement
        .clone()
        .unwrap();
    assert_eq!(receipt.nonce, nonce);
    assert_eq!(receipt.owner_pid, std::process::id() as i32);
    assert!(execute_command(
        &state,
        &writer,
        RuntimeServiceCommand::Drain {
            version: RUNTIME_SERVICE_VERSION,
            request_id: "cancel".into(),
            operation: "cancel".into()
        }
    )
    .is_err());
    assert!(
        restart::control(&state, &writer, "dev".into(), "begin", "development".into()).is_err()
    );
    assert!(attempt(&state, &writer, &uuid::Uuid::new_v4().to_string()).is_err());
    assert!(lock_state(&state)
        .unwrap()
        .runtime
        .control_drain("cancel")
        .is_err());
}

#[test]
fn lost_retirement_acknowledgement_never_releases_the_committed_fence() {
    let temporary = tempfile::tempdir().unwrap();
    let state = runtime_state(&temporary);
    lock_state(&state).unwrap().retirement_allowed = true;
    let (client, server) = wire::wire_pair();
    let writer = client_writer(server).unwrap();
    drop(client);
    let nonce = uuid::Uuid::new_v4().to_string();
    let _ = attempt(&state, &writer, &nonce);
    unsubscribe(&state, &writer);
    let locked = lock_state(&state).unwrap();
    assert_eq!(locked.committed_retirement.as_ref().unwrap().nonce, nonce);
    assert!(locked.runtime.control_drain("cancel").is_err());
}

#[test]
fn busy_retirement_restores_admission_without_committing() {
    let temporary = tempfile::tempdir().unwrap();
    let state = runtime_state(&temporary);
    lock_state(&state).unwrap().retirement_allowed = true;
    let runtime = lock_state(&state).unwrap().runtime.clone();
    // Intrinsic control execution uses no provider process or credentials.
    let admission = runtime
        .submit_text_as(
            "cwp://example.test/integrations/retirement",
            "busy-fixture",
            crate::test_refs::actor_ref("person").as_str(),
            None,
            "@cwr list actions",
        )
        .unwrap();
    let (started, ready) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel();
    let worker_runtime = runtime.clone();
    let worker = std::thread::spawn(move || {
        worker_runtime.execute_admission(&admission, |event| {
            if matches!(event, LocalRuntimeEvent::InvocationStarted { .. }) {
                let _ = started.send(());
                let _ = released.recv_timeout(Duration::from_secs(5));
            }
        })
    });
    ready.recv_timeout(Duration::from_secs(3)).unwrap();
    let (_client, server) = wire::wire_pair();
    let writer = client_writer(server).unwrap();
    let result = attempt(&state, &writer, &uuid::Uuid::new_v4().to_string());
    // Unblock before assertions so a failing assertion cannot strand the fixture.
    release.send(()).unwrap();
    let _ = worker.join().unwrap();
    assert!(result.unwrap_err().to_string().contains("busy"));
    assert!(lock_state(&state).unwrap().committed_retirement.is_none());
    assert!(runtime
        .submit_text_as(
            "cwp://example.test/integrations/retirement",
            "after-busy-fixture",
            crate::test_refs::actor_ref("person").as_str(),
            None,
            "@cwr list actions"
        )
        .is_ok());
    runtime.shutdown(Duration::from_secs(3)).unwrap();
}

#[test]
fn arming_is_immutable_across_disconnect_and_finalizes_only_before_cleanup() {
    let temporary = tempfile::tempdir().unwrap();
    let state = runtime_state(&temporary);
    lock_state(&state).unwrap().retirement_allowed = true;
    let (client, server) = wire::wire_pair();
    let writer = client_writer(server).unwrap();
    let nonce = uuid::Uuid::new_v4().to_string();
    let runtime_ref = lock_state(&state).unwrap().runtime_ref.clone();
    for _ in 0..2 {
        resident_retirement::arm(
            &state,
            &writer,
            "arm".into(),
            nonce.clone(),
            runtime_ref.clone(),
            std::process::id() as i32,
        )
        .unwrap();
    }
    assert!(resident_retirement::arm(
        &state,
        &writer,
        "other".into(),
        uuid::Uuid::new_v4().to_string(),
        runtime_ref,
        std::process::id() as i32
    )
    .is_err());
    assert!(drain::control(&state, &writer, "cancel".into(), "cancel").is_err());
    assert!(
        restart::control(&state, &writer, "dev".into(), "begin", "development".into()).is_err()
    );
    drop(client);
    unsubscribe(&state, &writer);
    let mut locked = lock_state(&state).unwrap();
    assert_eq!(locked.armed_retirement.as_ref().unwrap().nonce, nonce);
    assert!(
        !locked.runtime.has_drain_or_shutdown().unwrap(),
        "arming must not fence service work"
    );
    let receipt = resident_retirement::freeze(&mut locked).unwrap();
    assert_eq!(receipt.nonce, nonce);
    assert!(locked.runtime.control_drain("cancel").is_err());
    assert!(
        !temporary
            .path()
            .join("resident-retirement-v1.json")
            .exists(),
        "freeze is not successful cleanup evidence"
    );
}

#[test]
fn busy_armed_finalization_never_retrofits_a_receipt_after_work_finishes() {
    let temporary = tempfile::tempdir().unwrap();
    let state = runtime_state(&temporary);
    lock_state(&state).unwrap().retirement_allowed = true;
    let (_client, server) = wire::wire_pair();
    let writer = client_writer(server).unwrap();
    let nonce = uuid::Uuid::new_v4().to_string();
    let runtime_ref = lock_state(&state).unwrap().runtime_ref.clone();
    resident_retirement::arm(
        &state,
        &writer,
        "arm".into(),
        nonce,
        runtime_ref,
        std::process::id() as i32,
    )
    .unwrap();
    let runtime = lock_state(&state).unwrap().runtime.clone();
    let admission = runtime
        .submit_text_as(
            "cwp://example.test/integrations/retirement",
            "busy-arm",
            crate::test_refs::actor_ref("person").as_str(),
            None,
            "@cwr list actions",
        )
        .unwrap();
    let (started, ready) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel();
    let worker_runtime = runtime.clone();
    let worker = std::thread::spawn(move || {
        worker_runtime.execute_admission(&admission, |event| {
            if matches!(event, LocalRuntimeEvent::InvocationStarted { .. }) {
                let _ = started.send(());
                let _ = released.recv_timeout(Duration::from_secs(5));
            }
        })
    });
    ready.recv_timeout(Duration::from_secs(3)).unwrap();
    let frozen = resident_retirement::freeze(&mut lock_state(&state).unwrap());
    release.send(()).unwrap();
    let _ = worker.join().unwrap();
    assert!(frozen.is_none());
    assert!(resident_retirement::freeze(&mut lock_state(&state).unwrap()).is_none());
    runtime.shutdown(Duration::from_secs(3)).unwrap();
}
