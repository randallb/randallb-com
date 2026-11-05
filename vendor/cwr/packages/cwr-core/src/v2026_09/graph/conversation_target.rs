fn default_conversation_target(
    resource: &LoadedResource,
    coworker: &CoworkerFile,
    graph: &ResolvedGraph,
) -> CwrResult<Option<String>> {
    let mut defaults = coworker
        .imports
        .iter()
        .filter(|import| import.default_conversation_target);
    let Some(import) = defaults.next() else {
        return Ok(None);
    };
    if defaults.next().is_some() {
        return invalid(format!(
            "{}: at most one import may set default_conversation_target = true",
            resource.source.display()
        ));
    }
    let target = resource_of_type(
        &resource.source,
        import
            .locator
            .reference
            .as_deref()
            .expect("resolved default conversation target"),
        "coworker for default_conversation_target",
        graph,
        resource.scope_id,
        |kind| matches!(kind, ResourceKind::Coworker(_)),
    )?;
    if target.effective_ref == resource.effective_ref {
        return invalid(format!(
            "{}: default_conversation_target must select another coworker",
            resource.source.display()
        ));
    }
    Ok(Some(target.effective_ref.clone()))
}
