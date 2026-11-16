use super::*;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use std::time::{Duration, Instant};
struct Allow;
impl Authorizer for Allow {
    fn authorize(&self, _: &Context, _: &str, _: bool) -> CwpdbResult<()> {
        Ok(())
    }
}
#[test]
fn cancelled_timeline_sql_stops_and_a_stale_token_cannot_interrupt_reuse() {
    let scope = Scope {
        context: Context {
            workspace_ref: "cwp://fixture/timeline-control/workspace".into(),
            caller_ref: "cwp://fixture/timeline-control/host".into(),
        },
        runtime_root_ref: "cwp://fixture/timeline-control/runtime".into(),
    };
    let mut store = NativeConversationStore::in_memory(
        scope.clone(),
        OpenOptions {
            runtime_state_version: "cwp://org.coworkerprotocol/cwpdb/schemas/runtime-state/2026-09"
                .into(),
            workspace_binding: None,
            timeline_rollout: None,
        },
    )
    .unwrap();
    let conversation = "cwp://fixture/timeline-control/conversation";
    let connection = "cwp://fixture/timeline-control/connection";
    let message = "cwp://fixture/timeline-control/message";
    let read_request = ReadRequest {
        scope: scope.clone(),
        selection: Selection::Bootstrap,
        continuation: None,
        page_size: 100,
        max_bytes: MAX_WIRE_BYTES,
        expected_revision: None,
    };
    let revision = store.read(&read_request, &Allow).unwrap().revision;
    let member = |source: &str, kind, target: &str| MembershipChange::Upsert {
        source_ref: source.into(),
        kind,
        member: Member {
            target_ref: target.into(),
            position: 0,
            properties: json!({}),
        },
    };
    let records = vec![
        WriteRecord {
            reference: conversation.into(),
            kind: RecordKind::Conversation,
            properties: json!({}),
            owner_ref: None,
        },
        WriteRecord {
            reference: connection.into(),
            kind: RecordKind::ConversationTimelineConnection,
            properties: json!({"revision":1,"store_sequence":0,"next_sequence":3000}),
            owner_ref: Some(conversation.into()),
        },
        WriteRecord {
            reference: message.into(),
            kind: RecordKind::Message,
            properties: json!({"record":{"message_ref":message,"text":"fixture","author_actor_ref":"cwp://fixture/person","addressed_actor_refs":[]}}),
            owner_ref: None,
        },
    ];
    let items = (0..3000)
        .map(|position| TimelineItemWrite {
            conversation_ref: conversation.into(),
            connection_ref: connection.into(),
            item_ref: format!("cwp://fixture/timeline-control/items/{position}"),
            publication_ref: format!("cwp://fixture/timeline-control/publications/{position}"),
            position,
            content: TimelineContent::Message {
                message_ref: message.into(),
            },
            child_connection_ref: None,
        })
        .collect();
    store
        .commit(
            &Mutation {
                scope: scope.clone(),
                request_id: "seed-timeline-control".into(),
                expected_revision: revision,
                changes: Changes {
                    records,
                    timeline_items: items,
                    memberships: vec![
                        member(
                            &root_ref(&scope),
                            MembershipKind::Conversation,
                            conversation,
                        ),
                        member(conversation, MembershipKind::Message, message),
                        member(
                            conversation,
                            MembershipKind::ConversationTimelineRoot,
                            connection,
                        ),
                    ],
                    ..Default::default()
                },
                journal: None,
                authority_guards: vec![],
            },
            &Allow,
        )
        .unwrap();
    let request = TimelineReadRequest {
        scope,
        conversation_ref: conversation.into(),
        after: None,
        before: None,
        first: Some(1),
        last: None,
        participant_actor_ref: None,
        include_execution: false,
        deadline_unix_ms: i64::try_from(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis(),
        )
        .unwrap()
            + 30000,
        max_bytes: MAX_WIRE_BYTES,
    };
    let steps = Arc::new(AtomicUsize::new(0));
    let cancellation = ReadCancellation::default();
    let observed = steps.clone();
    let stopping = cancellation.clone();
    let canceller = std::thread::spawn(move || {
        let limit = Instant::now() + Duration::from_secs(5);
        while observed.load(Ordering::Relaxed) < 500 && Instant::now() < limit {
            std::thread::yield_now();
        }
        stopping.cancel();
    });
    let error = store
        .timeline_page(
            &request,
            &ReadControl {
                cancellation: cancellation.clone(),
                deadline: Instant::now() + Duration::from_secs(10),
                vm_steps: Some(steps.clone()),
            },
            &Allow,
        )
        .unwrap_err();
    canceller.join().unwrap();
    assert!(
        steps.load(Ordering::Relaxed) >= 500,
        "the native storage query must actually run before cancellation"
    );
    assert!(error.to_string().contains("cancelled"), "{error}");
    cancellation.cancel();
    let next = store
        .timeline_page(
            &request,
            &ReadControl {
                cancellation: ReadCancellation::default(),
                deadline: Instant::now() + Duration::from_secs(5),
                vm_steps: None,
            },
            &Allow,
        )
        .unwrap()
        .unwrap();
    assert_eq!(next.anchors.len(), 1);
    assert_eq!(next.total_count, 3000);
    let past = ReadControl {
        cancellation: ReadCancellation::default(),
        deadline: Instant::now() - Duration::from_millis(1),
        vm_steps: None,
    };
    assert!(store
        .timeline_page(&request, &past, &Allow)
        .unwrap_err()
        .to_string()
        .contains("deadline"));
}
