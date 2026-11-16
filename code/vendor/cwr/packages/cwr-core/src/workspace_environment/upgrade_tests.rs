use super::*;

#[test]
fn existing_workspace_environment_survives_graph_upgrade() {
    with_state(|workspace, store, _| {
        let saved = save(
            workspace,
            store,
            "TIMEZONE",
            EnvironmentEntryKind::Variable,
            "America/New_York",
            None,
            None,
        )
        .unwrap();
        let (legacy, _) = open(workspace).unwrap();
        legacy.fixture_legacy_identity_layout().unwrap();
        drop(legacy);
        let reopened = list(workspace, store).unwrap();
        assert_eq!(reopened.entries, saved.entries);
        save(
            workspace,
            store,
            "LANGUAGE",
            EnvironmentEntryKind::Variable,
            "en",
            None,
            None,
        )
        .unwrap();
        assert_eq!(list(workspace, store).unwrap().entries.len(), 2);
    });
}
