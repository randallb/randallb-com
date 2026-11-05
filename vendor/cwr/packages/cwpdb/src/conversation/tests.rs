use super::*;
use serde_json::json;
struct Allow;
impl Authorizer for Allow {
    fn authorize(&self, _: &Context, _: &str, _: bool) -> CwpdbResult<()> {
        Ok(())
    }
}
fn scope() -> Scope {
    Scope {
        context: Context {
            workspace_ref: "cwp://fixture/workspaces/family".into(),
            caller_ref: "cwp://fixture/actors/coordinator".into(),
        },
        runtime_root_ref: "cwp://fixture/runtime".into(),
    }
}
fn options() -> OpenOptions {
    OpenOptions {
        runtime_state_version: "cwp://org.coworkerprotocol/cwpdb/schemas/runtime-state/2026-09"
            .into(),
        workspace_binding: None,
        timeline_rollout: None,
    }
}
fn request(selection: Selection) -> ReadRequest {
    ReadRequest {
        scope: scope(),
        selection,
        continuation: None,
        page_size: 100,
        max_bytes: 1_048_576,
        expected_revision: None,
    }
}
fn header() -> WriteRecord {
    WriteRecord {
        reference: "cwp://fixture/conversations/one".into(),
        kind: RecordKind::Conversation,
        properties: json!({"title":"Family","owner_context_ref":null,"sidebar_eligible":true}),
        owner_ref: None,
    }
}
fn mutation(revision: String) -> Mutation {
    Mutation {
        scope: scope(),
        request_id: "admit-source-one".into(),
        expected_revision: revision,
        changes: Changes {
            records: vec![
                header(),
                WriteRecord {
                    reference: "cwp://fixture/messages/one".into(),
                    kind: RecordKind::Message,
                    properties: json!({"record":{"message_ref":"cwp://fixture/messages/one","author_actor_ref":"cwp://fixture/actors/customer","addressed_actor_refs":[],"text":"School closes early"}}),
                    owner_ref: None,
                },
            ],
            memberships: vec![
                MembershipChange::Upsert {
                    source_ref: "cwp://fixture/runtime/cwpdb".into(),
                    kind: MembershipKind::Conversation,
                    member: Member {
                        target_ref: header().reference,
                        position: 0,
                        properties: json!({}),
                    },
                },
                MembershipChange::Upsert {
                    source_ref: header().reference,
                    kind: MembershipKind::Message,
                    member: Member {
                        target_ref: "cwp://fixture/messages/one".into(),
                        position: 0,
                        properties: json!({}),
                    },
                },
            ],
            ..Changes::default()
        },
        journal: None,
        authority_guards: vec![],
    }
}
#[test]
fn native_runtime_commit_reopens_existing_graph_and_pages_cold_history() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("runtime.db");
    let mut store = NativeConversationStore::open(&path, scope(), options()).unwrap();
    let initial = store.read(&request(Selection::Bootstrap), &Allow).unwrap();
    let mutate = mutation(initial.revision);
    let result = store.commit(&mutate, &Allow).unwrap();
    assert_eq!(store.commit(&mutate, &Allow).unwrap(), result);
    drop(store);
    let mut store = NativeConversationStore::open(&path, scope(), options()).unwrap();
    let bootstrap = store.read(&request(Selection::Bootstrap), &Allow).unwrap();
    assert!(!bootstrap
        .records
        .iter()
        .any(|r| r.kind == RecordKind::Message));
    assert!(bootstrap
        .records
        .iter()
        .any(|r| r.kind == RecordKind::Conversation));
    let mut query = request(Selection::Conversation {
        conversation_ref: header().reference,
    });
    query.page_size = 1;
    let mut records = Vec::new();
    let mut relations = Vec::new();
    loop {
        let page = store.read(&query, &Allow).unwrap();
        if query.expected_revision.is_none() {
            query.expected_revision = Some(page.revision.clone());
        }
        records.extend(page.records);
        relations.extend(page.relationships);
        if page.complete {
            break;
        }
        query.continuation = page.continuation;
    }
    assert!(records.iter().any(|r| r.kind == RecordKind::Message
        && r.properties["record"]["text"] == "School closes early"));
    assert!(relations
        .iter()
        .any(|r| r.kind == "message" && r.position == 0));
    let mut stale = mutation("old".into());
    stale.request_id = "different".into();
    assert!(store
        .commit(&stale, &Allow)
        .unwrap_err()
        .to_string()
        .contains("revision_conflict"));
}
#[test]
fn receipt_erasure_prevents_reexecuting_protected_runtime_mutation() {
    let mut store = NativeConversationStore::in_memory(scope(), options()).unwrap();
    let first = store.read(&request(Selection::Bootstrap), &Allow).unwrap();
    let create = mutation(first.revision);
    store.commit(&create, &Allow).unwrap();
    let current = store.read(&request(Selection::Bootstrap), &Allow).unwrap();
    let remove = Mutation {
        scope: scope(),
        request_id: "erase-message".into(),
        expected_revision: current.revision,
        changes: Changes {
            remove_records: vec![RemoveRecord {
                reference: "cwp://fixture/messages/one".into(),
                kind: RecordKind::Message,
            }],
            ..Changes::default()
        },
        journal: None,
        authority_guards: vec![],
    };
    store.commit(&remove, &Allow).unwrap();
    assert_eq!(
        store.commit(&create, &Allow).unwrap(),
        CommitOutcome::CommittedResultExpired
    );
}
#[test]
fn execution_receipt_and_cwr_publication_commit_or_rollback_together() {
    use crate::invocation::{InvocationStorage, Operation, ReceiptLookup};
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("runtime.db");
    let mut graph = NativeConversationStore::open(&path, scope(), options()).unwrap();
    let mut journal = crate::NativeStore::open(&path).unwrap();
    let initial = graph.read(&request(Selection::Bootstrap), &Allow).unwrap();
    let event = crate::invocation::Mutation {
        context: scope().context,
        invocation_ref: "cwp://fixture/invocations/one".into(),
        request_id: "execution-admit".into(),
        expected_revision: None,
        operation: Operation::Admit {
            conversation_ref: header().reference,
            parent_invocation_ref: None,
            admitted: json!({"source":"ordinary-cwr-action"}),
        },
    };
    let mut write = mutation(initial.revision.clone());
    write.journal = Some(JournalCommit {
        request: event.clone(),
        now_unix_ms: 10,
    });
    // A graph-domain failure after the nested journal mutation rolls both back.
    write.changes.memberships.push(MembershipChange::Upsert {
        source_ref: header().reference,
        kind: MembershipKind::Message,
        member: Member {
            target_ref: "cwp://fixture/messages/missing".into(),
            position: 1,
            properties: json!({}),
        },
    });
    assert!(graph.commit(&write, &Allow).is_err());
    assert_eq!(
        journal
            .receipt(
                &scope().context,
                &event.invocation_ref,
                &event.request_id,
                &Allow
            )
            .unwrap(),
        ReceiptLookup::NotCommitted
    );
    assert_eq!(
        graph
            .read(&request(Selection::Bootstrap), &Allow)
            .unwrap()
            .revision,
        initial.revision
    );
    assert!(graph
        .get(&scope(), &header().reference, &Allow)
        .unwrap()
        .is_none());
    write.changes.memberships.pop();
    let committed = graph.commit(&write, &Allow).unwrap();
    assert!(matches!(
        &committed,
        CommitOutcome::Committed {
            journal_receipt: Some(_),
            ..
        }
    ));
    assert_eq!(graph.commit(&write, &Allow).unwrap(), committed);
    assert!(matches!(
        journal
            .receipt(
                &scope().context,
                &event.invocation_ref,
                &event.request_id,
                &Allow
            )
            .unwrap(),
        ReceiptLookup::Retained { .. }
    ));
    assert!(graph
        .get(&scope(), &header().reference, &Allow)
        .unwrap()
        .is_some());
}
#[test]
fn current_source_revision_guard_prevents_stale_authority_publication() {
    use crate::documents::{DocumentStorage, Mutation as DocumentMutation, Operation};
    struct Valid;
    impl crate::documents::Validator for Valid {
        fn validate(
            &self,
            _: &str,
            _: Option<&serde_json::Map<String, serde_json::Value>>,
            _: &serde_json::Map<String, serde_json::Value>,
        ) -> CwpdbResult<()> {
            Ok(())
        }
    }
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("runtime.db");
    let mut graph = NativeConversationStore::open(&path, scope(), options()).unwrap();
    let mut docs = crate::NativeStore::open(&path).unwrap();
    let source = "cwp://fixture/grants/source";
    docs.mutate_document(
        &DocumentMutation {
            context: scope().context,
            reference: source.into(),
            request_id: "grant".into(),
            expected_revision: None,
            operation: Operation::Create {
                schema_ref: "cwp://fixture/schemas/grant".into(),
                value: serde_json::from_value(json!({"active":true})).unwrap(),
            },
        },
        &Allow,
        &Valid,
    )
    .unwrap();
    let old = docs
        .get_document(&scope().context, source, &Allow)
        .unwrap()
        .unwrap();
    let mut write = mutation(
        graph
            .read(&request(Selection::Bootstrap), &Allow)
            .unwrap()
            .revision,
    );
    write.authority_guards.push(AuthorityGuard {
        reference: source.into(),
        expected_revision: old.revision.clone(),
    });
    docs.mutate_document(
        &DocumentMutation {
            context: scope().context,
            reference: source.into(),
            request_id: "revoke".into(),
            expected_revision: Some(old.revision),
            operation: Operation::Replace {
                value: serde_json::from_value(json!({"active":false})).unwrap(),
            },
        },
        &Allow,
        &Valid,
    )
    .unwrap();
    assert!(graph
        .commit(&write, &Allow)
        .unwrap_err()
        .to_string()
        .contains("source authority revision conflict"));
    assert!(graph
        .get(&scope(), &header().reference, &Allow)
        .unwrap()
        .is_none());
}

#[path = "tests/opaque.rs"]
mod opaque;

mod selectors;

mod repairs;

mod finite_selection;
