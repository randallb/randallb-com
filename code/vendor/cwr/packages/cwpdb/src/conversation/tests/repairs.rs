use super::*;
#[test]
fn native_open_normalizes_legacy_projected_work_attachment_once() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("legacy.sqlite3");
    let mut store = NativeConversationStore::open(&path, scope(), options()).unwrap();
    let mut write = mutation(
        store
            .read(&request(Selection::Bootstrap), &Allow)
            .unwrap()
            .revision,
    );
    write.changes.publications.push(PublicationSuffix {conversation_ref:header().reference,from_position:0,items:vec![],activities:vec![PublishedActivity {activity_ref:"cwp://fixture/activities/update".into(),position:0,record:json!({"event":{"kind":"work_update","invocation_ref":"cwp://fixture/invocations/one","attachment":{"old":"retained"}}})}],remove_activities:vec![]});
    store.commit(&write, &Allow).unwrap();
    let before = store
        .read(&request(Selection::Bootstrap), &Allow)
        .unwrap()
        .revision;
    drop(store);
    let mut reopened = NativeConversationStore::open(&path, scope(), options()).unwrap();
    let normalized = reopened
        .read(&request(Selection::Bootstrap), &Allow)
        .unwrap()
        .revision;
    assert_ne!(before, normalized);
    drop(reopened);
    let mut physical = crate::NativeStore::open(&path).unwrap();
    physical.graph_read(|db|{let remaining:i64=db.query_row("SELECT count(*) FROM edges WHERE kind='timeline_activity' AND json_type(props,'$.record.event.attachment') IS NOT NULL",[],|row|row.get(0))?;assert_eq!(remaining,0);Ok(())}).unwrap();
    drop(physical);
    let mut reopened = NativeConversationStore::open(&path, scope(), options()).unwrap();
    assert_eq!(
        reopened
            .read(&request(Selection::Bootstrap), &Allow)
            .unwrap()
            .revision,
        normalized
    );
}
#[test]
fn validated_legacy_repair_moves_visualization_and_rebuilds_owned_indexes() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("layout.sqlite3");
    drop(NativeConversationStore::open(&path, scope(), options()).unwrap());
    let mut physical = crate::NativeStore::open(&path).unwrap();
    physical.graph_transaction(|tx|{
        crate::put_node(tx,&header().reference,"conversation",&json!({"title":"Legacy","creation_order":0,"commit_revision":0}))?;
        crate::put_node(tx,"cwp://fixture/messages/legacy","message",&json!({"record":{"text":"Retained","nullable":null,"visualizations":[{"artifact_ref":"cwp://fixture/artifacts/one"}]}}))?;
        crate::put_node(tx,"cwp://fixture/activities/boundary","activity",&json!({"record":{"event":{"kind":"message_recorded","message_ref":"cwp://fixture/messages/legacy"}}}))?;
        crate::put_edge(tx,"cwp://fixture/runtime/cwpdb","conversation",&header().reference,0,&json!({}))?;
        crate::put_edge(tx,&header().reference,"message","cwp://fixture/messages/legacy",0,&json!({"extension":true}))?;
        crate::put_edge(tx,&header().reference,"activity","cwp://fixture/activities/boundary",0,&json!({}))?;
        tx.execute("UPDATE nodes SET props=jsonb_set(props,'$.timeline_projection_version',0) WHERE kind='runtime'",[])?;Ok(())
    }).unwrap();
    drop(physical);
    let mut store = NativeConversationStore::open(&path, scope(), options()).unwrap();
    let before = store.read(&request(Selection::Bootstrap), &Allow).unwrap();
    for flag in [
        "message_boundary",
        "visualization_metadata",
        "identity_owners",
        "timeline_projection",
    ] {
        assert!(
            before.metadata["storage_maintenance"]
                .as_array()
                .unwrap()
                .contains(&json!(flag)),
            "missing {flag}"
        );
    }
    let repair = Mutation {
        scope: scope(),
        request_id: "validated-layout-repair".into(),
        expected_revision: before.revision,
        changes: Changes {
            repair_legacy: true,
            publications: vec![PublicationSuffix {
                conversation_ref: header().reference,
                from_position: 0,
                items: vec![PublishedItem::Message {
                    message_ref: "cwp://fixture/messages/legacy".into(),
                    position: 0,
                }],
                activities: vec![],
                remove_activities: vec![],
            }],
            ..Changes::default()
        },
        journal: None,
        authority_guards: vec![],
    };
    store.commit(&repair, &Allow).unwrap();
    assert_eq!(
        store
            .read(&request(Selection::Bootstrap), &Allow)
            .unwrap()
            .metadata["storage_maintenance"],
        json!([])
    );
    let page = store
        .read(
            &request(Selection::Conversation {
                conversation_ref: header().reference,
            }),
            &Allow,
        )
        .unwrap();
    let message = page
        .records
        .iter()
        .find(|r| r.kind == RecordKind::Message)
        .unwrap();
    assert_eq!(message.properties["record"]["nullable"], Value::Null);
    assert!(message.properties["record"].get("visualizations").is_none());
    let edge = page
        .relationships
        .iter()
        .find(|r| r.kind == "message")
        .unwrap();
    assert_eq!(edge.properties["extension"], true);
    assert!(edge.properties["visualizations"].is_array());
    drop(store);
    let mut physical = crate::NativeStore::open(&path).unwrap();
    physical
        .graph_read(|db| {
            assert_eq!(
                crate::targets(
                    db,
                    "cwp://fixture/activities/boundary",
                    "field:event.message_ref"
                )?,
                vec!["cwp://fixture/messages/legacy"]
            );
            Ok(())
        })
        .unwrap();
}
