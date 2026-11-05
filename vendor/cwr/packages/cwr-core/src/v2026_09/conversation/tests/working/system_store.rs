use super::*;

#[test]
fn system_codex_followup_resumes_the_same_private_history() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = ConversationAdmissionStore::open(
        directory.path().join("system-provider.sqlite"),
        "cwp://com.example.runtime",
    )
    .unwrap();
    let first = working_turn(&mut store, None, "first request");
    let claimed = claim_working(&mut store, &first);
    let adapter = &claimed.adapter_invocation;
    let source = crate::adapter::working_context(&adapter.thread).unwrap();
    let working_ref = source["working_conversation"]["ref"].clone();
    store
        .append_execution_activity(
            &adapter.invocation_ref,
            ConversationActivityEvent::ProviderEvent {
                provider: "codex".into(),
                method: "cwr.working_state".into(),
                data: json!({
                    "version":1,
                    "idle":true,
                    "provider":"codex",
                    "provider_store":"system",
                    "thread_id":"system-provider-thread",
                    "working_ref":working_ref,
                    "invocation_ref":adapter.invocation_ref,
                    "context_snapshot":source["context_snapshot"],
                }),
            },
        )
        .unwrap();
    store
        .complete(&adapter.invocation_ref, json!({"text":"Done"}))
        .unwrap();

    let next = working_turn(
        &mut store,
        Some(&first.record.conversation_ref),
        "followup request",
    );
    let followup = claim_working(&mut store, &next);
    let working = &followup.adapter_invocation.thread["working_conversation"];
    assert!(working.get("history_gap").is_none());
    assert_eq!(working["ref"], working_ref);
    assert_eq!(working["state"]["provider_store"], "system");
    assert_eq!(working["state"]["thread_id"], "system-provider-thread");
}
