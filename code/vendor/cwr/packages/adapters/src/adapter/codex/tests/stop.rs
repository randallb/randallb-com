#[test]
fn generic_provider_tool_errors_are_returned_without_dispatching_invalid_input() {
    let (directory, fake_codex) = fixture(FAKE_GENERIC_TOOL_ERRORS_CODEX);
    let mut requested = invocation(&fake_codex);
    requested.host_operations.push(dynamic_operation(
        "cwr.start_action",
        "Start one declared action.",
        json!({
            "type": "object",
            "additionalProperties": false,
            "required": ["action_name"],
            "properties": {"action_name": {"type": "string"}}
        }),
        json!({"type": "object"}),
    ));
    let mut dispatched = Vec::new();

    let outcome = run_process_adapter_with_controlled_host_calls(
        &codex_adapter(directory.path()),
        requested,
        &ProcessAdapterStopControl::new(),
        |_| {},
        |call| {
            dispatched.push(call);
            Err(std::io::Error::other("host rejected start").into())
        },
    )
    .expect("return generic provider tool failures");

    assert_eq!(dispatched.len(), 1);
    assert_eq!(dispatched[0].call_id, "rejected-1");
    assert!(matches!(outcome, ProcessAdapterOutcome::Completed(_)));
}
#[test]
fn stopping_codex_adapter_terminates_the_provider_process_tree() {
    let (directory, fake_codex) = fixture(FAKE_SLOW_CODEX);
    let requested = invocation(&fake_codex);
    let stop = ProcessAdapterStopControl::new();
    let stop_after_progress = stop.clone();
    let started = Instant::now();
    let outcome = run_process_adapter(&codex_adapter(directory.path()), requested, &stop, |_| {
        stop_after_progress.stop()
    })
    .expect("stop standalone Codex adapter");
    assert_eq!(outcome, ProcessAdapterOutcome::Stopped);
    assert!(
        started.elapsed().as_secs() < 5,
        "stopping the adapter should terminate its Codex child"
    );
}
