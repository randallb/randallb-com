//! Run with the unmodified executable built from the pinned pre-change revision.
//! See memos/runbooks/local-ci.md for preparation and invocation.
use super::*;

fn legacy_test() -> GenerationTest {
    let executable = std::env::var_os("CWR_LEGACY_RUNTIME_EXECUTABLE")
        .expect("set CWR_LEGACY_RUNTIME_EXECUTABLE to the pre-change runtime");
    let test = GenerationTest::with_first_executable(std::path::Path::new(&executable));
    assert_ne!(
        test.first_identity, test.second_identity,
        "the old-to-new test requires different binaries"
    );
    test
}

fn assert_legacy_protocol(test: &GenerationTest) {
    let mut reader = test.control();
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    assert!(matches!(
        serde_json::from_str::<RuntimeServiceEvent>(&line).unwrap(),
        RuntimeServiceEvent::Ready { .. }
    ));
    // An unmodified old server closes this connection because it cannot decode
    // the new variant. A second copy of the current binary must fail this check.
    serde_json::to_writer(
        reader.get_mut(),
        &RuntimeServiceCommand::DevGenerationRestart {
            version: RUNTIME_SERVICE_VERSION,
            request_id: "verify-legacy".into(),
            operation: "status".into(),
            generation: test.generation(2),
        },
    )
    .unwrap();
    reader.get_mut().write_all(b"\n").unwrap();
    line.clear();
    assert_eq!(
        reader.read_line(&mut line).unwrap(),
        0,
        "fixture already understands DevGenerationRestart: {line}"
    );
}

fn wait_for_legacy_drain(test: &GenerationTest) {
    wait_until(|| {
        fs::read_to_string(test.directory.path().join("starter-2.log"))
            .is_ok_and(|text| text.contains("Waiting for 1 active tasks"))
    });
}

fn retained(test: &GenerationTest) -> ConversationAdmissionStore {
    ConversationAdmissionStore::open(
        runtime_state_directory(test.directory.path()).join("cwpdb.sqlite3"),
        RUNTIME_REF,
    )
    .unwrap()
}

fn wait_for_result(output: &Receiver<String>, conversation_ref: &str) {
    loop {
        if matches!(receive(output), HostEvent::ConversationExecutionFinished { conversation_ref: completed, .. } if completed == conversation_ref)
        {
            return;
        }
    }
}

#[test]
#[ignore = "requires CWR_LEGACY_RUNTIME_EXECUTABLE built from revision 2f85305f4618"]
fn legacy_generation_replacement_drains_active_work_and_preserves_queued_work() {
    let test = legacy_test();
    test.publish(1);
    let (_old, mut input, output) = test.start(1, false);
    assert!(matches!(
        test.select(&mut input, &output),
        HostEvent::ActorSelected { .. }
    ));
    assert_legacy_protocol(&test);
    let active = test.submit(&mut input, &output, "legacy-active");
    wait_until(|| {
        fs::read_to_string(test.directory.path().join("started")).is_ok_and(|text| !text.is_empty())
    });
    let runtime = runtime_directory(test.directory.path());
    let old_pid = fs::read_to_string(runtime.join("resident-runtime.pid")).unwrap();
    test.publish(2);
    let (_new, mut new_input, new_output) = test.start(2, false);
    write(&mut new_input, &test.selection());
    let (_competitor, mut competing_input, competing_output) = test.start(2, false);
    write(&mut competing_input, &test.selection());
    wait_for_legacy_drain(&test);
    let queued = test.submit(&mut input, &output, "legacy-queued");
    assert_eq!(
        fs::read_to_string(runtime.join("resident-runtime.pid")).unwrap(),
        old_pid
    );
    assert_eq!(
        fs::read_to_string(test.directory.path().join("started"))
            .unwrap()
            .lines()
            .count(),
        1
    );
    fs::write(test.directory.path().join("release"), "finish").unwrap();
    let selected = receive(&new_output);
    assert!(
        matches!(selected, HostEvent::ActorSelected { .. }),
        "{selected:?}"
    );
    assert!(matches!(
        receive(&competing_output),
        HostEvent::ActorSelected { .. }
    ));
    let new_pid = fs::read_to_string(runtime.join("resident-runtime.pid")).unwrap();
    assert_ne!(old_pid, new_pid);
    assert_eq!(
        fs::read_to_string(runtime.join("dev-runtime-generation"))
            .unwrap()
            .trim(),
        test.generation(2)
    );
    let restored = snapshot(&new_output);
    assert!(restored
        .iter()
        .any(
            |conversation| conversation.conversation_ref == active.conversation_ref
                && conversation
                    .messages
                    .iter()
                    .any(|message| message.text.contains("Finished before restart"))
        ));
    assert!(restored
        .iter()
        .any(|conversation| conversation.conversation_ref == queued.conversation_ref));
    if !restored.iter().any(|conversation| {
        conversation.conversation_ref == queued.conversation_ref
            && conversation
                .messages
                .iter()
                .any(|message| message.text.contains("Finished before restart"))
    }) {
        wait_for_result(&new_output, &queued.conversation_ref);
    }
    assert_eq!(
        fs::read_to_string(test.directory.path().join("started"))
            .unwrap()
            .lines()
            .count(),
        2
    );
    let history = retained(&test);
    assert_eq!(
        history
            .invocation_record(active.invocation_ref.as_ref().unwrap())
            .unwrap()
            .state,
        ActionInvocationState::Completed
    );
    assert_eq!(
        history
            .invocation_record(queued.invocation_ref.as_ref().unwrap())
            .unwrap()
            .state,
        ActionInvocationState::Completed
    );
    // The pre-change host cannot emit a typed notice. Its socket closes; this
    // says nothing about the separate, earlier generic TUI disconnect incident.
    loop {
        match output.recv_timeout(Duration::from_secs(10)) {
            Ok(line) => assert!(!matches!(
                serde_json::from_str::<HostEvent>(&line).unwrap(),
                HostEvent::RuntimeRestarting { .. }
            )),
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(error) => panic!("legacy host did not disconnect: {error}"),
        }
    }
    let log = fs::read_to_string(test.directory.path().join("starter-2.log")).unwrap();
    assert!(log.contains("attached hosts must reconnect manually"));
    assert_eq!(
        fs::read_to_string(runtime.join("resident-runtime.pid")).unwrap(),
        new_pid
    );
}

#[test]
#[ignore = "requires CWR_LEGACY_RUNTIME_EXECUTABLE built from revision 2f85305f4618"]
fn legacy_generation_replacement_releases_the_drain_when_superseded() {
    let test = legacy_test();
    test.publish(1);
    let (_old, mut input, output) = test.start(1, false);
    assert!(matches!(
        test.select(&mut input, &output),
        HostEvent::ActorSelected { .. }
    ));
    assert_legacy_protocol(&test);
    let active = test.submit(&mut input, &output, "legacy-before-newer-build");
    wait_until(|| {
        fs::read_to_string(test.directory.path().join("started")).is_ok_and(|text| !text.is_empty())
    });
    let runtime = runtime_directory(test.directory.path());
    let old_pid = fs::read_to_string(runtime.join("resident-runtime.pid")).unwrap();
    test.publish(2);
    let (_next, mut next_input, next_output) = test.start(2, false);
    write(&mut next_input, &test.selection());
    wait_for_legacy_drain(&test);
    let queued = test.submit(&mut input, &output, "legacy-queued-before-newer-build");
    test.publish(3);
    fs::write(test.directory.path().join("release"), "finish").unwrap();
    assert!(
        matches!(receive(&next_output), HostEvent::CommandFailed { error, .. } if error.contains("rebuilt again"))
    );
    assert_eq!(
        fs::read_to_string(runtime.join("resident-runtime.pid")).unwrap(),
        old_pid
    );
    assert_eq!(
        fs::read_to_string(runtime.join("dev-runtime-generation"))
            .unwrap()
            .trim(),
        test.generation(1)
    );
    wait_for_result(&output, &queued.conversation_ref);
    let history = retained(&test);
    assert_eq!(
        history
            .invocation_record(queued.invocation_ref.as_ref().unwrap())
            .unwrap()
            .state,
        ActionInvocationState::Completed
    );
    assert_eq!(
        history
            .invocation_record(active.invocation_ref.as_ref().unwrap())
            .unwrap()
            .state,
        ActionInvocationState::Completed
    );
}
