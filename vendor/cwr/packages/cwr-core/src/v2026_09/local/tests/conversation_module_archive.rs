use super::*;

#[test]
fn bundled_module_archives_with_current_upstream_owner_authority() {
    let (_temporary, runtime) = participation_runtime();
    let author = "cwp://example.test/software/authors/pack-manager";
    let owner = "fictional-provider-connection";
    register_module_software_author(&runtime, owner, author);
    let service = runtime
        .bundled_conversation_module_as("cwp://example.test/ingress", author)
        .unwrap();
    let admission = service
        .publish_message_in_context(
            "scoped-archive-request",
            None,
            "Fictional scoped note",
            Vec::new(),
            Some(owner),
            None,
        )
        .unwrap();
    let reference = admission.conversation_ref();
    assert!(service.set_archived(reference, true).is_err());
    assert!(service
        .set_archived_in_context(reference, true, Some("another-connection"))
        .is_err());
    let unknown = runtime
        .bundled_conversation_module_as(
            "cwp://example.test/ingress",
            "cwp://example.test/software/unregistered",
        )
        .unwrap();
    assert!(unknown
        .set_archived_in_context(reference, true, Some(owner))
        .is_err());
    let nonparticipant_actor = crate::v2026_09::test_fixture::actor_ref("helper");
    runtime
        .upstream_catalog_as(nonparticipant_actor, Some(owner))
        .unwrap();
    assert!(!runtime
        .lock()
        .unwrap()
        .admission
        .conversation(reference)
        .unwrap()
        .participant_actor_refs
        .iter()
        .any(|actor| actor == nonparticipant_actor));
    let nonparticipant = runtime
        .bundled_conversation_module_as(
            "cwp://example.test/ingress",
            crate::v2026_09::test_fixture::actor_ref("helper"),
        )
        .unwrap();
    assert!(nonparticipant
        .set_archived_in_context(reference, true, Some(owner))
        .is_err());
    service
        .set_archived_in_context(reference, true, Some(owner))
        .unwrap();
    assert!(
        runtime
            .lock()
            .unwrap()
            .admission
            .conversation(reference)
            .unwrap()
            .archived
    );
    service
        .set_archived_in_context(reference, false, Some(owner))
        .unwrap();
    assert!(
        !runtime
            .lock()
            .unwrap()
            .admission
            .conversation(reference)
            .unwrap()
            .archived
    );

    // Revocation after initial preparation must be checked again before stops.
    let (_, mutation) = runtime
        .prepare_conversation_archive_in_upstream_scope_as(
            Some(reference),
            author,
            true,
            Some(owner),
        )
        .unwrap();
    let reservation = runtime.reserve_conversation_archive(reference).unwrap();
    let session = runtime
        .lock()
        .unwrap()
        .upstream
        .visible(Some(owner))
        .unwrap()[0]
        .provider_session_ref
        .clone();
    runtime
        .enable_upstream_coworker(
            owner,
            &session,
            author,
            crate::v2026_09::test_fixture::actor_ref("person"),
            false,
        )
        .unwrap();
    assert!(runtime
        .prepare_archive_stops_reserved_in_upstream_scope(&mutation, None, reservation, Some(owner))
        .is_err());
    assert!(service
        .set_archived_in_context(reference, true, Some(owner))
        .is_err());
    assert!(service
        .set_archived_in_context(reference, false, Some(owner))
        .is_err());
    assert!(
        !runtime
            .lock()
            .unwrap()
            .admission
            .conversation(reference)
            .unwrap()
            .archived
    );
}
