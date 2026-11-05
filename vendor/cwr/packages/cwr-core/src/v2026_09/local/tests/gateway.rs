struct GatewayTestEnvironment(Option<std::ffi::OsString>);

impl Drop for GatewayTestEnvironment {
    fn drop(&mut self) {
        if let Some(value) = self.0.take() {
            std::env::set_var("CWR_SH_ADAPTER_EXECUTABLE", value);
        } else {
            std::env::remove_var("CWR_SH_ADAPTER_EXECUTABLE");
        }
    }
}

fn gateway_test_workspace(root: &Path) -> PathBuf {
    let workspace = root.join("workspace");
    copy_workspace(fixture_workspace().parent().unwrap(), &workspace);
    let directory = workspace.join("coworkers/gateway");
    fs::create_dir_all(&directory).unwrap();
    fs::write(
        directory.join("GATEWAY.toml"),
        r#"type = "cwp://org.coworkerprotocol/2026-09/schemas/gateway"
role = { ref = "~@/behavior" }
integration = { ref = "~@/execution" }
action_name = "handle--request"
[[imports]]
path = "./ROLE.toml"
as = "behavior"
[[imports]]
path = "./integration.toml"
as = "execution"
"#,
    )
    .unwrap();
    fs::write(directory.join("integration.toml"), "type = 'cwp://org.coworkerprotocol/2026-09/schemas/integration'\n[adapter]\nref = 'cwp://org.coworkerprotocol/2026-09/adapters/sh'\n").unwrap();
    fs::write(
        directory.join("ROLE.toml"),
        r#"type = "cwp://org.coworkerprotocol/2026-09/schemas/role"
[[adapters]]
ref = "cwp://org.coworkerprotocol/2026-09/adapters/sh"
[[actions]]
verb = "handle"
object = "request"
title = "Handle test request"
description = "Test the gateway process boundary"
[actions.input_schema]
path = "./input.json"
[actions.output_schema]
path = "./output.json"
"#,
    )
    .unwrap();
    fs::write(directory.join("input.json"), json!({"type":"object","required":["request","ref","requester_actor_ref","parent_invocation_ref"],"properties":{"request":{"type":"object","additionalProperties":false,"properties":{"value":{"type":"string"},"operation":{"type":"string"},"delivery_ref":{"type":"string"}}}}}).to_string()).unwrap();
    fs::write(directory.join("output.json"), json!({"type":"object","required":["echo","pid","ref","caller"],"properties":{"echo":{"type":"string"}}}).to_string()).unwrap();
    let manifest = workspace.join("CWR.toml");
    let mut contents = fs::read_to_string(&manifest).unwrap();
    contents.push_str(
        r#"
[[imports]]
path = "./coworkers/gateway/GATEWAY.toml"
as = "gateway"
"#,
    );
    fs::write(&manifest, contents).unwrap();
    manifest
}

#[test]
fn gateway_entry_point_preserves_the_authors_conversational_recipient() {
    let temp = tempfile::tempdir().unwrap();
    let manifest = gateway_test_workspace(temp.path());
    let person = load_default_local_acting_actor_ref(&manifest).unwrap();
    let runtime = LocalRuntime::open_with_state_path(
        &manifest,
        temp.path().join("history.sqlite"),
        LocalHostIdentity::new(&person, "cwp://example.test/host"),
    )
    .unwrap();
    let initial = runtime.admit_text(None, "@helper hello").unwrap();
    let conversation = initial.conversation_ref();
    let gateway_ref = {
        let mut state = runtime.lock().unwrap();
        let gateway = &mut state.graph.gateways[0];
        gateway.connection_ref = Some("test-connection".into());
        gateway.ingress_route = Some(gateway.route.clone());
        gateway.resource_ref.clone()
    };
    let admit = || {
        runtime.admit_gateway_action_in_upstream_scope(
        "test-connection", "cwp://example.test/host", "entry-event", &person,
        Some(conversation), "gateway--handle--request",
        json!({"request":{},"ref":"request","requester_actor_ref":person,"parent_invocation_ref":"parent"}), None,
    ).unwrap()
    };
    let entry = admit();
    assert_eq!(entry.invocation_ref(), admit().invocation_ref());
    {
        let state = runtime.lock().unwrap();
        let retained = state.admission.conversation(conversation).unwrap();
        assert!(!retained.participant_actor_refs.contains(&gateway_ref));
        let message = retained.messages.last().unwrap();
        assert_eq!(message.author_actor_ref, person);
        assert!(message.action.is_some());
        assert!(message.addressed_actor_refs.is_empty());
    }
    {
        let mut state = runtime.lock().unwrap();
        state
            .admission
            .claim_invocation(entry.invocation_ref().unwrap(), current_unix_ms())
            .unwrap();
        state
            .admission
            .publish_root_action_completed(
                entry.invocation_ref().unwrap(),
                ActionResponseStatus::COMPLETED,
                json!({"delivered":true}),
                "Entry completed".into(),
                "Completed".into(),
                current_unix_ms(),
            )
            .unwrap();
    }
    {
        let mut state = runtime.lock().unwrap();
        let gateway = &mut state.graph.gateways[0];
        gateway.delivery_route = Some(gateway.route.clone());
        gateway.ingress_route = None;
    }
    let delivery = runtime.admit_gateway_action_in_upstream_scope(
        "test-connection", "cwp://example.test/host", "delivery-event", &person,
        Some(conversation), "gateway--handle--request",
        json!({"request":{},"ref":"request","requester_actor_ref":person,"parent_invocation_ref":"parent"}), None,
    ).unwrap();
    {
        let mut state = runtime.lock().unwrap();
        state
            .admission
            .claim_invocation(delivery.invocation_ref().unwrap(), current_unix_ms())
            .unwrap();
        state
            .admission
            .publish_root_failure(
                delivery.invocation_ref().unwrap(),
                InvocationTerminal {
                    status: ActionResponseStatus::new(502).unwrap(),
                    summary: "Delivery failed".into(),
                    result: None,
                    error: Some(
                        ActionResponseDetail::new("provider_error").with_message("Delivery failed"),
                    ),
                },
                "Delivery failed".into(),
                current_unix_ms(),
            )
            .unwrap();
        let retained = state.admission.conversation(conversation).unwrap();
        assert!(!retained.participant_actor_refs.contains(&gateway_ref));
        for message in &retained.messages {
            assert_ne!(message.author_actor_ref, gateway_ref);
            assert!(!message.addressed_actor_refs.contains(&gateway_ref));
        }
        let results: Vec<_> = retained
            .messages
            .iter()
            .filter(|message| {
                matches!(
                    message.action,
                    Some(crate::v2026_09::conversation::ConversationActionEvent::Result { .. })
                )
            })
            .collect();
        assert_eq!(results.len(), 2);
        assert!(results
            .iter()
            .all(|message| message.author_actor_ref == person));
        assert_eq!(
            results[0].reply_to_message_ref.as_deref(),
            Some(entry.record.source_message_ref.as_str())
        );
        assert_eq!(
            results[1].reply_to_message_ref.as_deref(),
            Some(delivery.record.source_message_ref.as_str())
        );
    }
    let continued = runtime
        .admit_text(Some(conversation), "continue helping")
        .unwrap();
    let state = runtime.lock().unwrap();
    let first = state
        .admission
        .invocation_record(initial.invocation_ref().unwrap())
        .unwrap();
    let next = state
        .admission
        .invocation_record(continued.invocation_ref().unwrap())
        .unwrap();
    assert_eq!(next.target_actor_ref, first.target_actor_ref);
}

fn gateway_sms_parent(
    runtime: &LocalRuntime,
    person: &str,
    event: &str,
    owner: Option<&str>,
) -> ActionInvocationRecord {
    gateway_sms_parent_in_conversation(runtime, person, event, owner, None)
}

fn gateway_sms_parent_in_conversation(
    runtime: &LocalRuntime,
    person: &str,
    event: &str,
    owner: Option<&str>,
    conversation: Option<&str>,
) -> ActionInvocationRecord {
    use crate::v2026_09::conversation::{
        ConversationIngress, ConversationIngressAddressing, ConversationIngressConversation,
    };
    let manager = runtime
        .graph()
        .unwrap()
        .actors
        .iter()
        .find(|a| a.coworker_slug == "helper")
        .unwrap()
        .actor_ref
        .clone();
    let binding = runtime
        .graph()
        .unwrap()
        .actors
        .iter()
        .find(|a| a.actor_ref == manager)
        .unwrap()
        .default_actor_integration_ref
        .clone()
        .unwrap();
    let request = runtime
        .admit_ingress_in_scope(
            ConversationIngress {
                owner_context_ref: None,
                file_refs: Vec::new(),
                integration_ref: "cwp://example.test/sms/account-assignment".into(),
                provider_event_ref: event.into(),
                author_actor_ref: person.into(),
                conversation: conversation
                    .map(
                        |conversation_ref| ConversationIngressConversation::Continue {
                            conversation_ref: conversation_ref.into(),
                        },
                    )
                    .unwrap_or(ConversationIngressConversation::Create),
                addressing: ConversationIngressAddressing::Addressed {
                    actor_refs: vec![manager],
                },
                text: "Synthetic SMS mail request".into(),
            },
            Some(&binding),
            owner,
        )
        .unwrap();
    let mut state = runtime.lock().unwrap();
    let ConversationInvocationClaimOutcome::Claimed(claimed) = state
        .admission
        .claim_invocation(
            request.record.invocation_ref.as_deref().unwrap(),
            current_unix_ms(),
        )
        .unwrap()
    else {
        panic!("SMS manager not claimed")
    };
    let adapter = &claimed.adapter_invocation;
    let context = crate::adapter::working_context(&adapter.thread).unwrap();
    state.admission.append_execution_activity(&adapter.invocation_ref,
        ConversationActivityEvent::ProviderEvent { provider: "codex".into(), method: "cwr.working_state".into(),
            data: json!({"version":1,"idle":true,"provider":"codex","thread_id":event,
                "working_ref":context["working_conversation"]["ref"],"invocation_ref":adapter.invocation_ref,
                "context_snapshot":context["context_snapshot"]}),
        }).unwrap();
    claimed.claim.invocation
}

include!("gateway_authority.rs");
