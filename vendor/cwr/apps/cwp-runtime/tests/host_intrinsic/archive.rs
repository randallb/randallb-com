use super::*;
use cwr::intrinsic_cwr::IntrinsicCwrResponse;
use cwr::v2026_09::host::ConversationControl;

#[test]
fn host_archive_and_restore_persist_and_publish_shared_state() {
    let temporary = tempfile::tempdir().unwrap();
    let (child, mut input, receiver) = start_runtime(temporary.path());
    let actor = crate::test_refs::actor_ref("person");
    let created = send(
        &mut input,
        &receiver,
        HostCommand::CreateConversation {
            version: HOST_COMMAND_VERSION,
            request_id: "create-archive-note".into(),
            source_ref: SOURCE_REF.into(),
            author_actor_ref: actor.as_str().into(),
            text: "Archive this conversation".into(),
        },
    );
    let HostEvent::CommandSucceeded { receipt, .. } = created else {
        panic!("create note")
    };
    for (request_id, control, expected) in [
        ("archive-note", ConversationControl::Archive, true),
        ("restore-note", ConversationControl::Restore, false),
    ] {
        let command = HostCommand::ExecuteConversationControl {
            version: HOST_COMMAND_VERSION,
            request_id: request_id.into(),
            source_ref: SOURCE_REF.into(),
            conversation_ref: Some(receipt.conversation_ref.clone()),
            requesting_actor_ref: actor.as_str().into(),
            control,
        };
        serde_json::to_writer(&mut input, &command).unwrap();
        input.write_all(b"\n").unwrap();
        input.flush().unwrap();
        let mut response_seen = false;
        let mut upsert_seen = false;
        while !response_seen || !upsert_seen {
            match receive(&receiver) {
                HostEvent::IntrinsicResponse {
                    request_id: event_request_id,
                    response,
                    ..
                } if event_request_id == request_id => {
                    assert!(matches!(
                        *response,
                        IntrinsicCwrResponse::ConversationArchived { archived, .. }
                            if archived == expected
                    ));
                    response_seen = true;
                }
                HostEvent::ConversationUpsert { conversation, .. }
                    if conversation.conversation_ref == receipt.conversation_ref
                        && conversation.archived == expected =>
                {
                    assert_eq!(conversation.messages.len(), 1);
                    upsert_seen = true;
                }
                _ => {}
            }
        }
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
            assert!(!retained.archived);
            break;
        }
    }
    drop(input);
    drop(restarted);
}

#[test]
fn host_archive_stops_provider_work_and_orders_restore_after_acknowledgement() {
    use cwr::v2026_09::upstream::{UpstreamCommand, UpstreamCoworker, UpstreamEvent};
    let temporary = tempfile::tempdir().unwrap();
    let (_child, mut input, receiver) = start_runtime(temporary.path());
    let actor = "cwp://example.test/archive/actors/provider";
    let person = crate::test_refs::actor_ref("person");
    let registered = send(
        &mut input,
        &receiver,
        HostCommand::Upstream {
            version: HOST_COMMAND_VERSION,
            request_id: "archive-register".into(),
            command: UpstreamCommand::Register {
                coworkers: vec![UpstreamCoworker {
                    actor_ref: actor.into(),
                    coworker_ref: "cwp://example.test/coworkers/provider".into(),
                    handle: "@provider".into(),
                    name: "Provider".into(),
                    actions: vec![],
                }],
            },
        },
    );
    let HostEvent::UpstreamRegistered {
        provider_session_ref,
        ..
    } = registered
    else {
        panic!("register failed");
    };
    send(
        &mut input,
        &receiver,
        HostCommand::Upstream {
            version: HOST_COMMAND_VERSION,
            request_id: "archive-enable".into(),
            command: UpstreamCommand::Enable {
                provider_session_ref,
                actor_ref: actor.into(),
                enabled: true,
            },
        },
    );
    let admitted = send(
        &mut input,
        &receiver,
        HostCommand::CreateConversation {
            version: HOST_COMMAND_VERSION,
            request_id: "archive-work".into(),
            source_ref: SOURCE_REF.into(),
            author_actor_ref: person.as_str().into(),
            text: "@provider work".into(),
        },
    );
    let HostEvent::CommandSucceeded { receipt, .. } = admitted else {
        panic!("admit failed");
    };
    loop {
        if matches!(
            receive(&receiver),
            HostEvent::Upstream {
                event: UpstreamEvent::WorkRequested { .. }
            }
        ) {
            break;
        }
    }
    let queued = send(
        &mut input,
        &receiver,
        HostCommand::ContinueConversation {
            version: HOST_COMMAND_VERSION,
            request_id: "archive-queued".into(),
            source_ref: SOURCE_REF.into(),
            conversation_ref: receipt.conversation_ref.clone(),
            author_actor_ref: person.as_str().into(),
            text: "@provider queued".into(),
            recovery_only: false,
        },
    );
    let HostEvent::CommandSucceeded {
        receipt: queued, ..
    } = queued
    else {
        panic!("queue failed");
    };
    let command = HostCommand::ExecuteConversationControl {
        version: HOST_COMMAND_VERSION,
        request_id: "archive-active".into(),
        source_ref: SOURCE_REF.into(),
        conversation_ref: Some(receipt.conversation_ref.clone()),
        requesting_actor_ref: person.as_str().into(),
        control: ConversationControl::Archive,
    };
    serde_json::to_writer(&mut input, &command).unwrap();
    input.write_all(b"\n").unwrap();
    input.flush().unwrap();
    loop {
        match receive(&receiver) {
            HostEvent::Upstream {
                event: UpstreamEvent::StopRequested { invocation_ref },
            } => {
                assert_eq!(Some(invocation_ref), receipt.invocation_ref);
                break;
            }
            HostEvent::Upstream {
                event: UpstreamEvent::WorkRequested { .. },
            } => panic!("queued work started"),
            _ => {}
        }
    }
    queue_restore_before_stop_ack(
        &mut input,
        &receiver,
        &receipt.conversation_ref,
        person.as_str(),
    );
    let ack = HostCommand::Upstream {
        version: HOST_COMMAND_VERSION,
        request_id: "archive-stop-ack".into(),
        command: UpstreamCommand::AcknowledgeStop {
            invocation_ref: receipt.invocation_ref.clone().unwrap(),
        },
    };
    serde_json::to_writer(&mut input, &ack).unwrap();
    input.write_all(b"\n").unwrap();
    input.flush().unwrap();
    let mut restored = false;
    let mut response = false;
    let mut restore_response = false;
    let mut terminals = std::collections::BTreeSet::new();
    while !restored || !response || !restore_response || terminals.len() < 2 {
        match receive(&receiver) {
            HostEvent::IntrinsicResponse {
                request_id,
                response: result,
                ..
            } if request_id == "archive-active" => {
                assert!(matches!(
                    *result,
                    IntrinsicCwrResponse::ConversationArchived { archived: true, .. }
                ));
                response = true;
            }
            HostEvent::IntrinsicResponse {
                request_id,
                response: result,
                ..
            } if request_id == "restore-after-archive" => {
                assert!(response, "restore response preceded archive response");
                assert!(matches!(
                    *result,
                    IntrinsicCwrResponse::ConversationArchived {
                        archived: false,
                        ..
                    }
                ));
                restore_response = true;
            }
            HostEvent::ConversationUpsert { conversation, .. }
                if conversation.conversation_ref == receipt.conversation_ref
                    && !conversation.archived =>
            {
                assert!(conversation.messages.iter().any(|m| m.text == "work"));
                restored = true;
            }
            HostEvent::ConversationExecutionFinished { response, .. } => {
                assert_eq!(
                    response.status,
                    cwr::v2026_09::invocation::ActionResponseStatus::STOPPED
                );
                terminals.insert(response.invocation_ref.clone());
            }
            HostEvent::Upstream {
                event: UpstreamEvent::WorkRequested { .. },
            } => panic!("queued work started"),
            _ => {}
        }
    }
    assert!(terminals.contains(receipt.invocation_ref.as_ref().unwrap()));
    assert!(terminals.contains(queued.invocation_ref.as_ref().unwrap()));
    assert_fresh_provider_work(
        &mut input,
        &receiver,
        &receipt.conversation_ref,
        person.as_str(),
    );
}

// The setup command is rejected in an open host session. Its response proves
// the reader reached the queued restore while the provider acknowledgement waits.
fn queue_restore_before_stop_ack(
    input: &mut ChildStdin,
    receiver: &HostEvents,
    conversation: &str,
    person: &str,
) {
    for command in [
        HostCommand::ExecuteConversationControl {
            version: HOST_COMMAND_VERSION,
            request_id: "restore-after-archive".into(),
            source_ref: SOURCE_REF.into(),
            conversation_ref: Some(conversation.into()),
            requesting_actor_ref: person.into(),
            control: ConversationControl::Restore,
        },
        HostCommand::InspectWorkspace {
            version: HOST_COMMAND_VERSION,
            request_id: "archive-order-barrier".into(),
        },
    ] {
        serde_json::to_writer(&mut *input, &command).unwrap();
        input.write_all(b"\n").unwrap();
    }
    input.flush().unwrap();
    loop {
        match receive(receiver) {
            HostEvent::IntrinsicResponse { request_id, .. }
                if request_id == "restore-after-archive" =>
            {
                panic!("restore overtook archive")
            }
            HostEvent::CommandFailed { request_id, .. }
                if request_id == "archive-order-barrier" =>
            {
                break
            }
            _ => {}
        }
    }
}

fn assert_fresh_provider_work(
    input: &mut ChildStdin,
    receiver: &HostEvents,
    conversation: &str,
    person: &str,
) {
    let fresh = send(
        &mut *input,
        receiver,
        HostCommand::ContinueConversation {
            version: HOST_COMMAND_VERSION,
            request_id: "work-after-restore".into(),
            source_ref: SOURCE_REF.into(),
            conversation_ref: conversation.into(),
            author_actor_ref: person.into(),
            text: "@provider fresh".into(),
            recovery_only: false,
        },
    );
    let HostEvent::CommandSucceeded { receipt: fresh, .. } = fresh else {
        panic!("fresh admission failed")
    };
    loop {
        if let HostEvent::Upstream {
            event: cwr::v2026_09::upstream::UpstreamEvent::WorkRequested { work },
        } = receive(receiver)
        {
            assert_eq!(Some(work.invocation_ref), fresh.invocation_ref);
            break;
        }
    }
}
