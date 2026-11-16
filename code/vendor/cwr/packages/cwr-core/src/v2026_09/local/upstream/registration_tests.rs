#[test]
fn disconnect_allows_another_connection_to_reuse_the_private_handle() {
    let (_directory, runtime, actor, _session, _receiver) = fixture();
    let admission = runtime.admit_text(None, "hello").unwrap();
    runtime.disconnect_upstream_provider("connection").unwrap();
    let mut replacement = actor.clone();
    replacement.actor_ref = "cwp://example.test/tuis/two/actors/tui".into();
    let session = runtime
        .register_upstream_provider(
            "replacement",
            vec![replacement.clone()],
            Arc::new(|_| Ok(())),
            std::sync::mpsc::sync_channel(1).0,
        )
        .unwrap();
    runtime
        .enable_upstream_coworker(
            "replacement",
            &session,
            &replacement.actor_ref,
            crate::v2026_09::test_fixture::actor_ref("person"),
            true,
        )
        .unwrap();
    assert_eq!(
        runtime
            .upstream_catalog_as(
                crate::v2026_09::test_fixture::actor_ref("person"),
                Some("replacement")
            )
            .unwrap()
            .actor_by_handle("@tui")
            .unwrap()
            .actor_ref,
        replacement.actor_ref
    );
    assert!(!admission
        .conversation
        .participant_actor_refs
        .contains(&actor.actor_ref));
}

#[test]
fn retained_actor_refs_reject_other_identities_before_mutation() {
    for restart in [false, true] {
        let (directory, runtime, actor, _session, _receiver) = fixture();
        let person = crate::v2026_09::test_fixture::actor_ref("person");
        let admission = runtime
            .submit_text_in_upstream_scope(
                "cwp://example.test/ingress",
                "identity",
                person,
                None,
                "@tui new",
                Some("connection"),
            )
            .unwrap();
        runtime.disconnect_upstream_provider("connection").unwrap();
        let runtime = if restart {
            drop(runtime);
            LocalRuntime::open_with_state_path(
                super::tests::fixture_workspace(),
                directory.path().join("runtime.sqlite"),
                super::tests::fixture_host_identity(),
            )
            .unwrap()
        } else {
            runtime
        };
        let before = runtime
            .lock()
            .unwrap()
            .admission
            .retain_local_identities(&[])
            .unwrap();
        for change_presentation in [false, true] {
            let mut impostor = actor.clone();
            impostor.coworker_ref = "cwp://example.test/coworkers/impostor".into();
            if change_presentation {
                impostor.name = "Impostor".into();
                impostor.handle = "@impostor".into();
            }
            assert!(runtime
                .register_upstream_provider(
                    "impostor",
                    vec![impostor],
                    Arc::new(|_| Ok(())),
                    std::sync::mpsc::sync_channel(1).0,
                )
                .is_err());
            assert!(runtime
                .lock()
                .unwrap()
                .upstream_providers
                .values()
                .all(|provider| provider.owner != "impostor"));
            assert_eq!(
                runtime
                    .lock()
                    .unwrap()
                    .admission
                    .retain_local_identities(&[])
                    .unwrap(),
                before
            );
            let snapshot = runtime
                .publication_snapshot_until(
                    person,
                    Some("impostor"),
                    std::time::Instant::now() + Duration::from_secs(5),
                    || false,
                    |_| {},
                )
                .unwrap();
            let display = snapshot
                .related_actor(admission.invocation_ref().unwrap(), &actor.actor_ref)
                .unwrap()
                .unwrap();
            assert_eq!(display.name.as_deref(), Some("TUI"));
            assert_eq!(display.handle.as_deref(), Some("@tui"));
            assert!(runtime
                .upstream_catalog_as(person, Some("impostor"))
                .unwrap()
                .actor_by_ref(&actor.actor_ref)
                .is_none());
        }
        // A rejected declaration must not poison subsequent valid reconnection.
        let session = runtime
            .register_upstream_provider(
                "reconnected",
                vec![actor.clone()],
                Arc::new(|_| Ok(())),
                std::sync::mpsc::sync_channel(1).0,
            )
            .unwrap();
        assert!(runtime
            .upstream_catalog_as(person, Some("reconnected"))
            .unwrap()
            .actor_by_ref(&actor.actor_ref)
            .is_none());
        runtime
            .enable_upstream_coworker("reconnected", &session, &actor.actor_ref, person, true)
            .unwrap();
        assert_eq!(
            runtime
                .upstream_catalog_as(person, Some("reconnected"))
                .unwrap()
                .actor_by_ref(&actor.actor_ref)
                .unwrap()
                .name,
            "TUI"
        );
    }
}
