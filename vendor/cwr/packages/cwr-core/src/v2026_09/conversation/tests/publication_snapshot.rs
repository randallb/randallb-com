#[test]
fn publication_snapshot_keeps_nested_pages_and_sources_at_one_commit() {
    let (_directory, mut store, conversation, message, invocation, root, parent) =
        populated_canonical_timeline();
    let (path, runtime) = store.read_store_identity();
    let open = || ConversationPublicationSnapshot::open(
        &path, &runtime, crate::v2026_09::test_fixture::actor_ref("person"),
        std::time::Instant::now() + std::time::Duration::from_secs(10),
        ConversationReadCancellation::pending()).unwrap();
    let snapshot = open();
    let request = ConversationTimelinePageRequest { first: Some(10), ..Default::default() };
    let before = snapshot.page(&conversation, None, &request, true).unwrap().unwrap();
    assert_eq!(before.total_count, Some(3));
    assert_eq!(before.publications[0].item.content.reference(), message);
    assert!(before.publications.iter().all(|p| p.source.is_some()));
    let claimed = match store.claim_invocation(&invocation, 100).unwrap() {
        ConversationInvocationClaimOutcome::Claimed(claimed) => claimed,
        other => panic!("unexpected claim: {other:?}"),
    };
    settle_working(&mut store, &claimed);
    store.publish_timeline_message(&conversation, &format!("{conversation}/publications/later"),
        &message, ConversationTimelineDisposition::Child { parent_item_ref: parent.item_ref.clone() }).unwrap();
    let old_children = snapshot.page(&conversation, parent.child_connection_ref.as_deref(), &request, true).unwrap().unwrap();
    assert_eq!(old_children.total_count, Some(1));
    let old_invocation = snapshot.item(&parent.item_ref).unwrap().unwrap();
    let ConversationResource::Invocation { invocation: old } = old_invocation.source.unwrap().resource else { panic!("expected invocation") };
    assert_eq!(old.state, crate::v2026_09::invocation::ActionInvocationState::Admitted);
    assert_eq!(before.revision, snapshot.page(&conversation, Some(&root), &request, false).unwrap().unwrap().revision);
    let fresh = open();
    let children = fresh.page(&conversation, parent.child_connection_ref.as_deref(), &request, true).unwrap().unwrap();
    assert_eq!(children.total_count, Some(2));
    let current = fresh.item(&parent.item_ref).unwrap().unwrap();
    let ConversationResource::Invocation { invocation: current } = current.source.unwrap().resource else { panic!("expected invocation") };
    assert!(current.state.is_terminal());
    assert_ne!(before.revision, fresh.page(&conversation, None, &request, true).unwrap().unwrap().revision);
}

#[test]
fn publication_snapshot_preserves_empty_roots_and_enforces_cancellation() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("empty-publication.sqlite");
    let root = "cwp://com.example.runtime";
    let mut store = ConversationAdmissionStore::open(&path, root).unwrap();
    let turn = working_turn(&mut store, None, "empty-publication");
    let conversation = turn.record.conversation_ref.clone();
    store.create_retained_timeline(&conversation, &format!("{conversation}/timeline")).unwrap();
    let cancellation = ConversationReadCancellation::pending();
    let snapshot = ConversationPublicationSnapshot::open(&path, root,
        crate::v2026_09::test_fixture::actor_ref("person"),
        std::time::Instant::now() + std::time::Duration::from_secs(10), cancellation.clone()).unwrap();
    let request = ConversationTimelinePageRequest { first: Some(1), ..Default::default() };
    let page = snapshot.page(&conversation, None, &request, true).unwrap().unwrap();
    assert_eq!(page.total_count, Some(0));
    assert!(page.publications.is_empty());
    assert!(snapshot.resource(&turn.record.source_message_ref).unwrap().is_none());
    cancellation.cancel();
    assert!(snapshot.resource(&turn.record.source_message_ref).is_err());
    assert!(snapshot.page(&conversation, None, &request, false).is_err());
    drop(snapshot);
    // A cancelled reader must leave the writer usable.
    store.publish_timeline_message(&conversation, &format!("{conversation}/publications/source"),
        &turn.record.source_message_ref, ConversationTimelineDisposition::Root).unwrap();
}

#[test]
fn publication_snapshot_unavailable_source_keeps_its_placement_and_presentation() {
    let (_directory, store, conversation, message, _invocation, _root, _parent) =
        populated_canonical_timeline();
    // Model a retained placement whose source record is no longer available.
    // Publication identity and topology remain independently retained.
    let (path, runtime) = store.read_store_identity();
    cwpdb_store::test_support::publication_faults::detach_source_message(&path,&runtime,&conversation,&message).unwrap();
    let snapshot = ConversationPublicationSnapshot::open(&path, &runtime,
        crate::v2026_09::test_fixture::actor_ref("person"),
        std::time::Instant::now() + std::time::Duration::from_secs(10),
        ConversationReadCancellation::pending()).unwrap();
    let page = snapshot.page(&conversation, None, &ConversationTimelinePageRequest {
        first: Some(1), ..Default::default()
    }, false).unwrap().unwrap();
    assert_eq!(page.publications.len(), 1);
    assert_eq!(page.total_count, None);
    assert!(page.publications[0].source.is_none());
    assert_eq!(page.publications[0].text, "Content unavailable");
    let direct = snapshot.item(&page.publications[0].item.item_ref).unwrap().unwrap();
    assert_eq!(direct.item, page.publications[0].item);
    assert!(direct.source.is_none());
}

#[test]
fn publication_snapshot_reuses_resource_identity_and_isolates_authority() {
    let (_directory, store, _conversation, message, _invocation, _root, _parent) = populated_canonical_timeline();
    let (path, runtime) = store.read_store_identity();
    let open = |actor| ConversationPublicationSnapshot::open(&path, &runtime, actor,
        std::time::Instant::now()+std::time::Duration::from_secs(10), ConversationReadCancellation::pending()).unwrap();
    let actor = crate::v2026_09::test_fixture::actor_ref("person");
    let snapshot = open(actor);
    let first = snapshot.resource_shared(&message).unwrap().unwrap();
    let second = snapshot.resource_shared(&message).unwrap().unwrap();
    assert!(std::sync::Arc::ptr_eq(&first, &second));
    assert_eq!(snapshot.resource(&message).unwrap().as_deref(), Some(first.as_ref()));
    let fresh = open(actor).resource_shared(&message).unwrap().unwrap();
    assert!(!std::sync::Arc::ptr_eq(&first, &fresh));
    let outsider = open("cwp://com.example/outsider");
    assert!(outsider.resource_shared(&message).unwrap().is_none());
    assert!(outsider.resource_shared(&message).unwrap().is_none());
}

#[test]
fn publication_target_inheritance_is_actor_scoped_and_independent_of_pages() {
    let (_directory, mut store, conversation, message, _invocation, _root, _parent) = populated_canonical_timeline();
    // More than a displayed page of publications must not change the lookup.
    for index in 0..55 {
        store.publish_timeline_message(&conversation, &format!("{conversation}/publications/repeat-{index}"),
            &message, ConversationTimelineDisposition::Root).unwrap();
    }
    let (path, runtime) = store.read_store_identity();
    let open = |actor| ConversationPublicationSnapshot::open(&path, &runtime, actor,
        std::time::Instant::now() + std::time::Duration::from_secs(10), ConversationReadCancellation::pending()).unwrap();
    let person = crate::v2026_09::test_fixture::actor_ref("person");
    let helper = crate::v2026_09::test_fixture::actor_ref("helper");
    assert_eq!(open(person).last_addressed_actor_refs(&conversation).unwrap(), vec![helper.to_string()]);
    assert!(open(helper).last_addressed_actor_refs(&conversation).unwrap().is_empty());
    assert!(open("cwp://com.example/outsider").last_addressed_actor_refs(&conversation).unwrap().is_empty());
}


#[test]
fn publication_target_inheritance_ignores_later_placements_of_old_messages() {
    let (_directory, mut store, conversation, old_message, _invocation, _root, _parent) = populated_canonical_timeline();
    let reviewer = "cwp://com.example.fixture/actors/reviewer";
    let ConversationAdmissionOutcome::Started(turn) = store.admit(
        &fixture_graph_with_reviewer(),
        &ingress("new-recipient", crate::v2026_09::test_fixture::actor_ref("person"), Some(&conversation),
            ConversationIngressAddressing::Addressed { actor_refs: vec![reviewer.into()] }, "@reviewer review this"),
        Some("cwp://com.example.fixture/actor_integrations/reviewer_local_execution"),
    ).unwrap() else { panic!("start reviewer turn") };
    store.publish_timeline_message(&conversation, &format!("{conversation}/publications/new-recipient"),
        &turn.record.source_message_ref, ConversationTimelineDisposition::Root).unwrap();
    store.publish_timeline_message(&conversation, &format!("{conversation}/publications/old-message-again"),
        &old_message, ConversationTimelineDisposition::Root).unwrap();
    assert_eq!(snapshot_target_refs(&store, &conversation), vec![reviewer.to_string()]);
}

#[test]
fn publication_target_inheritance_skips_multi_recipient_messages() {
    let (_directory, mut store, conversation, _message, _invocation, _root, _parent) = populated_canonical_timeline();
    let helper = crate::v2026_09::test_fixture::actor_ref("helper");
    let multi = format!("{conversation}/messages/multiple-recipients");
    // Historical messages can address several actors even though new sends accept one.
    store.transact(|state| {
        let retained = state.conversations.iter_mut().find(|row| row.conversation_ref == conversation).unwrap();
        let mut message = retained.messages[0].clone();
        message.message_ref = multi.clone();
        message.addressed_actor_refs = vec![helper.into(), "cwp://com.example.fixture/actors/reviewer".into()];
        retained.messages.push(message);
        Ok(())
    }).unwrap();
    store.publish_timeline_message(&conversation, &format!("{conversation}/publications/multiple-recipients"),
        &multi, ConversationTimelineDisposition::Root).unwrap();
    assert_eq!(snapshot_target_refs(&store, &conversation), vec![helper.to_string()]);
}

fn snapshot_target_refs(store: &ConversationAdmissionStore, conversation: &str) -> Vec<String> {
    let (path, runtime) = store.read_store_identity();
    ConversationPublicationSnapshot::open(&path, &runtime, crate::v2026_09::test_fixture::actor_ref("person"),
        std::time::Instant::now() + std::time::Duration::from_secs(10), ConversationReadCancellation::pending())
        .unwrap().last_addressed_actor_refs(conversation).unwrap()
}


#[test]
fn publication_type_filter_does_not_hydrate_rejected_message_payloads_or_count_sources() {
    let (_directory, store, conversation, message, invocation, _root, _parent) = populated_canonical_timeline();
    let (path, runtime) = store.read_store_identity();
    let snapshot = ConversationPublicationSnapshot::open(&path, &runtime,
        crate::v2026_09::test_fixture::actor_ref("person"),
        std::time::Instant::now() + std::time::Duration::from_secs(10),
        ConversationReadCancellation::pending()).unwrap();
    let selected = snapshot.filtered_page(&conversation, None,
        &ConversationTimelinePageRequest { last: Some(1), ..Default::default() },
        &ConversationPublicationFilter {
            source_types: vec![ConversationTimelineSourceType::ActionInvocation],
            ..Default::default()
        }, true).unwrap().unwrap();
    assert_eq!(selected.total_count, Some(1));
    assert_eq!(selected.publications.len(), 1);
    assert_eq!(selected.publications[0].item.content.reference(), invocation);
    assert!(!snapshot.resource_was_loaded(&message), "rejected message payload was hydrated");
    assert!(snapshot.resource_was_loaded(&invocation));
}
