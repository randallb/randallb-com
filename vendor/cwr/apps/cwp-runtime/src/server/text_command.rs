fn submit_text_command(
    state: &SharedState,
    writer: &SharedWriter,
    request_id: String,
    integration_ref: String,
    author_actor_ref: String,
    conversation_ref: Option<String>,
    text: String,
) -> CwrResult<()> {
    let runtime = lock_state(state)?.runtime.clone();
    let module = runtime.bundled_conversation_module_as(&integration_ref, &author_actor_ref)?;
    let admission = module.publish_text_in_context(
        &request_id,
        conversation_ref.as_deref(),
        &text,
        Some(&upstream_owner(writer)),
    )?;
    accept_admission(state, writer, request_id, admission, None)
}

fn replay_text_command(
    state: &SharedState,
    writer: &SharedWriter,
    request_id: String,
    integration_ref: String,
    author_actor_ref: String,
    conversation_ref: String,
    text: String,
) -> CwrResult<()> {
    let runtime = lock_state(state)?.runtime.clone();
    let module = runtime.bundled_conversation_module_as(&integration_ref, &author_actor_ref)?;
    let admission = module.replay_text_in_context(
        &request_id,
        &conversation_ref,
        &text,
        Some(&upstream_owner(writer)),
    )?;
    accept_admission(state, writer, request_id, admission, None)
}
