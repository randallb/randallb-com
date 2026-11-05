#[test]
fn explicit_timeline_round_trips_through_the_canonical_flat_reader() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("explicit-timeline.sqlite");
    let root = "cwp://com.example.runtime";
    let mut store = ConversationAdmissionStore::open(&path, root).unwrap();
    let turn = working_turn(&mut store, None, "explicit-timeline-retention");
    let conversation_ref = turn.record.conversation_ref.clone();
    let source_message_ref = turn.record.source_message_ref.clone();
    let invocation_ref = turn.record.invocation_ref.clone().unwrap();
    let root_connection_ref = format!("{conversation_ref}/timeline");
    store
        .create_retained_timeline(&conversation_ref, &root_connection_ref)
        .unwrap();
    store
        .publish_timeline_message(
            &conversation_ref,
            &format!("{conversation_ref}/publications/source"),
            &source_message_ref,
            ConversationTimelineDisposition::Root,
        )
        .unwrap();
    let invocation_item = match store
        .retain_invocation_timeline_publication(
            &invocation_ref,
            &source_message_ref,
            &format!("{conversation_ref}/publications/invocation"),
            ConversationTimelineDisposition::Root,
        )
        .unwrap()
    {
        ConversationTimelinePublishOutcome::Placed(item) => item,
        outcome => panic!("unexpected publication outcome {outcome:?}"),
    };
    let duplicate_invocation_item = store
        .retain_invocation_timeline_publication(
            &invocation_ref,
            &source_message_ref,
            &format!("{conversation_ref}/publications/invocation-again"),
            ConversationTimelineDisposition::Root,
        )
        .unwrap();
    assert!(matches!(
        duplicate_invocation_item,
        ConversationTimelinePublishOutcome::Placed(_)
    ));
    let undisclosed_publication_ref = format!("{conversation_ref}/publications/undisclosed");
    assert_eq!(
        store
            .retain_invocation_timeline_publication(
                &invocation_ref,
                &source_message_ref,
                &undisclosed_publication_ref,
                ConversationTimelineDisposition::Undisclosed,
            )
            .unwrap(),
        ConversationTimelinePublishOutcome::Undisclosed
    );
    store
        .publish_timeline_message(
            &conversation_ref,
            &format!("{conversation_ref}/publications/child-message"),
            &source_message_ref,
            ConversationTimelineDisposition::Child {
                parent_item_ref: invocation_item.item_ref,
            },
        )
        .unwrap();
    let timeline = store.retained_timeline(&conversation_ref).unwrap().clone();

    let read_flat = |store: &ConversationAdmissionStore| {
        let mut reader = store.read_store().unwrap();
        let page = reader
            .begin_operation(std::time::Duration::from_secs(2))
            .unwrap()
            .conversation_timeline(
                &conversation_ref,
                ConversationRecordWindow {
                    first: Some(10),
                    ..Default::default()
                },
            )
            .unwrap();
        page
    };
    let canonical_flat_page = read_flat(&store);
    assert_eq!(canonical_flat_page.total_count, 4);
    assert!(canonical_flat_page.revision.starts_with("canonical:"));
    assert_eq!(
        canonical_flat_page
            .anchors
            .iter()
            .map(|anchor| match anchor {
                ConversationTimelineAnchor::Message { message_ref, .. } => {
                    ("message", message_ref.as_str())
                }
                ConversationTimelineAnchor::Turn { invocation_ref, .. } => {
                    ("turn", invocation_ref.as_str())
                }
            })
            .collect::<Vec<_>>(),
        vec![
            ("message", source_message_ref.as_str()),
            ("turn", invocation_ref.as_str()),
            ("message", source_message_ref.as_str()),
            ("turn", invocation_ref.as_str()),
        ]
    );
    drop(store);

    cwpdb_store::test_support::conversation_claims::ownerless_publication(&path, root, &conversation_ref, &undisclosed_publication_ref).unwrap();

    let reopened = ConversationAdmissionStore::open(&path, root).unwrap();
    assert_eq!(
        cwpdb_store::test_support::timeline_persistence::publication_claim(&path, &undisclosed_publication_ref).unwrap(),
        Some(crate::cwpdb::IdentityClaim {
            kind: "conversation_timeline_publication".into(),
            owner_ref: Some(conversation_ref.clone()),
        })
    );
    assert_eq!(
        reopened
            .conversation(&conversation_ref)
            .unwrap()
            .timeline_connection_ref
            .as_deref(),
        Some(root_connection_ref.as_str())
    );
    assert_eq!(reopened.retained_timeline(&conversation_ref), Some(&timeline));
    let invocation = reopened.invocation_record(&invocation_ref).unwrap();
    assert_eq!(
        invocation.source_message_ref.as_deref(),
        Some(source_message_ref.as_str())
    );
    assert_eq!(invocation.timeline_publications.len(), 3);
    let reopened_flat_page = read_flat(&reopened);
    assert_eq!(
        reopened_flat_page.anchors,
        canonical_flat_page.anchors
    );
    drop(reopened);

    cwpdb_store::test_support::timeline_persistence::assert_flat_retained_layout(&path, &conversation_ref, &invocation_ref, &undisclosed_publication_ref).unwrap();
}

#[test]
fn failed_cwpdb_placement_rolls_back_the_invocation_decision_and_timeline() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("explicit-timeline-rollback.sqlite");
    let root = "cwp://com.example.runtime";
    let mut store = ConversationAdmissionStore::open(&path, root).unwrap();
    let turn = working_turn(&mut store, None, "explicit-timeline-rollback");
    let conversation_ref = turn.record.conversation_ref.clone();
    let source_message_ref = turn.record.source_message_ref.clone();
    let invocation_ref = turn.record.invocation_ref.clone().unwrap();
    let root_connection_ref = format!("{conversation_ref}/timeline");
    store
        .create_retained_timeline(&conversation_ref, &root_connection_ref)
        .unwrap();
    let publication_ref = format!("{conversation_ref}/publications/invocation");
    let item_ref = format!("{publication_ref}/item");
    cwpdb_store::test_support::timeline_persistence::seed_message_identity_collision(&path, &item_ref).unwrap();

    let error = store
        .retain_invocation_timeline_publication(
            &invocation_ref,
            &source_message_ref,
            &publication_ref,
            ConversationTimelineDisposition::Root,
        )
        .unwrap_err();
    assert!(error
        .to_string()
        .contains("schema or kind conflict"));
    let invocation = store.invocation_record(&invocation_ref).unwrap();
    assert!(invocation.source_message_ref.is_none());
    assert!(invocation.timeline_publications.is_empty());
    assert!(store
        .retained_timeline(&conversation_ref)
        .unwrap()
        .connection(&root_connection_ref)
        .unwrap()
        .items
        .is_empty());
    drop(store);

    let reopened = ConversationAdmissionStore::open(&path, root).unwrap();
    let invocation = reopened.invocation_record(&invocation_ref).unwrap();
    assert!(invocation.source_message_ref.is_none());
    assert!(invocation.timeline_publications.is_empty());
    assert!(reopened
        .retained_timeline(&conversation_ref)
        .unwrap()
        .connection(&root_connection_ref)
        .unwrap()
        .items
        .is_empty());
}

#[test]
fn message_publication_requires_a_durable_disclosed_placement() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = ConversationAdmissionStore::open(
        directory.path().join("message-disclosure.sqlite"),
        "cwp://com.example.runtime",
    )
    .unwrap();
    let turn = working_turn(&mut store, None, "message-disclosure");
    let conversation_ref = turn.record.conversation_ref;
    let root_connection_ref = format!("{conversation_ref}/timeline");
    store
        .create_retained_timeline(&conversation_ref, &root_connection_ref)
        .unwrap();
    let error = store
        .publish_timeline_message(
            &conversation_ref,
            &format!("{conversation_ref}/publications/private-message"),
            &turn.record.source_message_ref,
            ConversationTimelineDisposition::Undisclosed,
        )
        .unwrap_err();
    assert!(error.to_string().contains("cannot be undisclosed"));
    assert!(store
        .retained_timeline(&conversation_ref)
        .unwrap()
        .connection(&root_connection_ref)
        .unwrap()
        .items
        .is_empty());
}

#[test]
fn invocation_publication_requires_the_retained_causal_source() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = ConversationAdmissionStore::open(
        directory.path().join("causal-source.sqlite"),
        "cwp://com.example.runtime",
    )
    .unwrap();
    let first = working_turn(&mut store, None, "causal-source-first");
    let conversation_ref = first.record.conversation_ref.clone();
    let second = working_turn(
        &mut store,
        Some(&conversation_ref),
        "causal-source-second",
    );
    store
        .create_retained_timeline(&conversation_ref, &format!("{conversation_ref}/timeline"))
        .unwrap();
    let invocation_ref = first.record.invocation_ref.unwrap();
    let error = store
        .retain_invocation_timeline_publication(
            &invocation_ref,
            &second.record.source_message_ref,
            &format!("{conversation_ref}/publications/wrong-source"),
            ConversationTimelineDisposition::Root,
        )
        .unwrap_err();
    assert!(error.to_string().contains("retained turn start"));
    let invocation = store.invocation_record(&invocation_ref).unwrap();
    assert!(invocation.source_message_ref.is_none());
    assert!(invocation.timeline_publications.is_empty());
}

#[test]
fn transfer_and_fork_reject_untransferred_timeline_state() {
    let directory = tempfile::tempdir().unwrap();
    let graph = fixture_graph_with_reviewer();
    let person = crate::v2026_09::test_fixture::actor_ref("person");
    let mut store = ConversationAdmissionStore::open(
        directory.path().join("timeline-transfer.sqlite"),
        "cwp://com.example.runtime",
    )
    .unwrap();
    let turn = working_turn(&mut store, None, "timeline-transfer");
    let conversation_ref = turn.record.conversation_ref;
    let source_message_ref = turn.record.source_message_ref;
    store
        .create_retained_timeline(&conversation_ref, &format!("{conversation_ref}/timeline"))
        .unwrap();

    let export = store.export_conversation(&conversation_ref, person).unwrap();
    assert!(export.conversation.messages.is_empty());
    assert!(export.timeline.is_some());

    let import_error = store
        .import_conversation(
            &graph,
            person,
            ConversationExport {
                timeline: None,
                version: super::transfer::EXPORT_VERSION.into(),
                exported_at_unix_ms: 1,
                conversation: store.conversation(&conversation_ref).unwrap().clone(),
            },
            Some("timeline-import"),
        )
        .unwrap_err();
    assert!(import_error.to_string().contains("timeline transfer"));

    let fork_error = store
        .fork_conversation(
            &graph,
            person,
            &conversation_ref,
            &source_message_ref,
            "timeline-fork",
        )
        .unwrap_err();
    assert!(fork_error.to_string().contains("not published"));
}

#[test]
fn new_message_refs_cannot_reuse_timeline_identities() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("timeline-identity-collision.sqlite");
    let root = "cwp://com.example.runtime";
    let mut store = ConversationAdmissionStore::open(&path, root).unwrap();
    let turn = working_turn(&mut store, None, "timeline-identity-collision");
    let conversation_ref = turn.record.conversation_ref.clone();
    let source_message_ref = turn.record.source_message_ref.clone();
    let invocation_ref = turn.record.invocation_ref.clone().unwrap();
    store
        .create_retained_timeline(&conversation_ref, &format!("{conversation_ref}/timeline"))
        .unwrap();
    let disclosed_ref = format!("{conversation_ref}/publications/disclosed");
    store
        .publish_timeline_message(
            &conversation_ref,
            &disclosed_ref,
            &source_message_ref,
            ConversationTimelineDisposition::Root,
        )
        .unwrap();
    let error = store
        .transact(|state| {
            let conversation = state
                .conversations
                .iter_mut()
                .find(|conversation| conversation.conversation_ref == conversation_ref)
                .unwrap();
            let mut message = conversation.messages[0].clone();
            message.message_ref = disclosed_ref.clone();
            conversation.messages.push(message);
            Ok(())
        })
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("reuses a retained timeline identity"),
        "{error}"
    );
    drop(store);

    let reopened = ConversationAdmissionStore::open(&path, root).unwrap();
    assert_eq!(
        reopened
            .invocation_record(&invocation_ref)
            .unwrap()
            .invocation_ref,
        invocation_ref
    );
    assert!(!reopened
        .conversation(&conversation_ref)
        .unwrap()
        .messages
        .iter()
        .any(|message| message.message_ref == disclosed_ref));
}

#[test]
fn explicit_timeline_persists_an_older_writer_fence() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("timeline-writer-fence.sqlite");
    let root = "cwp://com.example.runtime";
    let mut store = ConversationAdmissionStore::open(&path, root).unwrap();
    let retained_version = |_: &ConversationAdmissionStore| cwpdb_store::test_support::timeline_persistence::state_version(&path).unwrap();
    assert_eq!(retained_version(&store), super::STATE_VERSION);
    let turn = working_turn(&mut store, None, "timeline-writer-fence");
    assert_eq!(retained_version(&store), super::STATE_VERSION);
    store
        .create_retained_timeline(
            &turn.record.conversation_ref,
            &format!("{}/timeline", turn.record.conversation_ref),
        )
        .unwrap();
    assert_eq!(
        retained_version(&store),
        super::EXPLICIT_TIMELINE_STATE_VERSION
    );
    assert_ne!(
        super::EXPLICIT_TIMELINE_STATE_VERSION,
        super::STATE_VERSION,
        "older writers accepted only the legacy runtime state version"
    );
    drop(store);

    let reopened = ConversationAdmissionStore::open(&path, root).unwrap();
    drop(reopened);
    cwpdb_store::test_support::timeline_persistence::erase_explicit_timeline_writer_fence(&path).unwrap();
    let error = ConversationAdmissionStore::open(&path, root).err().unwrap();
    assert!(error
        .to_string()
        .contains("retained conversation timelines require"));
}

#[test]
fn timeline_root_cannot_reuse_any_existing_graph_record() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("timeline-root-collision.sqlite");
    let root = "cwp://com.example.runtime";
    let mut store = ConversationAdmissionStore::open(&path, root).unwrap();
    let first = working_turn(&mut store, None, "timeline-root-collision-first");
    let second = working_turn(&mut store, None, "timeline-root-collision-second");
    let edge_kind = "timeline-root-collision";
    let edge_ref = crate::cwpdb::derived_ref(
        edge_kind,
        &[
            &first.record.conversation_ref,
            &second.record.conversation_ref,
        ],
    );
    cwpdb_store::test_support::timeline_persistence::seed_timeline_root_edge_collision(&path, &first.record.conversation_ref, &second.record.conversation_ref).unwrap();

    for collision_ref in [
        first.record.conversation_ref.as_str(),
        second.record.conversation_ref.as_str(),
        second.record.invocation_ref.as_deref().unwrap(),
        root,
        "cwp://com.example.runtime/cwpdb",
        edge_ref.as_str(),
    ] {
        let error = store
            .create_retained_timeline(&first.record.conversation_ref, collision_ref)
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("already retained by another CWPDB record"),
            "{error}"
        );
        assert!(store
            .retained_timeline(&first.record.conversation_ref)
            .is_none());
    }

    let timeline_ref = format!("{}/timeline", first.record.conversation_ref);
    assert!(store
        .create_retained_timeline(&first.record.conversation_ref, &timeline_ref)
        .unwrap());
    drop(store);
    let reopened = ConversationAdmissionStore::open(&path, root).unwrap();
    assert_eq!(
        reopened
            .retained_timeline(&first.record.conversation_ref)
            .unwrap()
            .root_connection_ref,
        timeline_ref
    );
}
