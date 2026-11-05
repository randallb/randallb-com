use super::*;

#[test]
fn workspace_action_uses_bound_runtime_and_still_checks_actor_and_explicit_source() {
    let output = output::host_writer_for(io::sink()).unwrap();
    let pending = pending::registry();
    let actor = crate::test_refs::actor_ref("person");
    let command =
        |request: &str, author: &str, source: Option<&str>, conversation: Option<&str>| {
            HostCommand::InvokeAction {
                version: cwr::v2026_09::host::HOST_COMMAND_VERSION,
                request_id: request.into(),
                source_ref: source.map(str::to_owned),
                author_actor_ref: author.into(),
                conversation_ref: conversation.map(str::to_owned),
                activation: "@workloop_ops--disconnect--mail".into(),
                input: serde_json::json!({"provider":"imap", "expected_workspace":"/workspace"}),
            }
        };
    let admitted = translate_host_command(
        command("source-free", actor.as_str(), None, None),
        SOURCE_REF,
        WORKLOOP_DESKTOP_INGRESS_REF,
        Some(actor.as_str()),
        &output,
        &pending,
    )
    .unwrap()
    .unwrap();
    assert!(matches!(admitted, RuntimeServiceCommand::SubmitAction {
        author_actor_ref, integration_ref, conversation_ref: None, ..
    } if author_actor_ref == actor.as_str() && integration_ref == WORKLOOP_DESKTOP_INGRESS_REF));
    for (request, author, source, conversation) in [
        ("forged-author", "~@/actors/other", None, None),
        ("no-viewer", actor.as_str(), None, None),
        (
            "wrong-source",
            actor.as_str(),
            Some("cwp://example.test/sources/other"),
            None,
        ),
        (
            "missing-source",
            actor.as_str(),
            None,
            Some("cwp://example.test/conversations/one"),
        ),
    ] {
        let bound = if request == "no-viewer" {
            None
        } else {
            Some(actor.as_str())
        };
        assert!(translate_host_command(
            command(request, author, source, conversation),
            SOURCE_REF,
            WORKLOOP_DESKTOP_INGRESS_REF,
            bound,
            &output,
            &pending,
        )
        .unwrap()
        .is_none());
    }
    assert!(translate_host_command(
        command("legacy", actor.as_str(), Some(SOURCE_REF), None),
        SOURCE_REF,
        WORKLOOP_DESKTOP_INGRESS_REF,
        Some(actor.as_str()),
        &output,
        &pending,
    )
    .unwrap()
    .is_some());
}

#[test]
fn workspace_reload_uses_bound_actor_and_checks_legacy_sources() {
    let output = output::host_writer_for(io::sink()).unwrap();
    let pending = pending::registry();
    let actor = crate::test_refs::actor_ref("person");
    for (request, source, bound, accepted) in [
        ("workspace", None, Some(actor.as_str()), true),
        ("legacy", Some(SOURCE_REF), Some(actor.as_str()), true),
        (
            "wrong-source",
            Some("cwp://example.test/sources/other"),
            Some(actor.as_str()),
            false,
        ),
        ("no-viewer", None, None, false),
        ("legacy-no-viewer", Some(SOURCE_REF), None, false),
    ] {
        let result = translate_host_command(
            HostCommand::ReloadGraph {
                version: cwr::v2026_09::host::HOST_COMMAND_VERSION,
                request_id: request.into(),
                source_ref: source.map(str::to_owned),
            },
            SOURCE_REF,
            WORKLOOP_DESKTOP_INGRESS_REF,
            bound,
            &output,
            &pending,
        )
        .unwrap();
        assert_eq!(result.is_some(), accepted);
        if let Some(command) = result {
            assert!(
                matches!(command, RuntimeServiceCommand::ReloadGraph { requesting_actor_ref, .. } if requesting_actor_ref == actor.as_str())
            );
        }
    }
}

#[test]
fn sms_ingress_handoff_keeps_provider_identity_and_requires_selected_author() {
    use cwr::v2026_09::conversation::{
        ConversationIngress, ConversationIngressAddressing, ConversationIngressConversation,
    };
    let output = output::host_writer_for(io::sink()).unwrap();
    let actor = crate::test_refs::actor_ref("person");
    let ingress = ConversationIngress {
        owner_context_ref: None,
        file_refs: Vec::new(),
        integration_ref: "cwp://example.test/sms/assignment".into(),
        provider_event_ref: "sms-event-original".into(),
        author_actor_ref: actor.to_string(),
        conversation: ConversationIngressConversation::Continue {
            conversation_ref: "cwp://example.test/conversations/sms".into(),
        },
        addressing: ConversationIngressAddressing::Addressed {
            actor_refs: vec![crate::test_refs::actor_ref("helper").to_string()],
        },
        text: "Synthetic mail request".into(),
    };
    for (request, bound, accepted) in [
        ("owned", Some(actor.as_str()), true),
        ("unowned", None, false),
        ("other", Some("cwp://example.test/actors/other"), false),
    ] {
        let result = translate_host_command(
            HostCommand::AdmitIngress {
                version: 1,
                request_id: request.into(),
                ingress: Box::new(ingress.clone()),
                actor_integration_ref: None,
                attachment: Some(cwr::v2026_09::service::RuntimeIngressAttachment {
                    integration_ref: ingress.integration_ref.clone(),
                    destination_ref: "cwp://example.test/sms/destination".into(),
                    binding_key: "original-binding".into(),
                }),
            },
            SOURCE_REF,
            WORKLOOP_DESKTOP_INGRESS_REF,
            bound,
            &output,
            &pending::registry(),
        )
        .unwrap();
        if accepted {
            assert!(
                matches!(result, Some(RuntimeServiceCommand::AdmitIngress { ingress: forwarded, host_actor_ref: Some(author), attachment: Some(a), .. })
                if *forwarded == ingress && author == actor.as_str() && a.binding_key == "original-binding")
            );
        } else {
            assert!(result.is_none());
        }
    }
}
