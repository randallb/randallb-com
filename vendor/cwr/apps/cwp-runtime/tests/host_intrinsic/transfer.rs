use super::*;
use cwr::intrinsic_cwr::IntrinsicCwrResponse;
use cwr::v2026_09::host::ConversationControl;

#[test]
fn host_transfer_imports_large_history_and_continues_in_a_separate_conversation() {
    let temporary = tempfile::tempdir().unwrap();
    let (child, mut input, receiver) = start_runtime(temporary.path());
    let actor = crate::test_refs::actor_ref("person");
    let HostEvent::CommandSucceeded { receipt, .. } = send(
        &mut input,
        &receiver,
        HostCommand::CreateConversation {
            version: HOST_COMMAND_VERSION,
            request_id: "transfer-source".into(),
            source_ref: SOURCE_REF.into(),
            author_actor_ref: actor.as_str().into(),
            text: "Original text\nSecond line".into(),
        },
    ) else {
        panic!("create source")
    };
    let HostEvent::IntrinsicResponse { response, .. } = send(
        &mut input,
        &receiver,
        HostCommand::ExecuteConversationControl {
            version: HOST_COMMAND_VERSION,
            request_id: "export-source".into(),
            source_ref: SOURCE_REF.into(),
            requesting_actor_ref: actor.as_str().into(),
            conversation_ref: Some(receipt.conversation_ref.clone()),
            control: ConversationControl::Export,
        },
    ) else {
        panic!("export response")
    };
    let IntrinsicCwrResponse::ConversationExported { mut export } = *response else {
        panic!("export")
    };
    assert_eq!(export.conversation.messages.len(), 1);
    assert_eq!(
        export.conversation.messages[0].text,
        "Original text\nSecond line"
    );
    export.conversation.messages[0]
        .text
        .push_str(&"large history\n".repeat(100_000));
    let file = temporary.path().join("conversation-export.json");
    fs::write(&file, serde_json::to_vec(&export).unwrap()).unwrap();
    let HostEvent::IntrinsicResponse { response, .. } = send(
        &mut input,
        &receiver,
        HostCommand::ExecuteConversationControl {
            version: HOST_COMMAND_VERSION,
            request_id: "import-source".into(),
            source_ref: SOURCE_REF.into(),
            requesting_actor_ref: actor.as_str().into(),
            conversation_ref: None,
            control: ConversationControl::Import {
                path: file.to_string_lossy().into(),
                import_id: "stable-import".into(),
            },
        },
    ) else {
        panic!("import response")
    };
    let IntrinsicCwrResponse::ConversationImported { conversation_ref } = *response else {
        panic!("import")
    };
    fs::remove_file(&file).unwrap();
    let HostEvent::IntrinsicResponse { response, .. } = send(
        &mut input,
        &receiver,
        HostCommand::ExecuteConversationControl {
            version: HOST_COMMAND_VERSION,
            request_id: "retry-import-source".into(),
            source_ref: SOURCE_REF.into(),
            requesting_actor_ref: actor.as_str().into(),
            conversation_ref: None,
            control: ConversationControl::Import {
                path: file.to_string_lossy().into(),
                import_id: "stable-import".into(),
            },
        },
    ) else {
        panic!("retry import response")
    };
    assert!(
        matches!(*response, IntrinsicCwrResponse::ConversationImported { conversation_ref: ref retried } if retried == &conversation_ref)
    );
    assert_ne!(conversation_ref, receipt.conversation_ref);
    loop {
        if let HostEvent::ConversationUpsert { conversation, .. } = receive(&receiver) {
            if conversation.conversation_ref == conversation_ref {
                assert_eq!(conversation.messages.len(), 1);
                assert_eq!(
                    conversation.messages[0].text,
                    export.conversation.messages[0].text
                );
                break;
            }
        }
    }
    let continued = send(
        &mut input,
        &receiver,
        HostCommand::ContinueConversation {
            version: HOST_COMMAND_VERSION,
            request_id: "continue-import".into(),
            source_ref: SOURCE_REF.into(),
            conversation_ref: conversation_ref.clone(),
            author_actor_ref: actor.as_str().into(),
            text: "New branch".into(),
            recovery_only: false,
        },
    );
    assert!(matches!(continued, HostEvent::CommandSucceeded { .. }));
    drop(input);
    drop(child);
    let (restarted, input, receiver) = start_runtime(temporary.path());
    loop {
        if let HostEvent::ConversationSnapshot { conversations, .. } = receive(&receiver) {
            assert_eq!(
                conversations
                    .iter()
                    .find(|c| c.conversation_ref == receipt.conversation_ref)
                    .unwrap()
                    .messages
                    .len(),
                1
            );
            let imported = conversations
                .iter()
                .find(|c| c.conversation_ref == conversation_ref)
                .unwrap();
            assert_eq!(imported.messages.len(), 2);
            assert_eq!(imported.messages[1].text, "New branch");
            break;
        }
    }
    drop(input);
    drop(restarted);
}
