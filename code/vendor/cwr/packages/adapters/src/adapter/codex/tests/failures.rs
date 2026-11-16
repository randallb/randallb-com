#[test]
fn oversized_provider_frame_returns_the_cause_in_the_terminal_result() {
    let script = r#"#!/bin/sh
IFS= read -r line
/usr/bin/head -c 67108865 /dev/zero | /usr/bin/tr '\000' A
"#;
    let (directory, fake_codex) = fixture(script);
    let outcome = run_process_adapter(
        &codex_adapter(directory.path()),
        invocation(&fake_codex),
        &ProcessAdapterStopControl::new(),
        |_| {},
    ).expect("structured failure instead of adapter exit");
    let ProcessAdapterOutcome::Completed(failure) = outcome else { panic!("terminal failure") };
    assert_eq!(failure.status, 500);
    assert!(failure.summary.contains("64 MiB limit"));
    assert!(!failure.summary.contains("exit status"));
    assert!(failure.result.is_none());
    let error = failure.error.unwrap();
    assert_eq!(error.code, "provider_frame_too_large");
    assert_eq!(error.message.as_deref(), Some(failure.summary.as_str()));
}

#[test]
fn provider_start_error_preserves_message_without_auxiliary_payload() {
    let script = r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*)
      printf '%s\n' '{"id":"cwr-initialize","result":{}}' ;;
    *'"method":"thread/start"'*)
      printf '%s\n' '{"id":"cwr-thread-start","error":{"message":"Please sign in again.","data":"private auxiliary payload"}}' ;;
  esac
done
"#;
    let (directory, fake_codex) = fixture(script);
    let outcome = run_process_adapter(
        &codex_adapter(directory.path()),
        invocation(&fake_codex),
        &ProcessAdapterStopControl::new(),
        |_| {},
    ).expect("structured provider error");
    let ProcessAdapterOutcome::Completed(failure) = outcome else { panic!("terminal failure") };
    assert_eq!(failure.status, 500);
    assert_eq!(failure.summary, "Codex could not start the conversation: Please sign in again.");
    let error = failure.error.unwrap();
    assert_eq!(error.code, "codex_execution_failed");
    assert!(error.details.is_none());
}

#[test]
fn live_but_silent_provider_becomes_a_retryable_terminal_failure() {
    let script = r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*)
      printf '%s\n' '{"id":"cwr-initialize","result":{}}' ;;
    *'"method":"initialized"'*) ;;
    *'"method":"thread/start"'*)
      printf '%s\n' '{"id":"cwr-thread-start","result":{"thread":{"id":"silent-thread"}}}' ;;
    *'"method":"thread/inject_items"'*)
      printf '%s\n' '{"id":"cwr-observation-inject","result":{}}' ;;
    *'"method":"turn/start"'*)
      printf '%s\n' '{"id":"cwr-turn-start","result":{"turn":{"id":"silent-turn"}}}'
      while true; do sleep 1; done ;;
  esac
done
"#;
    let (directory, fake_codex) = fixture(script);
    let adapter = codex_adapter(directory.path()).with_environment(
        "CWR_CODEX_PROVIDER_INACTIVITY_TIMEOUT_MS",
        "75",
    );
    let started = Instant::now();

    let outcome = run_process_adapter(
        &adapter,
        invocation(&fake_codex),
        &ProcessAdapterStopControl::new(),
        |_| {},
    )
    .expect("silent provider must return a structured failure");

    assert!(started.elapsed() < Duration::from_secs(2));
    let ProcessAdapterOutcome::Completed(failure) = outcome else {
        panic!("terminal failure")
    };
    assert_eq!(failure.status, 504);
    assert_eq!(
        failure.summary,
        "Codex stopped responding. Check your connection or VPN, then try again."
    );
    assert!(failure.result.is_none());
    let error = failure.error.expect("structured timeout error");
    assert_eq!(error.code, "provider_timeout");
    assert_eq!(error.message.as_deref(), Some(failure.summary.as_str()));
    assert_eq!(
        error.details,
        Some(json!({"inactivity_timeout_ms": 75}))
    );
}
