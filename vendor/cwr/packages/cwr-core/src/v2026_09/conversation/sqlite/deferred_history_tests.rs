use super::*;
use std::{fs, os::unix::fs::PermissionsExt};

const ROOT: &str = "cwp://example.test/runtime";
const PERSON: &str = "cwp://example.test/actors/person";
const HELPER: &str = "cwp://example.test/actors/helper";

#[test]
fn managed_reopen_defers_history_until_explicit_read_or_mutation() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("deferred-history.sqlite");
    let mut store = ConversationAdmissionStore::open_with_timeline_rollout(
        &path,
        ROOT,
        ConversationTimelineRollout::NewConversations,
    )
    .unwrap();
    let first = store
        .record_host_exchange(
            None,
            PERSON,
            HELPER,
            "First request".into(),
            "First reply".into(),
        )
        .unwrap()
        .conversation;
    let second = store
        .record_host_exchange(
            None,
            PERSON,
            HELPER,
            "Other request".into(),
            "Other reply".into(),
        )
        .unwrap()
        .conversation;
    let first = store
        .rename_conversation(&first.conversation_ref, PERSON, "Saved title")
        .unwrap();
    assert!(first.timeline_connection_ref.is_some());
    assert!(second.timeline_connection_ref.is_some());
    let expected_bootstrap =
        crate::v2026_09::host::project_host_conversation_bootstrap(&store, true).unwrap();
    let expected_metadata =
        crate::v2026_09::host::project_host_conversation_metadata(&store, &second.conversation_ref)
            .unwrap();
    drop(store);

    let mut store = ConversationAdmissionStore::open_with_timeline_rollout_deferred(
        &path,
        ROOT,
        ConversationTimelineRollout::NewConversations,
    )
    .unwrap();
    for expected in [&first, &second] {
        let reference = &expected.conversation_ref;
        assert!(store.state.cold_conversation_refs.contains(reference));
        let header = store.conversation_header(reference).unwrap();
        assert_eq!(header.title, expected.title);
        assert_eq!(
            header.participant_actor_refs,
            expected.participant_actor_refs
        );
        assert!(header.messages.is_empty());
        assert!(header.activity.is_empty());
    }
    assert_eq!(
        crate::v2026_09::host::project_host_conversation_bootstrap(&store, true).unwrap(),
        expected_bootstrap
    );
    assert_eq!(
        crate::v2026_09::host::project_host_conversation_metadata(
            &store,
            &second.conversation_ref,
        )
        .unwrap(),
        expected_metadata
    );
    assert_eq!(store.state.cold_conversation_refs.len(), 2);

    assert_eq!(
        store.full_conversation(&first.conversation_ref).unwrap(),
        Some(first.clone())
    );
    assert!(store
        .state
        .cold_conversation_refs
        .contains(&first.conversation_ref));
    assert!(store
        .state
        .cold_conversation_refs
        .contains(&second.conversation_ref));
    assert!(store
        .conversation_header(&second.conversation_ref)
        .unwrap()
        .messages
        .is_empty());

    let appended = store
        .record_host_exchange(
            Some(&first.conversation_ref),
            PERSON,
            HELPER,
            "Follow up".into(),
            "Follow up reply".into(),
        )
        .unwrap()
        .conversation;
    assert!(store.state.cold_conversation_refs.is_empty());
    assert_eq!(appended.title, first.title);
    assert_eq!(&appended.messages[..first.messages.len()], first.messages);
    assert_eq!(&appended.activity[..first.activity.len()], first.activity);
    assert_eq!(appended.messages.len(), first.messages.len() + 2);
    assert_eq!(store.conversation(&second.conversation_ref), Some(&second));
    drop(store);

    let store = ConversationAdmissionStore::open_with_timeline_rollout(
        &path,
        ROOT,
        ConversationTimelineRollout::NewConversations,
    )
    .unwrap();
    assert_eq!(store.conversation(&first.conversation_ref), Some(&appended));
    assert_eq!(store.conversation(&second.conversation_ref), Some(&second));
}

#[test]
fn legacy_history_forces_eager_reopen_and_permissions_are_private() {
    let directory = tempfile::tempdir().unwrap();
    let parent = directory.path().join("state");
    fs::create_dir(&parent).unwrap();
    let path = parent.join("legacy.sqlite");
    let mut store = ConversationAdmissionStore::open_with_timeline_rollout(
        &path,
        ROOT,
        ConversationTimelineRollout::NewConversations,
    )
    .unwrap();
    let managed = store
        .record_host_exchange(
            None,
            PERSON,
            HELPER,
            "Managed request".into(),
            "Managed reply".into(),
        )
        .unwrap()
        .conversation;
    assert!(managed.timeline_connection_ref.is_some());
    store
        .configure_timeline_rollout(ConversationTimelineRollout::Disabled)
        .unwrap();
    let legacy = store
        .record_host_exchange(
            None,
            PERSON,
            HELPER,
            "Old request".into(),
            "Old reply".into(),
        )
        .unwrap()
        .conversation;
    assert!(legacy.timeline_connection_ref.is_none());
    drop(store);

    fs::set_permissions(&parent, fs::Permissions::from_mode(0o777)).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o666)).unwrap();
    let store = ConversationAdmissionStore::open_with_timeline_rollout_deferred(
        &path,
        ROOT,
        ConversationTimelineRollout::NewConversations,
    )
    .unwrap();
    assert!(store.state.cold_conversation_refs.is_empty());
    assert_eq!(
        store.conversation(&managed.conversation_ref),
        Some(&managed)
    );
    assert_eq!(store.conversation(&legacy.conversation_ref), Some(&legacy));
    assert_eq!(
        fs::metadata(&parent).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

#[test]
fn participant_can_read_attached_file_while_conversation_is_cold() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("cold-file.sqlite");
    let graph = crate::v2026_09::graph::resolve_graph(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures/contracts/agent-recovery-turn/workspace/CWR.toml"),
    )
    .unwrap();
    let person = crate::v2026_09::test_fixture::actor_ref("person");
    let helper = crate::v2026_09::test_fixture::actor_ref("helper");
    let mut store = ConversationAdmissionStore::open_with_timeline_rollout(
        &path,
        ROOT,
        ConversationTimelineRollout::NewConversations,
    )
    .unwrap();
    let file = store
        .ingest_file(person, "brief.txt", "text/plain", b"Private brief")
        .unwrap();
    let input = ConversationIngress {
        owner_context_ref: None,
        file_refs: vec![file.file_ref.clone()],
        integration_ref: "cwp://com.example.fixture/integrations/local".into(),
        provider_event_ref: "cold-file-message".into(),
        author_actor_ref: person.into(),
        conversation: ConversationIngressConversation::Create,
        addressing: ConversationIngressAddressing::Unaddressed,
        text: "Read the attached brief".into(),
    };
    let ConversationAdmissionOutcome::Started(started) = store.admit(&graph, &input, None).unwrap()
    else {
        panic!("expected a new conversation")
    };
    let conversation_ref = started.conversation.conversation_ref;
    store
        .record_host_exchange(
            Some(&conversation_ref),
            person,
            helper,
            "Please read it".into(),
            "I read it".into(),
        )
        .unwrap();
    assert!(store
        .conversation(&conversation_ref)
        .unwrap()
        .messages
        .iter()
        .any(|message| message.file_refs.contains(&file.file_ref)));
    drop(store);

    let store = ConversationAdmissionStore::open_with_timeline_rollout_deferred(
        &path,
        ROOT,
        ConversationTimelineRollout::NewConversations,
    )
    .unwrap();
    assert!(store
        .state
        .cold_conversation_refs
        .contains(&conversation_ref));
    assert!(store
        .conversation_header(&conversation_ref)
        .unwrap()
        .messages
        .is_empty());
    let retained = store.read_file(helper, &file.file_ref).unwrap();
    assert_eq!(retained.metadata, file);
    assert_eq!(hex::decode(retained.content_hex).unwrap(), b"Private brief");
    assert!(store
        .read_file("cwp://example.test/actors/stranger", &file.file_ref,)
        .is_err());
    assert!(store
        .state
        .cold_conversation_refs
        .contains(&conversation_ref));
}

#[test]
fn scoped_cold_read_rejects_corrupt_message_without_loading_other_history() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("corrupt-cold-message.sqlite");
    let mut store = ConversationAdmissionStore::open_with_timeline_rollout(
        &path,
        ROOT,
        ConversationTimelineRollout::NewConversations,
    )
    .unwrap();
    let good = store
        .record_host_exchange(
            None,
            PERSON,
            HELPER,
            "Good request".into(),
            "Good reply".into(),
        )
        .unwrap()
        .conversation;
    let bad = store
        .record_host_exchange(
            None,
            PERSON,
            HELPER,
            "Bad request".into(),
            "Bad reply".into(),
        )
        .unwrap()
        .conversation;
    drop(store);

    cwpdb_store::test_support::admission_history::corrupt_cold_message_text_type(
        &path,
        &bad.messages[0].message_ref,
    )
    .unwrap();

    let store = ConversationAdmissionStore::open_with_timeline_rollout_deferred(
        &path,
        ROOT,
        ConversationTimelineRollout::NewConversations,
    )
    .unwrap();
    assert!(store
        .state
        .cold_conversation_refs
        .contains(&good.conversation_ref));
    assert!(store
        .state
        .cold_conversation_refs
        .contains(&bad.conversation_ref));
    assert_eq!(
        store.full_conversation(&good.conversation_ref).unwrap(),
        Some(good)
    );
    let error = store.full_conversation(&bad.conversation_ref).unwrap_err();
    assert!(error.to_string().contains("conversation record"), "{error}");
}

#[test]
fn malformed_header_edge_cannot_authorize_or_publish_a_cold_header() {
    let directory = tempfile::tempdir().unwrap();
    for kind in ["participant", "root_conversation"] {
        let path = directory.path().join(format!("invalid-{kind}.sqlite"));
        let mut store = ConversationAdmissionStore::open_with_timeline_rollout(
            &path,
            ROOT,
            ConversationTimelineRollout::NewConversations,
        )
        .unwrap();
        store
            .record_host_exchange(None, PERSON, HELPER, "Request".into(), "Reply".into())
            .unwrap();
        drop(store);
        let relationship = if kind == "participant" {
            cwpdb_store::test_support::admission_history::ColdHeaderRelationship::Participant
        } else {
            cwpdb_store::test_support::admission_history::ColdHeaderRelationship::RootConversation
        };
        cwpdb_store::test_support::admission_history::corrupt_cold_header_position(
            &path,
            relationship,
        )
        .unwrap();
        assert!(
            ConversationAdmissionStore::open_with_timeline_rollout_deferred(
                &path,
                ROOT,
                ConversationTimelineRollout::NewConversations,
            )
            .is_err()
        );
    }
}

#[test]
fn runtime_root_edge_to_non_conversation_is_rejected_before_cold_publication() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("wrong-root-target.sqlite");
    let mut store = ConversationAdmissionStore::open_with_timeline_rollout(
        &path,
        ROOT,
        ConversationTimelineRollout::NewConversations,
    )
    .unwrap();
    store
        .record_host_exchange(None, PERSON, HELPER, "Request".into(), "Reply".into())
        .unwrap();
    drop(store);
    cwpdb_store::test_support::admission_history::corrupt_runtime_root_target_kind(&path).unwrap();
    let error = ConversationAdmissionStore::open_with_timeline_rollout_deferred(
        &path,
        ROOT,
        ConversationTimelineRollout::NewConversations,
    )
    .err()
    .expect("a runtime root edge with a wrong-kind target must fail closed");
    assert!(error.to_string().contains("root_conversation"), "{error}");
}

#[test]
fn unrelated_custom_conversation_relationships_do_not_block_cold_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("custom-conversation.sqlite");
    let mut admission = ConversationAdmissionStore::open_with_timeline_rollout(
        &path,
        ROOT,
        ConversationTimelineRollout::NewConversations,
    )
    .unwrap();
    let owned = admission
        .record_host_exchange(None, PERSON, HELPER, "Request".into(), "Reply".into())
        .unwrap()
        .conversation;
    drop(admission);
    cwpdb_store::test_support::admission_history::seed_unowned_custom_conversation(&path).unwrap();
    let reopened = ConversationAdmissionStore::open_with_timeline_rollout_deferred(
        &path,
        ROOT,
        ConversationTimelineRollout::NewConversations,
    )
    .unwrap();
    assert!(reopened
        .state
        .cold_conversation_refs
        .contains(&owned.conversation_ref));
    assert_eq!(
        reopened
            .conversation_header(&owned.conversation_ref)
            .unwrap()
            .conversation_ref,
        owned.conversation_ref
    );
}
