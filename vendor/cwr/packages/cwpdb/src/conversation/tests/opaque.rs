use super::*;
#[test]
fn opaque_invocation_null_false_array_and_nested_null_survive_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("opaque.sqlite3");
    let mut store = NativeConversationStore::open(&path, scope(), options()).unwrap();
    let before = store.read(&request(Selection::Bootstrap), &Allow).unwrap();
    let reference = "cwp://fixture/invocations/opaque";
    let record = json!({"input":null,"enabled":false,"items":[],"nested":{"kept":null}});
    let mutation = Mutation {
        scope: scope(),
        request_id: "opaque-record".into(),
        expected_revision: before.revision,
        changes: Changes {
            records: vec![WriteRecord {
                reference: reference.into(),
                kind: RecordKind::Invocation,
                properties: json!({"record":record}),
                owner_ref: None,
            }],
            ..Changes::default()
        },
        journal: None,
        authority_guards: Vec::new(),
    };
    store.commit(&mutation, &Allow).unwrap();
    drop(store);
    let mut store = NativeConversationStore::open(&path, scope(), options()).unwrap();
    assert_eq!(
        store
            .get(&scope(), reference, &Allow)
            .unwrap()
            .unwrap()
            .properties["record"],
        record
    );
}

#[test]
fn provider_child_metadata_never_creates_native_invocation_authority() {
    let mut store = NativeConversationStore::in_memory(scope(), options()).unwrap();
    let before = store.read(&request(Selection::Bootstrap), &Allow).unwrap();
    let operation = "cwp://fixture/tools/operation";
    let child = "cwp://fixture/provider/opaque-child";
    let mutation = Mutation {
        scope: scope(),
        request_id: "provider-child-metadata".into(),
        expected_revision: before.revision,
        changes: Changes {
            records: vec![WriteRecord {
                reference: operation.into(),
                kind: RecordKind::ToolOperation,
                properties: json!({"record":{"activity":{"child_invocation_ref":child}}}),
                owner_ref: None,
            }],
            memberships: vec![MembershipChange::Upsert {
                source_ref: operation.into(),
                kind: MembershipKind::ChildInvocation,
                member: Member {
                    target_ref: child.into(),
                    position: 0,
                    properties: json!({}),
                },
            }],
            ..Changes::default()
        },
        journal: None,
        authority_guards: Vec::new(),
    };
    let committed = store.commit(&mutation, &Allow).unwrap();
    assert!(store.get(&scope(), child, &Allow).unwrap().is_none());
    let revision = match committed {
        CommitOutcome::Committed { revision, .. } => revision,
        _ => panic!("committed"),
    };
    let wrong = "cwp://fixture/messages/collision";
    let mut conflicting = mutation;
    conflicting.request_id = "provider-child-wrong-kind".into();
    conflicting.expected_revision = revision;
    conflicting.changes.records.push(WriteRecord {
        reference: wrong.into(),
        kind: RecordKind::Message,
        properties: json!({"record":{"message_ref":wrong}}),
        owner_ref: None,
    });
    conflicting.changes.memberships = vec![MembershipChange::Upsert {
        source_ref: operation.into(),
        kind: MembershipKind::ChildInvocation,
        member: Member {
            target_ref: wrong.into(),
            position: 0,
            properties: json!({}),
        },
    }];
    assert!(store
        .commit(&conflicting, &Allow)
        .unwrap_err()
        .to_string()
        .contains("invalid endpoint"));
    assert!(store.get(&scope(), wrong, &Allow).unwrap().is_none());
}

#[test]
fn semantic_turn_anchor_is_distinct_from_projected_activity() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("projection.sqlite3");
    let mut store = NativeConversationStore::open(&path, scope(), options()).unwrap();
    let before = store.read(&request(Selection::Bootstrap), &Allow).unwrap();
    let mut mutation = mutation(before.revision);
    let conversation = header().reference;
    let invocation = "cwp://fixture/invocations/turn";
    let activity = "cwp://fixture/activities/progress";
    mutation.changes.records.push(WriteRecord{reference:invocation.into(),kind:RecordKind::Invocation,properties:json!({"record":{"input":null,"state":"running","conversation_ref":conversation}}),owner_ref:None});
    mutation.changes.publications.push(PublicationSuffix {
        conversation_ref:conversation.clone(),from_position:0,
        items:vec![PublishedItem::Message{message_ref:"cwp://fixture/messages/one".into(),position:0},PublishedItem::Turn{invocation_ref:invocation.into(),position:1}],
        activities:vec![PublishedActivity{activity_ref:activity.into(),position:0,record:json!({"activity_ref":activity,"event":{"kind":"work_update","invocation_ref":invocation,"source_message_ref":"cwp://fixture/messages/one"}})}],remove_activities:Vec::new(),
    });
    store.commit(&mutation, &Allow).unwrap();
    let db = rusqlite::Connection::open(path).unwrap();
    let items=db.prepare("SELECT json_extract(e.props,'$.item_kind'),r.canonical_ref FROM edges e JOIN ref_bindings r ON r.ref_id=e.target_id WHERE e.kind='timeline_item' ORDER BY e.position").unwrap().query_map([],|row|Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?))).unwrap().collect::<Result<Vec<_>,_>>().unwrap();
    assert_eq!(
        items,
        vec![
            ("message".into(), "cwp://fixture/messages/one".into()),
            ("turn".into(), invocation.into())
        ]
    );
    assert_eq!(db.query_row("SELECT count(*) FROM edges WHERE kind='timeline_activity' AND target_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1)",[activity],|row|row.get::<_,i64>(0)).unwrap(),1);
    assert_eq!(db.query_row("SELECT count(*) FROM edges WHERE kind='timeline_item' AND target_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1)",[activity],|row|row.get::<_,i64>(0)).unwrap(),0);
}
