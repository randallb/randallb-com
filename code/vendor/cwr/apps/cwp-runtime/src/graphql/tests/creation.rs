//! Creation and continuation have distinct destinations and retry identities.
use super::*;

#[test]
fn create_conversation_and_send_message_have_distinct_destinations() {
    use cwr::v2026_09::upstream::ConversationContextPolicy;
    let directory = tempfile::tempdir().unwrap();
    let runtime = runtime(directory.path());
    let workspace = "cwp://example.test/contexts/workspace";
    let pack = "cwp://example.test/contexts/pack";
    let disabled = "cwp://example.test/contexts/disabled";
    let helper = crate::test_refs::actor_ref("helper");
    runtime
        .configure_conversation_contexts(Some(ConversationContextPolicy {
            action_permissions: Vec::new(),
            suspended: false,
            default_context_ref: workspace.into(),
            contexts: std::collections::BTreeMap::from([
                (workspace.into(), vec![helper.clone()]),
                (pack.into(), vec![helper]),
                (disabled.into(), vec![]),
            ]),
        }))
        .unwrap();
    let server = serve(directory.path(), runtime);
    let client = client(&server.endpoint);
    let person = session_endpoint(&client, &server.endpoint, None);
    let create = "mutation($owner:ID!){createConversation(text:\"@helper Pack task\",requestId:\"create-owned\",ownerContextRef:$owner){conversationRef messageRef conversation{ownerContextRef}}}";
    let first = query_session(&client, &person, create, json!({"owner":pack}));
    assert!(first.get("errors").is_none(), "{first}");
    assert_eq!(
        first["data"]["createConversation"]["conversation"]["ownerContextRef"],
        pack
    );
    let replay = query_session(&client, &person, create, json!({"owner":pack}));
    assert_eq!(first["data"], replay["data"]);
    let changed = query_session(&client, &person, create, json!({"owner":workspace}));
    assert!(changed.get("errors").is_some(), "{changed}");
    let denied = query_session(
        &client,
        &person,
        create.replace("create-owned", "disabled-create").as_str(),
        json!({"owner":disabled}),
    );
    assert!(denied.get("errors").is_some(), "{denied}");
    let missing = query_session(
        &client,
        &person,
        "mutation{sendMessage(text:\"hello\",requestId:\"missing-conversation\"){conversationRef}}",
        json!({}),
    );
    assert!(missing.get("errors").is_some(), "{missing}");
    let continued = query_session(
        &client,
        &person,
        "mutation($ref:ID!){sendMessage(text:\"@helper Follow up\",requestId:\"continue-owned\",conversationRef:$ref){conversationRef conversation{ownerContextRef}}}",
        json!({"ref":first["data"]["createConversation"]["conversationRef"]}),
    );
    assert!(continued.get("errors").is_none(), "{continued}");
    assert_eq!(
        continued["data"]["sendMessage"]["conversationRef"],
        first["data"]["createConversation"]["conversationRef"]
    );
    assert_eq!(
        continued["data"]["sendMessage"]["conversation"]["ownerContextRef"],
        pack
    );
    for mutation in [
        "mutation{createConversation(text:\"hello\",requestId:\"bad-create\",conversationRef:\"existing\"){conversationRef}}",
        "mutation{sendMessage(text:\"hello\",requestId:\"bad-send\",conversationRef:\"existing\",ownerContextRef:\"other\"){conversationRef}}",
        "mutation{sendMessage(text:\"hello\",requestId:\"empty-send\",conversationRef:\" \"){conversationRef}}",
    ] {
        assert!(
            query_session(&client, &person, mutation, json!({}))
                .get("errors")
                .is_some()
        );
    }
}
