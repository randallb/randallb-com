//! Private history spans requests; each invocation keeps its own lifecycle.
use super::*;
use crate::v2026_09::invocation::ActionInvocationRecord;

const WORKING_STATE_METHOD: &str = "cwr.working_state";

mod gateway;
mod progress;
mod toolset;

// Bind only when execution starts: queued requests include public updates that
// arrived while they waited. Once bound, retries keep the exact same boundary.
pub(super) fn initialize(
    state: &mut ConversationRuntimeState,
    invocation_ref: &str,
    current_browse_permission: Option<bool>,
    current_graph: Option<&ResolvedCwrGraph>,
) -> CwrResult<()> {
    let execution = invocation::execution_conversation_in_state(state, invocation_ref)?;
    let mut adapter = snapshot(execution)?;
    // Legacy conversational requests and exceptional embedded contexts retain
    // their original boundaries. Current named actions carry an admitted route
    // and a canonical source snapshot, which can be bound at first execution.
    if !adapter.conversation_continuity
        && (adapter.action_name == RESPOND_TO_MESSAGE_ACTION_NAME
            || adapter.retained_route.is_none()
            || adapter.thread.get("context_snapshot").is_none())
    {
        return Ok(());
    }
    if adapter.thread["context_initialized"] == true
        || crate::adapter::working_context(&adapter.thread).is_some()
    {
        return Ok(());
    }
    context::restore(state, &adapter.thread)?;
    let execution_ref = execution.conversation_ref.clone();
    let record = state
        .invocation_ledger
        .invocation(invocation_ref)
        .ok_or("missing invocation context record")?;
    // Already-started legacy attempts retain their original admission boundary.
    // Root named actions may have no requesting conversation to observe.
    if record.attempt_count > 1 || record.conversation_ref.is_none() {
        return Ok(());
    }
    let conversation_ref = record.conversation_ref.as_deref().unwrap();
    let conversation = state
        .conversations
        .iter()
        .find(|c| c.conversation_ref == conversation_ref)
        .ok_or("missing invocation context source")?;
    adapter.thread = conversation_thread(conversation);
    adapter.thread["context_initialized"] = json!(true);
    let current_gateway = current_graph
        .filter(|_| adapter.conversation_continuity)
        .map(|graph| crate::v2026_09::local::gateway_operation_for_record(graph, record))
        .transpose()?;
    let mut progress_source_message_refs = prepare(
        state,
        &mut adapter,
        &record.target_actor_ref,
        record.actor_integration_ref.as_deref().unwrap_or(""),
        &execution_ref,
        current_browse_permission,
        current_gateway.as_ref(),
    )?;
    if selective_conversation_history(&adapter, current_browse_permission) {
        let source_message_ref = adapter.input["source_message_ref"]
            .as_str()
            .map(str::to_owned);
        if let Some(source) = progress::history_gap_source_message_ref(state, &adapter.thread)? {
            progress_source_message_refs.push(source);
        }
        context::select_recent(
            &mut adapter.thread,
            source_message_ref.as_deref(),
            &progress_source_message_refs,
        )?;
    }
    let retained = context::invocation_snapshot(&adapter)?;
    let execution = state
        .conversations
        .find_mut(|c| c.conversation_ref == execution_ref)
        .ok_or("missing private execution")?;
    for activity in &mut execution.activity {
        if let ConversationActivityEvent::TurnStarted {
            invocation: snapshot,
            ..
        } = &mut activity.event
        {
            *snapshot = Some(retained.clone());
        }
    }
    Ok(())
}

pub(super) fn check_child(
    state: &ConversationRuntimeState,
    adapter: &AdapterInvocation,
    actor_ref: &str,
    integration_ref: &str,
) -> CwrResult<()> {
    if !adapter.conversation_continuity {
        return Ok(());
    }
    let conversation_ref = adapter.thread["conversation_ref"]
        .as_str()
        .ok_or("missing child conversation")?;
    if latest(
        state,
        conversation_ref,
        actor_ref,
        integration_ref,
        &adapter.invocation_ref,
    )?
    .is_some_and(|record| record.attempt_active || !record.state.is_terminal())
    {
        return invalid(
            "This coworker is already working in this conversation; a nested call cannot continue that active private history.",
        );
    }
    Ok(())
}

fn prepare(
    state: &ConversationRuntimeState,
    invocation: &mut AdapterInvocation,
    actor_ref: &str,
    actor_integration_ref: &str,
    execution_ref: &str,
    current_browse_permission: Option<bool>,
    current_gateway: Option<&crate::adapter::AdapterHostOperation>,
) -> CwrResult<Vec<String>> {
    if !invocation.conversation_continuity {
        return Ok(Vec::new());
    }
    if invocation.action_name != RESPOND_TO_MESSAGE_ACTION_NAME {
        return invalid("private conversation continuity requires a conversational request");
    }
    let conversation_ref = invocation.thread["conversation_ref"]
        .as_str()
        .ok_or("private work requires a requesting conversation")?
        .to_owned();
    let prior = latest(
        state,
        &conversation_ref,
        actor_ref,
        actor_integration_ref,
        &invocation.invocation_ref,
    )?;
    let Some(mut prior) = prior else {
        annotate_tool_contracts(&mut invocation.host_operations, false)?;
        invocation.thread["working_conversation"] = json!({"ref": execution_ref});
        return Ok(Vec::new());
    };
    let mut progress_source_message_refs = Vec::new();
    if !prior.state.is_terminal() || prior.attempt_active {
        return invalid(
            "This coworker is already working in this conversation. Try again after that request finishes.",
        );
    }
    if matches!(
        prior.state,
        crate::v2026_09::invocation::ActionInvocationState::Stopped
            | crate::v2026_09::invocation::ActionInvocationState::Failed
            | crate::v2026_09::invocation::ActionInvocationState::Blocked
    ) {
        let predecessors = predecessors(
            state,
            &conversation_ref,
            actor_ref,
            actor_integration_ref,
            &invocation.invocation_ref,
        )?;
        let mut candidates = predecessors.into_iter();
        let _ = candidates.next();
        while unsuccessful(prior) && safely_stopped_before_live_execution(state, prior)? {
            progress_source_message_refs
                .push(progress::source_message_ref(state, &prior.invocation_ref)?);
            let Some(candidate) = candidates.next() else {
                annotate_tool_contracts(&mut invocation.host_operations, false)?;
                invocation.thread["working_conversation"] = json!({"ref":execution_ref});
                return Ok(progress_source_message_refs);
            };
            prior = candidate;
        }
        if unsuccessful(prior) {
            begin_after_history_gap(invocation, execution_ref, prior)?;
            return Ok(progress_source_message_refs);
        }
    }
    let execution = invocation::execution_conversation_in_state(state, &prior.invocation_ref)?;
    let previous = snapshot(execution)?;
    if !previous.conversation_continuity {
        // Legacy invocations did not retain a continuation contract. Expose the
        // boundary, rather than claiming to have restored their private work.
        invocation.thread["working_conversation"] = json!({"ref":execution_ref,
            "history_gap":"Earlier invocations predate private conversation continuity. Their public records remain available.",
            "previous_invocation_ref":prior.invocation_ref});
        return Ok(progress_source_message_refs);
    }
    let checkpoint = execution.activity.iter().rev().find_map(|activity| match &activity.event {
        ConversationActivityEvent::ProviderEvent { provider, method, data }
            if method == WORKING_STATE_METHOD => Some((activity, provider, data)),
        _ => None,
    }).ok_or("Private working history could not be restored after the previous request. Its completed effects are retained; start a new conversation to begin fresh.")?;
    let (activity, provider, data) = checkpoint;
    let supported_version = data["version"] == 1
        || (provider == "codex"
            && data["provider"] == "codex"
            && matches!(data["version"].as_u64(), Some(2 | 3)));
    if prior.state != crate::v2026_09::invocation::ActionInvocationState::Completed
        || data["invocation_ref"].as_str() != Some(prior.invocation_ref.as_str())
        || !supported_version
        || data["idle"] != true
    {
        return invalid(
            "The previous private execution did not settle safely; completed effects will not be replayed. Start a new conversation to begin fresh.",
        );
    }
    // These hashes cover different fields. An older snapshot can only prove
    // compatibility through the complete route fingerprint retained by both sides.
    let same_configuration = match (
        &previous.provider_fingerprint,
        &invocation.provider_fingerprint,
    ) {
        (Some(previous_fingerprint), Some(current_fingerprint)) => {
            previous_fingerprint == current_fingerprint
                || matches!(
                    (&previous.retained_route, &invocation.retained_route),
                    (Some(previous_route), Some(current_route))
                        if previous_route.adapter_ref() == crate::adapter::CODEX_ADAPTER_REF
                            && previous_fingerprint
                                == &previous_route.legacy_selective_provider_fingerprint()
                            && current_fingerprint == &current_route.provider_fingerprint()
                            && previous_route.provider_fingerprint()
                            == current_route.provider_fingerprint()
                )
        }
        _ => matches!((&previous.route_fingerprint, &invocation.route_fingerprint),
            (Some(previous), Some(current)) if previous == current),
    };
    if previous.integration_ref == invocation.integration_ref && !same_configuration {
        // A new admitted route after activation can begin fresh private history.
        // Verify both retained fingerprints; mismatched or legacy evidence still
        // rejects below, and retries keep their already-bound configuration.
        let verified_change = matches!((&previous.retained_route, &invocation.retained_route),
            (Some(old), Some(new))
                if old.adapter_ref() == new.adapter_ref()
                    && previous.provider_fingerprint.as_deref() == Some(old.provider_fingerprint().as_str())
                    && invocation.provider_fingerprint.as_deref() == Some(new.provider_fingerprint().as_str())
                    && previous.route_fingerprint.as_deref() == Some(old.fingerprint().as_str())
                    && invocation.route_fingerprint.as_deref() == Some(new.fingerprint().as_str()));
        if verified_change {
            annotate_tool_contracts(&mut invocation.host_operations, false)?;
            invocation.thread["working_conversation"] = json!({"ref":execution_ref,
                "history_gap":"The loop's execution configuration changed. Its public conversation and completed effects remain available; this request starts fresh private provider history.",
                "previous_invocation_ref":prior.invocation_ref});
            return Ok(progress_source_message_refs);
        }
    }
    if previous.integration_ref != invocation.integration_ref || !same_configuration {
        return invalid(
            "The coworker's execution configuration changed; its private provider state cannot be reused. Start a new conversation to begin fresh.",
        );
    }
    // Current provider sessions carry an explicit store marker. A private
    // session without one predates that boundary, so its provider ID cannot be
    // safely assigned to whichever store is now the default. Keep its public
    // record, but start fresh private provider history.
    if matches!(provider.as_str(), "codex" | "claude") && data.get("provider_store").is_none() {
        annotate_tool_contracts(&mut invocation.host_operations, false)?;
        invocation.thread["working_conversation"] = json!({"ref":execution_ref,
            "history_gap":"Workloop's provider sign-in directory changed. Earlier public records and completed effects remain available; this request starts fresh private provider history.",
            "previous_invocation_ref":prior.invocation_ref});
        return Ok(progress_source_message_refs);
    }
    // Gateway access is embedded in the registered send_request schema. A
    // settled provider session cannot pick up a newly approved source or a new
    // request contract. Start fresh without replaying any completed invocation.
    let gateway_changed = gateway::changed(&previous, invocation, current_gateway);
    if gateway_changed || toolset::changed(&previous, invocation) {
        annotate_tool_contracts(&mut invocation.host_operations, false)?;
        invocation.thread["working_conversation"] = json!({"ref":execution_ref,
            "history_gap":if gateway_changed {
                "The permitted gateway contracts changed. Its public conversation and completed effects remain available; this request starts fresh private provider history."
            } else {
                "The available host tools changed. Its public conversation and completed effects remain available; this request starts fresh private provider history."
            },
            "previous_invocation_ref":prior.invocation_ref});
        return Ok(progress_source_message_refs);
    }
    // Other catalog changes do not retarget names in a retained provider session.
    invocation.host_operations = previous.host_operations.clone();
    annotate_tool_contracts(&mut invocation.host_operations, true)?;
    if serde_json::to_vec(data)?.len() > 16 * 1024 {
        return invalid("private continuation control state exceeds 16 KiB");
    }
    let previous_context = crate::adapter::working_context(&previous.thread)
        .ok_or("missing previous working source")?;
    let previous_source = context::reference(previous_context)?;
    if data["context_snapshot"] != previous_source["context_snapshot"] {
        return invalid("private history does not cover its recorded conversation context");
    }
    // Validate canonical contents, not just a message count or a provider ID.
    context::restore(state, &previous_source)?;
    let start = previous_source["context_snapshot"]["message_count"]
        .as_u64()
        .and_then(|value| usize::try_from(value).ok())
        .ok_or("private context boundary is invalid")?;
    let selective_history = selective_conversation_history(invocation, current_browse_permission);
    let messages = invocation.thread["messages"]
        .as_array_mut()
        .ok_or("private work requires conversation messages")?;
    if start > messages.len() {
        return invalid("private context boundary exceeds the current conversation");
    }
    // A queued request can precede the previous turn's observation boundary.
    // Selective delivery keeps the archive intact so that request can be found.
    // A selective checkpoint names the archive boundary, but covers only its
    // listed indices. Turning selective history off must send the full archive
    // once, rather than treating every omitted message as already delivered.
    if !selective_history && previous_source["context_snapshot"]["version"] != 3 {
        *messages = messages.split_off(start);
        invocation.thread["context_start"] = json!(start);
    }
    invocation.thread["working_conversation"] = json!({
        "ref":previous_context["working_conversation"]["ref"],
        "previous_invocation_ref":prior.invocation_ref,
        "checkpoint_activity_ref":activity.activity_ref,
        "state":data,
    });
    Ok(progress_source_message_refs)
}

fn selective_conversation_history(
    invocation: &AdapterInvocation,
    current_browse_permission: Option<bool>,
) -> bool {
    invocation.conversation_continuity
        && invocation.action_name == RESPOND_TO_MESSAGE_ACTION_NAME
        && invocation
            .retained_route
            .as_ref()
            .is_some_and(|route| route.adapter_ref() == crate::adapter::CODEX_ADAPTER_REF)
        && invocation.adapter_params["isolated"] != true
        && invocation.adapter_params["selective_history"] != false
        && current_browse_permission.unwrap_or_else(|| {
            invocation
                .actor_integration_params
                .get("cwr_action_actor_refs")
                .is_none_or(|allowed| {
                    allowed.as_array().is_some_and(|refs| {
                        refs.iter().any(|reference| {
                            reference == crate::action_contract::INTRINSIC_CWR_ACTOR_REF
                        })
                    })
                })
        })
        // A resumed provider session keeps its original tools. Older sessions
        // without the current action contract cannot fetch omitted history.
        && (invocation.thread["working_conversation"]["state"].is_null()
            || retained_browse_tools(invocation))
}

fn retained_browse_tools(invocation: &AdapterInvocation) -> bool {
    let Some(discovery) = invocation
        .host_operations
        .iter()
        .find(|operation| operation.name == "cwr.list_actions")
    else {
        return false;
    };
    let Some(action) = invocation
        .host_operations
        .iter()
        .find(|operation| operation.name == crate::action_contract::INVOKE_ACTION_HOST_OPERATION)
    else {
        return false;
    };
    if discovery.input_schema["x-cwr-decoder-version"] == "cwr/1" {
        return false;
    }
    let mut schema = action.input_schema.clone();
    let Some(properties) = schema.as_object_mut() else {
        return false;
    };
    properties.retain(|key, _| !key.starts_with("x-cwr-"));
    schema == crate::action_contract::invoke_action_input_schema()
}

fn unsuccessful(record: &ActionInvocationRecord) -> bool {
    matches!(
        record.state,
        crate::v2026_09::invocation::ActionInvocationState::Stopped
            | crate::v2026_09::invocation::ActionInvocationState::Failed
            | crate::v2026_09::invocation::ActionInvocationState::Blocked
    )
}

fn safely_stopped_before_live_execution(
    state: &ConversationRuntimeState,
    record: &ActionInvocationRecord,
) -> CwrResult<bool> {
    let execution = invocation::execution_conversation_in_state(state, &record.invocation_ref)?;
    let integration_ref = snapshot(execution)
        .ok()
        .and_then(|invocation| invocation.integration_ref);
    if execution.activity.iter().any(|activity| {
        matches!(
            &activity.event,
            ConversationActivityEvent::HostCall { .. }
                | ConversationActivityEvent::HostActionStarted { .. }
                | ConversationActivityEvent::HostResult { .. }
                | ConversationActivityEvent::CommentaryProgress { .. }
        )
    }) {
        return Ok(false);
    }
    Ok(execution
        .activity
        .iter()
        .rev()
        .find_map(|activity| match &activity.event {
            ConversationActivityEvent::ProviderEvent { method, data, .. }
                if method == "cwr.adapter_checkpoint" =>
            {
                Some(
                    matches!(data["version"].as_u64(), Some(1..=5))
                        && data["invocation_ref"].as_str() == Some(record.invocation_ref.as_str())
                        && data["integration_ref"].as_str() == integration_ref.as_deref()
                        && data["live_started"] == false,
                )
            }
            _ => None,
        })
        .unwrap_or(false))
}

fn begin_after_history_gap(
    invocation: &mut AdapterInvocation,
    execution_ref: &str,
    prior: &ActionInvocationRecord,
) -> CwrResult<()> {
    let outcome = match prior.state {
        crate::v2026_09::invocation::ActionInvocationState::Stopped => "was stopped",
        crate::v2026_09::invocation::ActionInvocationState::Failed => "failed",
        crate::v2026_09::invocation::ActionInvocationState::Blocked => "was blocked",
        _ => unreachable!("history gaps require an unsuccessful terminal state"),
    };
    annotate_tool_contracts(&mut invocation.host_operations, false)?;
    invocation.thread["working_conversation"] = json!({"ref":execution_ref,
        "history_gap":format!("The previous request {outcome}. Its public records and completed effects remain available, but its private provider history was not resumed."),
        "previous_invocation_ref":prior.invocation_ref});
    Ok(())
}

fn latest<'a>(
    state: &'a ConversationRuntimeState,
    conversation_ref: &str,
    actor_ref: &str,
    actor_integration_ref: &str,
    current_ref: &str,
) -> CwrResult<Option<&'a ActionInvocationRecord>> {
    let predecessors = predecessors(
        state,
        conversation_ref,
        actor_ref,
        actor_integration_ref,
        current_ref,
    )?;
    Ok(predecessors
        .iter()
        .copied()
        .find(|record| !record.state.is_terminal() || record.attempt_active)
        .or_else(|| predecessors.first().copied()))
}

fn predecessors<'a>(
    state: &'a ConversationRuntimeState,
    conversation_ref: &str,
    actor_ref: &str,
    actor_integration_ref: &str,
    current_ref: &str,
) -> CwrResult<Vec<&'a ActionInvocationRecord>> {
    let Some(conversation) = state
        .conversations
        .iter()
        .find(|c| c.conversation_ref == conversation_ref)
    else {
        return Ok(Vec::new());
    };
    let message_positions = conversation
        .messages
        .iter()
        .enumerate()
        .map(|(index, message)| (message.message_ref.as_str(), index))
        .collect::<BTreeMap<_, _>>();
    let mut records = Vec::new();
    for record in state.invocation_ledger.invocations().filter(|record| {
        record.invocation_ref != current_ref
            && record.attempt_count > 0
            && record.conversation_ref.as_deref() == Some(conversation_ref)
            && record.target_actor_ref == actor_ref
            && record.actor_integration_ref.as_deref() == Some(actor_integration_ref)
            && record.action_name == RESPOND_TO_MESSAGE_ACTION_NAME
    }) {
        let execution = invocation::execution_conversation_in_state(state, &record.invocation_ref)?;
        let source = execution
            .activity
            .iter()
            .find_map(|activity| match &activity.event {
                ConversationActivityEvent::TurnStarted {
                    source_message_ref, ..
                } => Some(source_message_ref),
                _ => None,
            });
        if let Some(index) = source
            .and_then(|reference| message_positions.get(reference.as_str()))
            .copied()
        {
            records.push((index, record));
        }
    }
    records.sort_by_key(|(position, _)| std::cmp::Reverse(*position));
    Ok(records.into_iter().map(|(_, record)| record).collect())
}

pub(super) fn snapshot(execution: &RetainedConversation) -> CwrResult<AdapterInvocation> {
    let snapshot = execution
        .activity
        .iter()
        .find_map(|activity| match &activity.event {
            ConversationActivityEvent::TurnStarted {
                invocation: Some(snapshot),
                ..
            } => Some(snapshot),
            _ => None,
        })
        .ok_or("private execution has no retained initialization")?;
    Ok(serde_json::from_value(snapshot.clone())?)
}

pub(super) fn publish_progress(
    state: &mut ConversationRuntimeState,
    invocation_ref: &str,
    activity: &ConversationActivity,
    text: String,
    status: Option<ActionResponseStatus>,
) -> CwrResult<()> {
    progress::publish(state, invocation_ref, activity, text, status)
}

fn annotate_tool_contracts(
    tools: &mut [crate::adapter::AdapterHostOperation],
    legacy: bool,
) -> CwrResult<()> {
    for tool in tools {
        if tool.input_schema.get("x-cwr-model-name").is_some() {
            continue;
        }
        // These Codex helpers were introduced after the tool-name migration
        // and installed after native tool annotation. Their unannotated
        // contracts already use current names; adding legacy metadata on
        // resume changes the retained provider configuration.
        if legacy
            && matches!(
                tool.name.as_str(),
                "cwr.read_file" | "cwr.provider_user_input"
            )
        {
            continue;
        }
        let (name, version) = if legacy {
            (tool.name.clone(), "cwr/1")
        } else {
            (
                crate::tool_names::LogicalToolName::for_host_operation(&tool.name)?.qualified(),
                "cwp/1",
            )
        };
        let schema = tool
            .input_schema
            .as_object_mut()
            .ok_or("tool contract must be an object schema")?;
        schema.insert("x-cwr-model-name".into(), json!(name));
        schema.insert("x-cwr-decoder-version".into(), json!(version));
    }
    Ok(())
}

pub(super) fn streaming_destination(
    state: &ConversationRuntimeState,
    invocation_ref: &str,
) -> CwrResult<(String, Option<String>, String)> {
    progress::public_destination(state, invocation_ref)
}
