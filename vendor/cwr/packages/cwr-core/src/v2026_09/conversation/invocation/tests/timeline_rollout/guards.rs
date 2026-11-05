//! Activation guards for ambiguous retained invocation placements.
use super::*;

fn retain_repeated_root_invocation(store: &mut ConversationAdmissionStore) {
    let ConversationAdmissionOutcome::Started(_) = store.commit(prepared_root()).unwrap() else {
        panic!("manual root admission must start");
    };
    let timeline_ref = format!("{CONVERSATION_REF}/repeated-parent-timeline");
    store
        .create_retained_timeline(CONVERSATION_REF, &timeline_ref)
        .unwrap();
    store
        .publish_timeline_message(
            CONVERSATION_REF,
            &format!("{CONVERSATION_REF}/publications/source"),
            SOURCE_MESSAGE_REF,
            ConversationTimelineDisposition::Root,
        )
        .unwrap();
    for name in ["root-first", "root-second"] {
        store
            .retain_invocation_timeline_publication(
                ROOT_INVOCATION_REF,
                SOURCE_MESSAGE_REF,
                &format!("{CONVERSATION_REF}/publications/{name}"),
                ConversationTimelineDisposition::Root,
            )
            .unwrap();
    }
}

#[test]
fn managed_rollout_rejects_repeated_parents_before_activation() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("repeated-parent-activation.sqlite");
    let mut store = open(&path);
    retain_repeated_root_invocation(&mut store);
    let revision_before = store.commit_revision;

    let error = store
        .configure_timeline_rollout(ConversationTimelineRollout::NewConversations)
        .expect_err("managed rollout must reject an ambiguous parent before activation");
    assert!(
        error
            .to_string()
            .contains("cannot manage repeated invocation placements")
    );
    assert_eq!(store.commit_revision, revision_before);
    drop(store);

    let error = match ConversationAdmissionStore::open_with_timeline_rollout(
        &path,
        RUNTIME_ROOT,
        ConversationTimelineRollout::NewConversations,
    ) {
        Ok(_) => panic!("managed startup must reject an ambiguous retained parent"),
        Err(error) => error,
    };
    assert!(
        error
            .to_string()
            .contains("cannot manage repeated invocation placements")
    );
    assert_eq!(
        cwpdb_store::test_support::invocation_reads::commit_revision(&path, RUNTIME_ROOT).unwrap(),
        revision_before
    );
}

#[test]
fn managed_rollout_rejects_a_later_repeated_parent_append() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("repeated-parent-append.sqlite");
    let mut store = open(&path);
    store
        .configure_timeline_rollout(ConversationTimelineRollout::NewConversations)
        .unwrap();
    let ConversationAdmissionOutcome::Started(_) = store.commit(prepared_root()).unwrap() else {
        panic!("managed root admission must start");
    };

    let error = store
        .retain_invocation_timeline_publication(
            ROOT_INVOCATION_REF,
            SOURCE_MESSAGE_REF,
            &format!("{CONVERSATION_REF}/publications/repeated-root"),
            ConversationTimelineDisposition::Root,
        )
        .expect_err("managed rollout must reject a repeated parent append");
    assert!(
        error
            .to_string()
            .contains("cannot manage repeated invocation placements")
    );
    let timeline = store.retained_timeline(CONVERSATION_REF).unwrap();
    assert_eq!(
        timeline
            .connections
            .iter()
            .flat_map(|connection| &connection.items)
            .filter(|item| item.content.reference() == ROOT_INVOCATION_REF)
            .count(),
        1
    );
    assert_eq!(
        store
            .invocation_record(ROOT_INVOCATION_REF)
            .unwrap()
            .timeline_publications
            .len(),
        1
    );
}
