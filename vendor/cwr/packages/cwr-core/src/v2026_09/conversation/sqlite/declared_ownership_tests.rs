use super::*;

#[test]
fn retained_validation_reopens_with_unrelated_declared_activity() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("retained.sqlite");
    let runtime = "cwp://example.test/runtime";
    let conversation = "cwp://example.test/conversations/retained";
    let mut admission = ConversationAdmissionStore::open(&path, runtime).unwrap();
    let mut state = admission.state.clone();
    state.conversations = vec![serde_json::from_value(json!({
        "conversation_ref":conversation,"participant_actor_refs":[],
        "messages":[{"message_ref":"cwp://example.test/messages/retained",
        "author_actor_ref":"cwp://example.test/actors/person","addressed_actor_refs":[],"text":"Retained history"}]
    })).unwrap()].into();
    admission.replace_state(state).unwrap();
    drop(admission);
    let catalog = crate::entities::EntityCatalog::from_toml(r#"
type = "cwp://org.coworkerprotocol/2026-10-22/schemas/entity-definition"
ref = "cwp://example.test/definitions/custom-activity"
storage = "cwpdb"
record_kind = "activity"
record_schema = "cwp://org.coworkerprotocol/cwpdb/schemas/activity/2026-09"
value_schema = { type = "object", required = ["custom_label"], properties = { custom_label = { type = "string" } } }
"#).unwrap();
    let reference = "cwp://example.test/custom-activities/one";
    let value = json!({"custom_label":"Fictional custom activity"});
    let mut native = crate::cwpdb::NativeStore::open(&path).unwrap();
    catalog
        .create(
            &mut native,
            "cwp://example.test/definitions/custom-activity",
            reference,
            &value,
        )
        .unwrap();
    drop(native);
    let reopened = ConversationAdmissionStore::open(&path, runtime).unwrap();
    assert_eq!(reopened.state.conversations.len(), 1);
    assert_eq!(
        reopened.state.conversations[0].conversation_ref,
        conversation
    );
    assert_eq!(
        reopened.state.conversations[0].messages[0].text,
        "Retained history"
    );
    drop(reopened);
    let native = crate::cwpdb::NativeStore::open(&path).unwrap();
    assert_eq!(
        catalog
            .get(
                &native,
                "cwp://example.test/definitions/custom-activity",
                reference
            )
            .unwrap()
            .value
            .properties,
        value
    );
}

#[test]
fn retained_validation_rejects_owned_invalid_values_and_wrong_schemas() {
    for case in [
        RetainedCase::OwnedInvalidActivity,
        RetainedCase::WrongActivitySchema,
    ] {
        let facts = retained_case(case).unwrap();
        let error = validate_retained_conversation_entities(&facts)
            .unwrap_err()
            .to_string();
        assert!(error.contains("cwp://example.test/activity"), "{error}");
        if matches!(case, RetainedCase::WrongActivitySchema) {
            assert!(error.contains("unexpected schema"));
        }
        assert_eq!(
            facts
                .records
                .iter()
                .find(|n| n.kind == "activity")
                .unwrap()
                .value,
            json!({})
        );
    }
}
#[test]
fn retained_validation_ignores_unowned_kinds_but_checks_owned_orphan_claims() {
    validate_retained_conversation_entities(&retained_case(RetainedCase::UnownedKinds).unwrap())
        .unwrap();
    let error =
        validate_retained_conversation_entities(&retained_case(RetainedCase::OwnedOrphan).unwrap())
            .unwrap_err()
            .to_string();
    assert!(
        error.contains("cwp://example.test/timelines/owned-orphan"),
        "{error}"
    );
}
