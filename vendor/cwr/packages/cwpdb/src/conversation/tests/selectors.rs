use super::*;
#[test]
fn metadata_dedup_and_binding_only_actor_reads_stay_finite() {
    let mut store = NativeConversationStore::in_memory(scope(), options()).unwrap();
    let mut write = mutation(
        store
            .read(&request(Selection::Bootstrap), &Allow)
            .unwrap()
            .revision,
    );
    write.changes.records.extend([
        WriteRecord { reference:"cwp://fixture/messages/two".into(),kind:RecordKind::Message,properties:json!({"record":{"text":"second closed message"}}),owner_ref:None },
        WriteRecord { reference:"cwp://fixture/activities/commentary".into(),kind:RecordKind::Activity,properties:json!({"record":{"event":{"kind":"commentary_progress","provider_integration_ref":"cwp://fixture/integrations/provider","response_or_turn_id":"turn-one","item_id":"item-one"}}}),owner_ref:None },
        WriteRecord { reference:"cwp://fixture/declarations/one".into(),kind:RecordKind::UpstreamDeclaration,properties:json!({"record":{"actor_ref":"cwp://fixture/actors/remote"}}),owner_ref:None },
    ]);
    for (source, kind, target, position) in [
        (
            header().reference,
            MembershipKind::Message,
            "cwp://fixture/messages/two",
            1,
        ),
        (
            header().reference,
            MembershipKind::Activity,
            "cwp://fixture/activities/commentary",
            2,
        ),
        (
            "cwp://fixture/actors/remote".into(),
            MembershipKind::UpstreamDeclaration,
            "cwp://fixture/declarations/one",
            0,
        ),
    ] {
        write.changes.memberships.push(MembershipChange::Upsert {
            source_ref: source,
            kind,
            member: Member {
                target_ref: target.into(),
                position,
                properties: json!({}),
            },
        });
    }
    store.commit(&write, &Allow).unwrap();
    let page = store
        .read(
            &request(Selection::ConversationMetadata {
                conversation_ref: Some(header().reference),
            }),
            &Allow,
        )
        .unwrap();
    assert!(page
        .records
        .iter()
        .any(|r| r.reference == "cwp://fixture/messages/one"));
    assert!(!page
        .records
        .iter()
        .any(|r| r.reference == "cwp://fixture/messages/two"));
    assert_eq!(
        page.relationships
            .iter()
            .filter(|r| r.kind == "message")
            .count(),
        1
    );
    let page = store
        .read(
            &request(Selection::Commentary {
                provider_integration_ref: "cwp://fixture/integrations/provider".into(),
                response_or_turn_id: "turn-one".into(),
                item_id: "item-one".into(),
            }),
            &Allow,
        )
        .unwrap();
    assert_eq!(page.records.len(), 1);
    assert_eq!(page.relationships.len(), 1);
    assert_eq!(
        page.records[0].reference,
        "cwp://fixture/activities/commentary"
    );
    let page = store.read(&request(Selection::Bootstrap), &Allow).unwrap();
    assert!(
        page.relationships
            .iter()
            .any(|r| r.kind == "upstream_declaration"
                && r.source_ref == "cwp://fixture/actors/remote")
    );
}
