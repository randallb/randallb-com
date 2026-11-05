use super::*;

include!("tests/fixtures.rs");

#[test]
fn prepare_rejects_a_nonparticipant_author_on_a_continued_conversation() {
    let temporary = tempfile::tempdir().expect("temporary state");
    let mut store = ConversationAdmissionStore::open(
        temporary.path().join("conversation.json"),
        "cwp://com.example.runtime",
    )
    .expect("open store");
    let graph = fixture_graph_with_reviewer();
    let first = store
        .admit(
            &graph,
            &ConversationIngress {
                owner_context_ref: None,
                file_refs: Vec::new(),
                integration_ref: "cwp://com.example.fixture/integrations/local".into(),
                provider_event_ref: "event-first".into(),
                author_actor_ref: crate::v2026_09::test_fixture::actor_ref("person").into(),
                conversation: ConversationIngressConversation::Create,
                addressing: ConversationIngressAddressing::Unaddressed,
                text: "Starting context".into(),
            },
            None,
        )
        .expect("admit first message");
    let ConversationAdmissionOutcome::Started(first) = first else {
        panic!("first admission must start a conversation")
    };
    assert_eq!(first.record.target_actor_ref, None);
    assert!(first.invocation.is_none());
    let error = store
        .prepare(
            &graph,
            &ConversationIngress {
                owner_context_ref: None,
                file_refs: Vec::new(),
                integration_ref: "cwp://com.example.fixture/integrations/local".into(),
                provider_event_ref: "event-second".into(),
                author_actor_ref: crate::v2026_09::test_fixture::actor_ref("helper").into(),
                conversation: ConversationIngressConversation::Continue {
                    conversation_ref: first.record.conversation_ref.clone(),
                },
                addressing: ConversationIngressAddressing::Addressed {
                    actor_refs: vec!["cwp://com.example.fixture/actors/reviewer".into()],
                },
                text: "@reviewer take a look".into(),
            },
            Some("cwp://com.example.fixture/actor_integrations/reviewer_local_execution"),
        )
        .expect_err("continued ingress requires existing membership");
    assert!(error.to_string().contains("not a conversation participant"));
    assert_eq!(
        store
            .conversation(&first.record.conversation_ref)
            .unwrap()
            .participant_actor_refs,
        vec![crate::v2026_09::test_fixture::actor_ref("person")]
    );
}

#[test]
fn prepare_adds_only_addressed_coworkers_to_a_continued_conversation() {
    let temporary = tempfile::tempdir().expect("temporary state");
    let mut store = ConversationAdmissionStore::open(
        temporary.path().join("conversation.json"),
        "cwp://com.example.runtime",
    )
    .expect("open store");
    let graph = fixture_graph_with_reviewer();
    let ConversationAdmissionOutcome::Started(first) = store
        .admit(
            &graph,
            &ingress(
                "event-default-first",
                crate::v2026_09::test_fixture::actor_ref("person"),
                None,
                ConversationIngressAddressing::Unaddressed,
                "Starting context",
            ),
            None,
        )
        .expect("admit first message")
    else {
        panic!("first admission must start a conversation")
    };

    let ConversationPreparationOutcome::New(prepared) = store
        .prepare(
            &graph,
            &ingress(
                "event-default-second",
                crate::v2026_09::test_fixture::actor_ref("person"),
                Some(&first.record.conversation_ref),
                ConversationIngressAddressing::Addressed {
                    actor_refs: vec!["cwp://com.example.fixture/actors/reviewer".into()],
                },
                "@reviewer take a look",
            ),
            Some("cwp://com.example.fixture/actor_integrations/reviewer_local_execution"),
        )
        .expect("prepare continued message")
    else {
        panic!("continued admission must be new")
    };

    assert_eq!(
        prepared.conversation.participant_actor_refs,
        vec![
            crate::v2026_09::test_fixture::actor_ref("person"),
            "cwp://com.example.fixture/actors/reviewer"
        ]
    );
}

#[test]
fn unaddressed_message_inherits_same_authors_last_target() {
    let temporary = tempfile::tempdir().expect("temporary state");
    let mut store = ConversationAdmissionStore::open(
        temporary.path().join("conversation.json"),
        "cwp://com.example.runtime",
    )
    .expect("open store");
    let graph = fixture_graph_with_reviewer();
    let first = store
        .admit(
            &graph,
            &ingress(
                "event-first",
                crate::v2026_09::test_fixture::actor_ref("person"),
                None,
                ConversationIngressAddressing::Addressed {
                    actor_refs: vec![crate::v2026_09::test_fixture::actor_ref("helper").into()],
                },
                "@helper first",
            ),
            Some(crate::v2026_09::test_fixture::binding_ref("helper")),
        )
        .expect("admit addressed message");
    let ConversationAdmissionOutcome::Started(first) = first else {
        panic!("first admission must start a conversation")
    };

    let continued = store
        .admit(
            &graph,
            &ingress(
                "event-second",
                crate::v2026_09::test_fixture::actor_ref("person"),
                Some(&first.record.conversation_ref),
                ConversationIngressAddressing::Unaddressed,
                "continue",
            ),
            None,
        )
        .expect("inherit addressed route");
    let ConversationAdmissionOutcome::Started(continued) = continued else {
        panic!("continued admission must start a turn")
    };

    assert_eq!(
        continued.record.target_actor_ref.as_deref(),
        Some(crate::v2026_09::test_fixture::actor_ref("helper"))
    );
    assert_eq!(
        continued.record.actor_integration_ref.as_deref(),
        Some(crate::v2026_09::test_fixture::binding_ref("helper"))
    );
    assert!(continued.invocation.is_some());
    assert_eq!(
        continued
            .conversation
            .messages
            .last()
            .unwrap()
            .addressed_actor_refs,
        vec![crate::v2026_09::test_fixture::actor_ref("helper")]
    );
    assert_eq!(
        continued
            .conversation
            .messages
            .last()
            .unwrap()
            .addressing_mode,
        Some(ConversationAddressingMode::Implicit)
    );
}

#[test]
fn unaddressed_message_ignores_other_authors_target() {
    let temporary = tempfile::tempdir().expect("temporary state");
    let mut store = ConversationAdmissionStore::open(
        temporary.path().join("conversation.json"),
        "cwp://com.example.runtime",
    )
    .expect("open store");
    let graph = fixture_graph_with_reviewer();
    let first = store
        .admit(
            &graph,
            &ingress(
                "event-first",
                crate::v2026_09::test_fixture::actor_ref("person"),
                None,
                ConversationIngressAddressing::Addressed {
                    actor_refs: vec![crate::v2026_09::test_fixture::actor_ref("helper").into()],
                },
                "@helper first",
            ),
            Some(crate::v2026_09::test_fixture::binding_ref("helper")),
        )
        .expect("admit first message");
    let ConversationAdmissionOutcome::Started(first) = first else {
        panic!("first admission must start a conversation")
    };
    store
        .admit(
            &graph,
            &ingress(
                "event-other-author",
                crate::v2026_09::test_fixture::actor_ref("helper"),
                Some(&first.record.conversation_ref),
                ConversationIngressAddressing::Addressed {
                    actor_refs: vec!["cwp://com.example.fixture/actors/reviewer".into()],
                },
                "@reviewer inspect",
            ),
            Some("cwp://com.example.fixture/actor_integrations/reviewer_local_execution"),
        )
        .expect("admit other author's message");

    let continued = store
        .admit(
            &graph,
            &ingress(
                "event-continued",
                crate::v2026_09::test_fixture::actor_ref("person"),
                Some(&first.record.conversation_ref),
                ConversationIngressAddressing::Unaddressed,
                "continue",
            ),
            None,
        )
        .expect("inherit original author's route");
    let ConversationAdmissionOutcome::Started(continued) = continued else {
        panic!("continued admission must start a turn")
    };
    assert_eq!(
        continued.record.target_actor_ref.as_deref(),
        Some(crate::v2026_09::test_fixture::actor_ref("helper"))
    );
}

#[test]
fn explicit_target_replaces_inherited_target() {
    let temporary = tempfile::tempdir().expect("temporary state");
    let mut store = ConversationAdmissionStore::open(
        temporary.path().join("conversation.json"),
        "cwp://com.example.runtime",
    )
    .expect("open store");
    let graph = fixture_graph_with_reviewer();
    let first = store
        .admit(
            &graph,
            &ingress(
                "event-first",
                crate::v2026_09::test_fixture::actor_ref("person"),
                None,
                ConversationIngressAddressing::Addressed {
                    actor_refs: vec![crate::v2026_09::test_fixture::actor_ref("helper").into()],
                },
                "@helper first",
            ),
            Some(crate::v2026_09::test_fixture::binding_ref("helper")),
        )
        .expect("admit first message");
    let ConversationAdmissionOutcome::Started(first) = first else {
        panic!("first admission must start a conversation")
    };
    store
        .admit(
            &graph,
            &ingress(
                "event-retarget",
                crate::v2026_09::test_fixture::actor_ref("person"),
                Some(&first.record.conversation_ref),
                ConversationIngressAddressing::Addressed {
                    actor_refs: vec!["cwp://com.example.fixture/actors/reviewer".into()],
                },
                "@reviewer next",
            ),
            Some("cwp://com.example.fixture/actor_integrations/reviewer_local_execution"),
        )
        .expect("admit explicit retarget");

    let continued = store
        .admit(
            &graph,
            &ingress(
                "event-continued",
                crate::v2026_09::test_fixture::actor_ref("person"),
                Some(&first.record.conversation_ref),
                ConversationIngressAddressing::Unaddressed,
                "continue",
            ),
            None,
        )
        .expect("inherit replacement route");
    let ConversationAdmissionOutcome::Started(continued) = continued else {
        panic!("continued admission must start a turn")
    };
    assert_eq!(
        continued.record.target_actor_ref.as_deref(),
        Some("cwp://com.example.fixture/actors/reviewer")
    );
}

#[test]
fn inherited_target_requires_its_retained_integration_route() {
    let temporary = tempfile::tempdir().expect("temporary state");
    let mut store = ConversationAdmissionStore::open(
        temporary.path().join("conversation.json"),
        "cwp://com.example.runtime",
    )
    .expect("open store");
    let graph = fixture_graph_with_reviewer();
    let first = store
        .admit(
            &graph,
            &ingress(
                "event-first",
                crate::v2026_09::test_fixture::actor_ref("person"),
                None,
                ConversationIngressAddressing::Addressed {
                    actor_refs: vec![crate::v2026_09::test_fixture::actor_ref("helper").into()],
                },
                "@helper first",
            ),
            Some(crate::v2026_09::test_fixture::binding_ref("helper")),
        )
        .expect("admit first message");
    let ConversationAdmissionOutcome::Started(first) = first else {
        panic!("first admission must start a conversation")
    };
    let mut changed_graph = graph;
    changed_graph.actor_integrations.retain(|integration| {
        integration.actor_integration_ref != crate::v2026_09::test_fixture::binding_ref("helper")
    });

    let error = store
        .admit(
            &changed_graph,
            &ingress(
                "event-continued",
                crate::v2026_09::test_fixture::actor_ref("person"),
                Some(&first.record.conversation_ref),
                ConversationIngressAddressing::Unaddressed,
                "continue",
            ),
            None,
        )
        .expect_err("missing retained route must fail");
    assert!(error.to_string().contains("is not in"));
}

#[test]
fn legacy_admission_without_execution_conversation_is_skipped() {
    let temporary = tempfile::tempdir().expect("temporary state");
    let mut store = ConversationAdmissionStore::open(
        temporary.path().join("conversation.json"),
        "cwp://com.example.runtime",
    )
    .expect("open store");
    let record = ConversationAdmissionRecord {
        integration_ref: "cwp://com.example.fixture/integrations/slack".into(),
        provider_event_ref: "event-legacy".into(),
        ingress_fingerprint: None,
        source_input_fingerprint: None,
        conversation_ref: "cwp://com.example.runtime/conversations/legacy".into(),
        source_message_ref: "cwp://com.example.runtime/messages/legacy".into(),
        execution_conversation_ref: None,
        target_actor_ref: Some("cwp://com.example.fixture/actors/coworker".into()),
        actor_integration_ref: Some("cwp://com.example.fixture/actor_integrations/coworker".into()),
        adapter_ref: Some("cwp://org.coworkerprotocol/2026-09/adapters/codex".into()),
        invocation_ref: Some("cwp://com.example.runtime/invocations/legacy".into()),
        state: ConversationAdmissionState::Admitted,
        reply_recipient_visibility: ConversationRecipientVisibility::Implicit,
        reply_message_ref: None,
    };
    store.state.admissions.push(record.clone());
    assert!(store
        .resume_admitted(&record)
        .expect("inspect legacy admission")
        .is_none());
}

include!("tests/recipient_visibility.rs");
include!("tests/addressing.rs");
include!("tests/action_target.rs");
include!("tests/titles.rs");
include!("tests/revisions.rs");
include!("tests/validation.rs");
include!("tests/context_snapshots.rs");

include!("tests/working.rs");
include!("tests/working_provider_switch.rs");
include!("tests/working_stop_continuity.rs");
include!("tests/retained_timeline_persistence.rs");
include!("tests/retained_timeline_reads.rs");
include!("tests/retained_timeline_edge_reads.rs");
include!("tests/retained_timeline_order.rs");
include!("tests/timeline_migration.rs");
include!("tests/timeline_persistence.rs");
include!("tests/timeline_cursor_fencing.rs");

include!("tests/transfer.rs");
include!("tests/timeline_rollout_transfer.rs");
include!("tests/timeline_transfer_inspector.rs");
include!("tests/transfer_visualizations.rs");
include!("tests/fork_transfer.rs");
include!("tests/fork_storage_compatibility.rs");
include!("tests/fork_boundary_work.rs");
include!("tests/transfer_validation.rs");

include!("tests/publication_snapshot.rs");

include!("tests/streaming.rs");
include!("tests/presentation.rs");

include!("tests/inspector.rs");

include!("tests/canonical_fork.rs");
include!("tests/admission_retry_identity.rs");

include!("tests/file_transfer_custody.rs");

include!("tests/ownership.rs");
include!("tests/ownership_admission.rs");

#[path = "tests/dogfood_fixture.rs"]
mod dogfood_fixture;

include!("tests/message_boundary_lookup.rs");

include!("tests/declared_backend.rs");
