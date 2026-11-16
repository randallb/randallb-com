/// Renders a provider-neutral conversational result for the runtime-owned CWR
/// coworker. Hosts may apply richer native formatting when projecting it.
pub fn render_intrinsic_cwr_response(response: &IntrinsicCwrResponse) -> String {
    match response {
        IntrinsicCwrResponse::Coworkers { coworkers } => {
            let mut lines = vec!["Available coworkers:".to_owned()];
            lines.extend(coworkers.iter().map(render_intrinsic_cwr_coworker));
            lines.join("\n")
        }
        IntrinsicCwrResponse::Actions { actions, .. } => {
            format!("Available actions:\n{}", actions.join("\n"))
        }
        IntrinsicCwrResponse::Help { actions } => format!(
            "CWR commands:\n{}\n\nYou can also use @cwr with list coworkers, list actions, reload, debug invocation <ref-or-id>, stop [ref-or-id], or help. Debug invocation without a ref inspects the latest retained failure in the conversation.",
            actions
                .iter()
                .filter(|action| action.starts_with("@cwr--"))
                .cloned()
                .collect::<Vec<_>>()
                .join("\n")
        ),
        IntrinsicCwrResponse::Unknown { command, actions } => format!(
            "CWR does not recognize {command:?}.\nAvailable CWR commands:\n{}",
            actions
                .iter()
                .filter(|action| action.starts_with("@cwr--"))
                .cloned()
                .collect::<Vec<_>>()
                .join("\n")
        ),
        IntrinsicCwrResponse::Unavailable { activation, reason } => {
            format!("{activation} is unavailable: {reason}.")
        }
        IntrinsicCwrResponse::ConversationExported { export } => {
            serde_json::to_string_pretty(export).expect("serializable export")
        }
        IntrinsicCwrResponse::ConversationImported { conversation_ref } => {
            format!("Imported conversation {conversation_ref}.")
        }
        IntrinsicCwrResponse::ConversationForked {
            conversation_ref,
            source_conversation_ref,
            source_message_ref,
        } => format!(
            "Forked conversation {conversation_ref} from {source_conversation_ref} through {source_message_ref}."
        ),
        IntrinsicCwrResponse::ConversationRenamed {
            conversation_ref,
            title,
        } => format!("Renamed conversation {conversation_ref} to {title:?}."),
        IntrinsicCwrResponse::ConversationArchived {
            conversation_ref,
            archived,
        } => format!(
            "{} conversation {conversation_ref}.",
            if *archived { "Archived" } else { "Restored" }
        ),
        IntrinsicCwrResponse::ConversationsListed { response } => {
            serde_json::to_string_pretty(&response.page)
                .expect("serializable conversation summaries")
        }
        IntrinsicCwrResponse::ConversationRead { response } => {
            serde_json::to_string_pretty(&response.records)
                .expect("serializable conversation records")
        }
        IntrinsicCwrResponse::ConversationBrowsed { response } => {
            serde_json::to_string_pretty(&response.records)
                .expect("serializable conversation browse result")
        }
        IntrinsicCwrResponse::Reloaded { changes } => {
            let mut lines = vec!["CWR graph reloaded.".to_owned()];
            for (label, values) in [
                ("Coworkers added", &changes.added_coworkers),
                ("Coworkers removed", &changes.removed_coworkers),
                ("Coworkers updated", &changes.updated_coworkers),
                ("Actions added", &changes.added_actions),
                ("Actions removed", &changes.removed_actions),
            ] {
                if !values.is_empty() {
                    lines.push(format!("{label}: {}", values.join(", ")));
                }
            }
            if lines.len() == 1 {
                lines.push("Catalog unchanged.".into());
            }
            lines.join("\n")
        }
        IntrinsicCwrResponse::ReloadFailed { error } => {
            format!("CWR graph reload failed: {error}\nThe current graph remains loaded.")
        }
        IntrinsicCwrResponse::InvocationDebugged { report } => {
            serde_json::to_string_pretty(report).unwrap_or_else(|_| "Invocation inspected.".into())
        }
        IntrinsicCwrResponse::InvocationDebugFailed { error, .. } => {
            format!("CWR invocation inspection failed: {error}")
        }
        IntrinsicCwrResponse::InvocationStopped { invocation_ref, .. } => {
            format!("Stopped {invocation_ref}.")
        }
        IntrinsicCwrResponse::InvocationStopTooLate { invocation_ref } => {
            format!("Could not stop {invocation_ref}: it already finished.")
        }
        IntrinsicCwrResponse::InvocationStopFailed { error, .. } => {
            format!("CWR invocation stop failed: {error}")
        }
    }
}

pub fn render_intrinsic_cwr_coworker(coworker: &IntrinsicCwrCatalogCoworker) -> String {
    let unavailable = coworker
        .details
        .get("bindings")
        .and_then(Value::as_array)
        .and_then(|bindings| {
            if bindings.is_empty()
                || bindings
                    .iter()
                    .any(|binding| binding["available"].as_bool().unwrap_or(false))
            {
                return None;
            }
            let binding = bindings
                .iter()
                .max_by_key(|binding| binding["default"].as_bool().unwrap_or(false))?;
            if binding["readiness"]["status"].as_str() == Some("ready") {
                Some("No authorized route is available.")
            } else {
                binding["readiness"]["message"]
                    .as_str()
                    .or(Some("The provider is unavailable."))
            }
        });
    match unavailable {
        Some(message) => format!(
            "{} — {} (unavailable: {message})",
            coworker.handle, coworker.name
        ),
        None => format!("{} — {}", coworker.handle, coworker.name),
    }
}
