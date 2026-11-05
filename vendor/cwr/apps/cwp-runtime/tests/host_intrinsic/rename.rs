use super::*;
use cwr::intrinsic_cwr::IntrinsicCwrResponse;
use cwr::v2026_09::host::ConversationControl;

#[test]
fn host_rename_persists_and_publishes_without_adding_messages() {
    let temporary = tempfile::tempdir().unwrap();
    let (child, mut input, receiver) = start_runtime(temporary.path());
    let actor = crate::test_refs::actor_ref("person");
    let created = send(
        &mut input,
        &receiver,
        HostCommand::CreateConversation {
            version: HOST_COMMAND_VERSION,
            request_id: "create-note".into(),
            source_ref: SOURCE_REF.into(),
            author_actor_ref: actor.as_str().into(),
            text: "Original automatic title".into(),
        },
    );
    let HostEvent::CommandSucceeded { receipt, .. } = created else {
        panic!("create note")
    };
    let rename = HostCommand::ExecuteConversationControl {
        version: HOST_COMMAND_VERSION,
        request_id: "rename-note".into(),
        source_ref: SOURCE_REF.into(),
        conversation_ref: Some(receipt.conversation_ref.clone()),
        requesting_actor_ref: actor.as_str().into(),
        control: ConversationControl::Rename {
            title: "  Our \"shared\" plan 🚀  ".into(),
        },
    };
    let result = send(&mut input, &receiver, rename.clone());
    let HostEvent::IntrinsicResponse { response, .. } = result else {
        panic!("rename response")
    };
    assert!(
        matches!(*response, IntrinsicCwrResponse::ConversationRenamed { title, .. } if title == "Our \"shared\" plan 🚀")
    );
    loop {
        if let HostEvent::ConversationUpsert { conversation, .. } = receive(&receiver) {
            if conversation.title.as_deref() == Some("Our \"shared\" plan 🚀") {
                assert_eq!(conversation.messages.len(), 1);
                break;
            }
        }
    }
    for (index, invalid_title) in [" ", "line\nbreak", &"x".repeat(121)].iter().enumerate() {
        let mut invalid = rename.clone();
        if let HostCommand::ExecuteConversationControl {
            control: ConversationControl::Rename { title },
            request_id,
            ..
        } = &mut invalid
        {
            *title = (*invalid_title).into();
            *request_id = format!("invalid-{index}");
        }
        assert!(matches!(
            send(&mut input, &receiver, invalid),
            HostEvent::CommandFailed { .. }
        ));
    }
    for wrong_source in [false, true] {
        let mut invalid = rename.clone();
        if let HostCommand::ExecuteConversationControl {
            source_ref,
            requesting_actor_ref,
            request_id,
            ..
        } = &mut invalid
        {
            *request_id = format!("wrong-identity-{wrong_source}");
            if wrong_source {
                *source_ref = "cwp://example.test/wrong-source".into();
            } else {
                *requesting_actor_ref = "cwp://example.test/actors/stranger".into();
            }
        }
        assert!(matches!(
            send(&mut input, &receiver, invalid),
            HostEvent::CommandFailed { .. }
        ));
    }
    drop(input);
    drop(child);
    let (restarted, input, receiver) = start_runtime(temporary.path());
    loop {
        if let HostEvent::ConversationSnapshot { conversations, .. } = receive(&receiver) {
            let retained = conversations
                .iter()
                .find(|item| item.conversation_ref == receipt.conversation_ref)
                .unwrap();
            assert_eq!(retained.title.as_deref(), Some("Our \"shared\" plan 🚀"));
            assert_eq!(retained.messages.len(), 1);
            break;
        }
    }
    drop(input);
    drop(restarted);
}
