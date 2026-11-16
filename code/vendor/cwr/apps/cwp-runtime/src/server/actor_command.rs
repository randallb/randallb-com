fn inspect_actor(
    state: &SharedState,
    writer: &SharedWriter,
    request_id: String,
    actor_ref: Option<&str>,
) -> CwrResult<()> {
    let runtime = lock_state(state)?.runtime.clone();
    let catalog = actor_ref.map_or_else(
        || runtime.catalog(),
        |actor_ref| runtime.catalog_as(actor_ref),
    )?;
    lock_state(state)?.host_actors.insert(
        upstream_owner(writer),
        catalog.acting_actor.actor_ref.clone(),
    );
    let workspace = cwr::v2026_09::host::inspect_host_workspace(&runtime.graph()?)?;
    write_event(
        writer,
        &RuntimeServiceEvent::ActorInspected {
            request_id,
            workspace,
            catalog,
        },
    )
}

fn reload_graph(
    state: &SharedState,
    writer: &SharedWriter,
    request_id: String,
    requesting_actor_ref: String,
) -> CwrResult<()> {
    let _publication = lock_broadcast(state)?;
    let runtime = lock_state(state)?.runtime.clone();
    let (_, catalog) =
        runtime.reload_graph_for_connected_actor_validated(&requesting_actor_ref, |_, _| Ok(()))?;
    broadcast_in_publication(state, RuntimeServiceEvent::CatalogUpdated { catalog })?;
    command_succeeded(writer, request_id, None, Vec::new(), None)
}
