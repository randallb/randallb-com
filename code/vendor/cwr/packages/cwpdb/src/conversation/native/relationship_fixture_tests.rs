//! Physical journal compatibility and current typed-commit rollback assertions.
use super::*;
use rusqlite::Connection;
fn initialize_relationship_journal(db: &Connection) -> CwpdbResult<()> {
    db.execute_batch("CREATE TEMP TABLE IF NOT EXISTS cwr_changed_relationships(ref_id INTEGER PRIMARY KEY);
        CREATE TEMP TRIGGER IF NOT EXISTS cwr_relationship_insert AFTER INSERT ON main.edges BEGIN
            INSERT INTO cwr_changed_relationships VALUES(NEW.ref_id) ON CONFLICT(ref_id) DO NOTHING;
        END;
        CREATE TEMP TRIGGER IF NOT EXISTS cwr_relationship_update AFTER UPDATE ON main.edges BEGIN
            INSERT INTO cwr_changed_relationships VALUES(NEW.ref_id) ON CONFLICT(ref_id) DO NOTHING;
        END;
        CREATE TEMP TRIGGER IF NOT EXISTS cwr_relationship_node_insert AFTER INSERT ON main.nodes BEGIN
            INSERT INTO cwr_changed_relationships SELECT ref_id FROM edges WHERE source_id=NEW.ref_id OR target_id=NEW.ref_id ON CONFLICT(ref_id) DO NOTHING;
        END;
        CREATE TEMP TRIGGER IF NOT EXISTS cwr_relationship_node_kind AFTER UPDATE OF kind ON main.nodes WHEN OLD.kind IS NOT NEW.kind BEGIN
            INSERT INTO cwr_changed_relationships SELECT ref_id FROM edges WHERE source_id=NEW.ref_id OR target_id=NEW.ref_id ON CONFLICT(ref_id) DO NOTHING;
        END;
        CREATE TEMP TRIGGER IF NOT EXISTS cwr_relationship_node_delete AFTER DELETE ON main.nodes BEGIN
            INSERT INTO cwr_changed_relationships SELECT ref_id FROM edges WHERE source_id=OLD.ref_id OR target_id=OLD.ref_id ON CONFLICT(ref_id) DO NOTHING;
        END;")?;
    Ok(())
}

#[test]
fn legacy_relationship_journal_deduplicates_edges_under_outer_abort_conflict_policy() {
    let mut store = crate::NativeStore::open_in_memory().unwrap();
    store
        .graph_transaction(|tx| {
            initialize_relationship_journal(tx)?;
            let source = "cwp://example.test/conversation";
            let target = "cwp://example.test/message";
            crate::put_node(tx, source, "conversation", &json!({}))?;
            crate::put_node(tx, target, "message", &json!({}))?;
            crate::put_edge(tx, source, "message", target, 0, &json!({}))?;
            tx.execute("UPDATE OR ABORT edges SET position=position", [])?;
            tx.execute("UPDATE OR ABORT edges SET position=position", [])?;
            let count: i64 =
                tx.query_row("SELECT count(*) FROM cwr_changed_relationships", [], |r| {
                    r.get(0)
                })?;
            assert_eq!(count, 1);
            tx.execute("DELETE FROM cwr_changed_relationships", [])?;
            assert_eq!(
                tx.query_row("SELECT count(*) FROM cwr_changed_relationships", [], |r| {
                    r.get::<_, i64>(0)
                })?,
                0
            );
            Ok(())
        })
        .unwrap();
}
struct Allow;
impl crate::invocation::Authorizer for Allow {
    fn authorize(&self, _: &crate::invocation::Context, _: &str, _: bool) -> CwpdbResult<()> {
        Ok(())
    }
}
#[test]
fn typed_endpoint_checks_follow_forward_nodes_and_roll_back_invalid_transactions() {
    let scope = Scope {
        runtime_root_ref: "cwp://example.test/runtime".into(),
        context: crate::invocation::Context {
            workspace_ref: "cwp://example.test/workspace".into(),
            caller_ref: "cwp://example.test/caller".into(),
        },
    };
    let options = OpenOptions {
        runtime_state_version: "cwp://org.coworkerprotocol/cwpdb/schemas/runtime-state/2026-09"
            .into(),
        workspace_binding: None,
        timeline_rollout: None,
    };
    let mut store = NativeConversationStore::in_memory(scope.clone(), options).unwrap();
    let conversation = "cwp://example.test/conversation";
    let message = "cwp://example.test/message";
    let actor = "cwp://example.test/actor";
    let (revision, counter) = current_revision(&store.connection, &scope).unwrap();
    let mut changes = Changes {
        records: vec![WriteRecord {
            reference: conversation.into(),
            kind: RecordKind::Conversation,
            properties: json!({}),
            owner_ref: None,
        }],
        memberships: vec![
            MembershipChange::Upsert {
                source_ref: root_ref(&scope),
                kind: MembershipKind::Conversation,
                member: Member {
                    target_ref: conversation.into(),
                    position: 0,
                    properties: json!({}),
                },
            },
            MembershipChange::Upsert {
                source_ref: conversation.into(),
                kind: MembershipKind::Message,
                member: Member {
                    target_ref: message.into(),
                    position: 0,
                    properties: json!({}),
                },
            },
        ],
        ..Changes::default()
    };
    for wrong_kind in [None, Some(RecordKind::Invocation)] {
        let mut attempt = changes.clone();
        if let Some(kind) = wrong_kind {
            attempt.records.push(WriteRecord {
                reference: message.into(),
                kind,
                properties: json!({"record":{}}),
                owner_ref: None,
            });
        }
        let mutation = Mutation {
            scope: scope.clone(),
            request_id: format!("wrong-{wrong_kind:?}"),
            expected_revision: revision.clone(),
            changes: attempt,
            journal: None,
            authority_guards: vec![],
        };
        assert!(store.commit(&mutation, &Allow).is_err());
        assert!(crate::node_props(&store.connection, conversation)
            .unwrap()
            .is_none());
        assert!(crate::node_props(&store.connection, message)
            .unwrap()
            .is_none());
        assert_eq!(
            current_revision(&store.connection, &scope).unwrap(),
            (revision.clone(), counter)
        );
    }
    changes.records.push(WriteRecord {
        reference: message.into(),
        kind: RecordKind::Message,
        properties: json!({"record":{}}),
        owner_ref: None,
    });
    changes.memberships.push(MembershipChange::Upsert {
        source_ref: conversation.into(),
        kind: MembershipKind::Participant,
        member: Member {
            target_ref: actor.into(),
            position: 0,
            properties: json!({}),
        },
    });
    let mutation = Mutation {
        scope: scope.clone(),
        request_id: "valid-forward".into(),
        expected_revision: revision,
        changes,
        journal: None,
        authority_guards: vec![],
    };
    store.commit(&mutation, &Allow).unwrap();
    let (revision, counter) = current_revision(&store.connection, &scope).unwrap();
    let collision = Mutation {
        scope: scope.clone(),
        request_id: "wrong-concrete-actor".into(),
        expected_revision: revision.clone(),
        changes: Changes {
            records: vec![WriteRecord {
                reference: actor.into(),
                kind: RecordKind::Message,
                properties: json!({"record":{}}),
                owner_ref: None,
            }],
            ..Changes::default()
        },
        journal: None,
        authority_guards: vec![],
    };
    assert!(store.commit(&collision, &Allow).is_err());
    assert!(crate::node_props(&store.connection, actor)
        .unwrap()
        .is_none());
    assert_eq!(
        current_revision(&store.connection, &scope).unwrap(),
        (revision, counter)
    );
}

#[test]
#[cfg(feature = "test-support")]
fn named_retained_fixtures_keep_missing_endpoints_and_current_physical_kinds() {
    use crate::test_support::retained_relationships::{retained_case, RetainedCase};
    use RetainedCase::*;
    let missing = retained_case(MissingMessage).unwrap();
    let edge = missing
        .relationships
        .iter()
        .find(|e| e.kind == "message")
        .unwrap();
    assert_eq!(edge.target_node, None);
    let valid = retained_case(ForwardMessage).unwrap();
    assert_eq!(
        valid
            .relationships
            .iter()
            .find(|e| e.kind == "message")
            .unwrap()
            .target_node
            .as_deref(),
        Some("message")
    );
    let wrong = retained_case(ParticipantWrongKind).unwrap();
    assert_eq!(
        wrong
            .relationships
            .iter()
            .find(|e| e.kind == "participant")
            .unwrap()
            .target_node
            .as_deref(),
        Some("message")
    );
    let person = retained_case(ParticipantPerson).unwrap();
    assert_eq!(
        person
            .relationships
            .iter()
            .find(|e| e.kind == "participant")
            .unwrap()
            .target_node
            .as_deref(),
        Some("person")
    );
    let removed = retained_case(MissingProjectionEdge).unwrap();
    assert_eq!(
        removed
            .relationships
            .iter()
            .find(|e| e.kind == "timeline_source")
            .unwrap()
            .target_edge,
        None
    );
    let unowned = retained_case(UnownedKinds).unwrap();
    assert!(unowned.records.is_empty());
    assert!(unowned.relationships.is_empty());
    let orphan = retained_case(OwnedOrphan).unwrap();
    assert!(orphan
        .records
        .iter()
        .any(|n| n.reference == "cwp://example.test/timelines/owned-orphan"));
}
