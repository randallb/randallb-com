#[test]
fn limited_actor_integration_is_rejected_before_admission() {
    let mut graph = fixture_graph_with_reviewer();
    let integration = graph
        .actor_integrations
        .iter_mut()
        .find(|integration| integration.actor_ref == crate::v2026_09::test_fixture::actor_ref("helper"))
        .expect("helper actor integration");
    integration.missing_required_authority = vec!["workspace.read".into()];

    let error = select_actor_integration(
        &graph,
        crate::v2026_09::test_fixture::actor_ref("helper"),
        crate::v2026_09::test_fixture::binding_ref("helper"),
    )
    .expect_err("limited route must not be selected");

    assert!(error
        .to_string()
        .contains("missing required authority: workspace.read"));
}

#[test]
fn runtime_root_must_be_absolute() {
    let error = validate_runtime_root("~@").expect_err("mount root must fail");
    assert!(error.to_string().contains("absolute unfragmented"));
    validate_runtime_root("cwp://com.example.runtime").expect("absolute runtime root");
}
