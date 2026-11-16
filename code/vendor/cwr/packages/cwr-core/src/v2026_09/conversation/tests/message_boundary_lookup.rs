#[test]
fn message_boundary_lookup_repairs_retained_edges_and_preserves_timestamp_scope() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("message-boundary.sqlite");
    let runtime = "cwp://example.test/message-boundary";
    let person = crate::v2026_09::test_fixture::actor_ref("person");
    let conversation = format!("{runtime}/conversation");
    let other = format!("{runtime}/other");
    let message = format!("{runtime}/message");
    let missing = format!("{runtime}/without-boundary");
    let different = format!("{runtime}/different-message");
    let mut store = ConversationAdmissionStore::open(&path, runtime).unwrap();
    let mut state = store.state.clone();
    let boundary = |id: &str, sequence: u64, occurred: u64| {
        json!({
            "activity_ref":format!("{runtime}/activities/{id}"),"sequence":sequence,"occurred_at_unix_ms":occurred,
            "event":{"kind":"message_recorded","message_ref":message}
        })
    };
    state.conversations = vec![
        serde_json::from_value(json!({"conversation_ref":conversation,"participant_actor_refs":[person],
            "messages":[{"message_ref":message,"author_actor_ref":person,"addressed_actor_refs":[],"text":"Timed"},
                {"message_ref":missing,"author_actor_ref":person,"addressed_actor_refs":[],"text":"No historical timestamp"},
                {"message_ref":different,"author_actor_ref":person,"addressed_actor_refs":[],"text":"Different message"}],
            "activity":[
                {"activity_ref":format!("{runtime}/activities/not-boundary"),"sequence":1,"occurred_at_unix_ms":1,
                    "event":{"kind":"message_work_association","message_ref":message,"activity_ref":format!("{runtime}/activities/first")}},
                {"activity_ref":format!("{runtime}/activities/different"),"sequence":2,"occurred_at_unix_ms":2,
                    "event":{"kind":"message_recorded","message_ref":different}},
                boundary("first",3,90),boundary("later",4,10)]
        })).unwrap(),
        // A historical event mentioning the same resource does not transfer its
        // timestamp across the owning conversation membership.
        serde_json::from_value(json!({"conversation_ref":other,"participant_actor_refs":[person],
            "messages":[],"activity":[boundary("foreign",1,5)]})).unwrap(),
    ].into();
    store.replace_state(state).unwrap();
    let retained = || cwpdb_store::test_support::message_boundaries::retained_records(&path, runtime).unwrap();
    let before = retained();
    cwpdb_store::test_support::message_boundaries::remove_derived_lookup(&path, runtime).unwrap();
    drop(store);
    // This is the supported owning-writer reopen, with older derived indexes
    // missing. A read never repairs or rewrites retained history.
    let store = ConversationAdmissionStore::open(&path, runtime).unwrap();
    assert_eq!(retained(), before);
    // A stale derived lookup is only a candidate. Its payload still names a
    // different message and must never supply this message's earlier timestamp.
    cwpdb_store::test_support::message_boundaries::stale_lookup(
        &path, runtime, &format!("{runtime}/activities/different"), &message,
    ).unwrap();
    let snapshot = ConversationPublicationSnapshot::open(
        &path,
        runtime,
        person,
        std::time::Instant::now() + std::time::Duration::from_secs(10),
        ConversationReadCancellation::pending(),
    )
    .unwrap();
    assert_eq!(
        snapshot
            .message_metadata(&message)
            .unwrap()
            .unwrap()
            .occurred_at_unix_ms,
        Some(90)
    );
    assert_eq!(
        snapshot
            .message_metadata(&missing)
            .unwrap()
            .unwrap()
            .occurred_at_unix_ms,
        None
    );
    let links = cwpdb_store::test_support::message_boundaries::lookup_count(&path, runtime, None).unwrap();
    assert_eq!(links, 5);
    drop(snapshot);
    drop(store);
    let _store = ConversationAdmissionStore::open(&path, runtime).unwrap();
    assert_eq!(retained(), before);
}

#[test]
fn message_boundary_lookup_ignores_foreign_activity_records() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("foreign-message-boundary.sqlite");
    let runtime = "cwp://example.test/message-boundary-ownership";
    drop(ConversationAdmissionStore::open(&path, runtime).unwrap());

    let catalog = crate::entities::EntityCatalog::from_toml(
        r#"
type = "cwp://org.coworkerprotocol/2026-10-22/schemas/entity-definition"
ref = "cwp://example.test/definitions/foreign-activity"
storage = "cwpdb"
record_kind = "activity"
record_schema = "cwp://org.coworkerprotocol/cwpdb/schemas/activity/2026-09"
value_schema = { type = "object" }
"#,
    )
    .unwrap();
    let activity = "cwp://example.test/foreign/activities/message-recorded";
    let value = json!({
        "event": {
            "kind": "message_recorded",
            "message_ref": "cwp://example.test/foreign/messages/one"
        }
    });
    let mut native = crate::cwpdb::NativeStore::open(&path).unwrap();
    catalog
        .create(
            &mut native,
            "cwp://example.test/definitions/foreign-activity",
            activity,
            &value,
        )
        .unwrap();
    drop(native);

    let _store = ConversationAdmissionStore::open(&path, runtime).unwrap();
    let repaired_links = cwpdb_store::test_support::message_boundaries::lookup_count(&path, runtime, Some(activity)).unwrap();
    assert_eq!(repaired_links, 0);
}
