use super::*;

#[test]
fn host_stdio_rejects_an_unterminated_actor_selection() {
    let temporary = tempfile::tempdir().expect("temporary runtime configuration");
    let (mut child, mut input, _output) = start_runtime(temporary.path());
    let command = HostCommand::SelectActor {
        version: HOST_COMMAND_VERSION,
        request_id: "unterminated-selection".into(),
        actor_ref: crate::test_refs::actor_ref("person").as_str().into(),
    };
    serde_json::to_writer(&mut input, &command).expect("encode unterminated selection");
    drop(input);

    assert!(!child
        .0
        .wait()
        .expect("wait for rejected host input")
        .success());
    let runtime = runtime_directory(temporary.path());
    assert!(!runtime.join("resident-runtime.pid").exists());
    assert!(!runtime.join("host.sock").exists());
}

#[test]
fn host_stdio_accepts_a_host_command_larger_than_8192_bytes() {
    let temporary = tempfile::tempdir().expect("temporary runtime configuration");
    let (mut child, mut input, output) = start_runtime(temporary.path());
    send(
        &mut input,
        &output,
        HostCommand::SelectActor {
            version: HOST_COMMAND_VERSION,
            request_id: "select-for-large-command".into(),
            actor_ref: crate::test_refs::actor_ref("person").as_str().into(),
        },
    );

    let result = send(
        &mut input,
        &output,
        HostCommand::CreateConversation {
            version: HOST_COMMAND_VERSION,
            request_id: "large-host-command".into(),
            source_ref: SOURCE_REF.into(),
            author_actor_ref: crate::test_refs::actor_ref("person").as_str().into(),
            text: "x".repeat(16 * 1024),
        },
    );

    assert!(matches!(result, HostEvent::CommandSucceeded { .. }));
    drop(input);
    assert!(child.0.wait().expect("wait for host runtime").success());
    stop_resident(temporary.path());
}
