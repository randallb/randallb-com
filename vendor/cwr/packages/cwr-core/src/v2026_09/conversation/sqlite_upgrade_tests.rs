use super::*;
use cwpdb_store::test_support::conversation_claims::{
    self as claims, ClaimRole, TimelineIdentityRefs,
};
const TIMELINE_CONNECTION_CLAIM: &str = "node:conversation_timeline_connection";
const TIMELINE_ITEM_CLAIM: &str = "node:conversation_timeline_item";
const TIMELINE_PUBLICATION_CLAIM: &str = "conversation_timeline_publication";

#[test]
fn opening_an_existing_timeline_claims_its_ownerless_identities() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("timeline-owner-upgrade.sqlite3");
    let root = "cwp://example.test/runtime";
    let mut store = ConversationAdmissionStore::open(&path, root).unwrap();
    let exchange = store
        .record_host_exchange(
            None,
            "cwp://example.test/actors/person",
            "cwp://example.test/actors/helper",
            "Question".into(),
            "Answer".into(),
        )
        .unwrap();
    let conversation_ref = exchange.conversation.conversation_ref;
    let root_connection_ref = format!("{conversation_ref}/timeline");
    store
        .create_retained_timeline(&conversation_ref, &root_connection_ref)
        .unwrap();
    let publication_ref = format!("{conversation_ref}/publications/request");
    let item = match store
        .publish_timeline_message(
            &conversation_ref,
            &publication_ref,
            &exchange.request_message_ref,
            ConversationTimelineDisposition::Root,
        )
        .unwrap()
    {
        ConversationTimelinePublishOutcome::Placed(item) => item,
        outcome => panic!("unexpected timeline outcome: {outcome:?}"),
    };
    drop(store);

    let refs = TimelineIdentityRefs {
        conversation: &conversation_ref,
        connection: &root_connection_ref,
        item: &item.item_ref,
        publication: &publication_ref,
    };
    claims::ownerless_timeline(&path, root, &refs).unwrap();
    assert!(claims::needs_repair(&path, root).unwrap());

    let _reopened = ConversationAdmissionStore::open(&path, root).unwrap();
    for (reference, kind, role) in [
        (
            conversation_ref.as_str(),
            "node:conversation",
            ClaimRole::Conversation,
        ),
        (
            root_connection_ref.as_str(),
            TIMELINE_CONNECTION_CLAIM,
            ClaimRole::TimelineConnection,
        ),
        (
            item.item_ref.as_str(),
            TIMELINE_ITEM_CLAIM,
            ClaimRole::TimelineItem,
        ),
        (
            publication_ref.as_str(),
            TIMELINE_PUBLICATION_CLAIM,
            ClaimRole::Publication,
        ),
    ] {
        assert_eq!(
            claims::observe(&path, root, &conversation_ref, role, reference).unwrap(),
            Some(crate::cwpdb::IdentityClaim {
                kind: kind.into(),
                owner_ref: Some(conversation_ref.clone())
            })
        );
    }
    assert!(!claims::needs_repair(&path, root).unwrap());
    // Foreign-owner rejection for each physical timeline claim lives beside
    // cwpdb's claim primitive; no native transaction crosses this test boundary.
}

#[test]
fn opening_rejects_a_retained_conversation_claimed_by_another_owner() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("wrong-owner.sqlite3");
    let root = "cwp://example.test/runtime";
    let mut store = ConversationAdmissionStore::open(&path, root).unwrap();
    let conversation_ref = store
        .record_host_exchange(
            None,
            "cwp://example.test/actors/person",
            "cwp://example.test/actors/helper",
            "Question".into(),
            "Answer".into(),
        )
        .unwrap()
        .conversation
        .conversation_ref;
    assert!(!claims::needs_repair(&path, root).unwrap());
    drop(store);

    claims::conflicting_conversation_owner(&path, root, &conversation_ref).unwrap();
    assert!(claims::needs_repair(&path, root).unwrap());

    let error = ConversationAdmissionStore::open(&path, root).err().unwrap();
    assert!(error.to_string().contains("already claimed"));
}

#[test]
fn opening_repairs_a_lone_ownerless_timeline_publication() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("ownerless-publication.sqlite3");
    let root = "cwp://example.test/runtime";
    let mut store = ConversationAdmissionStore::open(&path, root).unwrap();
    let conversation_ref = store
        .record_host_exchange(
            None,
            "cwp://example.test/actors/person",
            "cwp://example.test/actors/helper",
            "Question".into(),
            "Answer".into(),
        )
        .unwrap()
        .conversation
        .conversation_ref;
    store
        .create_retained_timeline(&conversation_ref, &format!("{conversation_ref}/timeline"))
        .unwrap();
    let publication_ref = format!("{conversation_ref}/publications/request");
    let message_ref = store.state.conversations[0].messages[0].message_ref.clone();
    store
        .publish_timeline_message(
            &conversation_ref,
            &publication_ref,
            &message_ref,
            ConversationTimelineDisposition::Root,
        )
        .unwrap();
    drop(store);

    claims::ownerless_publication(&path, root, &conversation_ref, &publication_ref).unwrap();
    assert!(claims::needs_repair(&path, root).unwrap());

    let _reopened = ConversationAdmissionStore::open(&path, root).unwrap();
    assert!(!claims::needs_repair(&path, root).unwrap());
    assert_eq!(
        claims::observe(
            &path,
            root,
            &conversation_ref,
            ClaimRole::Publication,
            &publication_ref
        )
        .unwrap(),
        Some(crate::cwpdb::IdentityClaim {
            kind: TIMELINE_PUBLICATION_CLAIM.into(),
            owner_ref: Some(conversation_ref),
        })
    );
}
