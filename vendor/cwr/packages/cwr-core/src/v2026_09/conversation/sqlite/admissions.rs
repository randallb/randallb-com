#[cfg(test)]
mod admission_storage_tests {
    use super::*;

    const FINGERPRINT: &str = "sha256:source-input";

    fn fixture_ingress() -> ConversationIngress {
        ConversationIngress {
            owner_context_ref: None,
            file_refs: Vec::new(),
            integration_ref: "cwp://com.example.fixture/integrations/local".into(),
            provider_event_ref: "event-source-storage".into(),
            author_actor_ref: crate::v2026_09::test_fixture::actor_ref("person").into(),
            conversation: ConversationIngressConversation::Create,
            addressing: ConversationIngressAddressing::Unaddressed,
            text: "Store the source fingerprint".into(),
        }
    }

    #[test]
    fn source_fingerprint_stays_outside_the_rollback_record_payload() {
        let temporary = tempfile::tempdir().expect("temporary state");
        let state_path = temporary.path().join("conversation.sqlite");
        let mut store = ConversationAdmissionStore::open(&state_path, "cwp://com.example.runtime")
            .expect("open store");
        store
            .admit_host_local(
                crate::v2026_09::test_fixture::graph(),
                &fixture_ingress(),
                None,
                &[],
                None,
                Some(FINGERPRINT),
            )
            .expect("admit source-aware message");
        cwpdb_store::test_support::admission_history::assert_source_fingerprint_layout(&state_path, FINGERPRINT).unwrap();
        drop(store);

        cwpdb_store::test_support::admission_history::seed_embedded_source_fingerprint(&state_path, FINGERPRINT).unwrap();

        let reopened = ConversationAdmissionStore::open(&state_path, "cwp://com.example.runtime")
            .expect("reopen and migrate store");
        let retained = reopened
            .admission(
                "cwp://com.example.fixture/integrations/local",
                "event-source-storage",
            )
            .expect("retained admission");
        assert_eq!(
            retained.source_input_fingerprint.as_deref(),
            Some(FINGERPRINT)
        );
        cwpdb_store::test_support::admission_history::assert_source_fingerprint_layout(&state_path, FINGERPRINT).unwrap();
    }
}
