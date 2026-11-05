use std::{collections::BTreeSet, error::Error, ffi::OsStr, path::Path};

use cwr::intrinsic_cwr::IntrinsicCwrResponse;

use super::{
    exec_io::{read_exec_source, write_json_record},
    exec_signal::ExecSigint,
    host::requested_cli_actor_ref,
    status::print_responding,
    usage,
};

pub(super) fn run_exec_command(
    arguments: &[std::ffi::OsString],
    cwr_toml: &Path,
    actor_ref: Option<&OsStr>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    if matches!(arguments, [mode, ..] if mode == OsStr::new("--records"))
        || matches!(arguments, [json, records, ..] if json == OsStr::new("--json") && records == OsStr::new("--records"))
    {
        return Err("cwr exec --records is no longer supported".into());
    }
    let (presentation, arguments) = match arguments {
        [mode, rest @ ..] if mode == OsStr::new("--json") => (ExecPresentation::Json, rest),
        _ => (ExecPresentation::Human, arguments),
    };
    let (conversation_ref, message) = match arguments {
        [message] => (None, message.clone()),
        [resume, conversation_ref, message] if resume == OsStr::new("resume") => {
            let conversation_ref = conversation_ref
                .clone()
                .into_string()
                .map_err(|_| "exact conversation ref must be valid UTF-8")?;
            (Some(conversation_ref), message.clone())
        }
        [] => {
            let mode = match presentation {
                ExecPresentation::Human => "",
                ExecPresentation::Json => " --json",
            };
            return Err(format!("cwr exec{mode} requires a message").into());
        }
        _ => return usage(),
    };
    run_exec(message, presentation, conversation_ref, cwr_toml, actor_ref)
}

#[derive(Clone, Copy)]
enum ExecPresentation {
    Human,
    Json,
}

fn run_exec(
    message: std::ffi::OsString,
    presentation: ExecPresentation,
    conversation_ref: Option<String>,
    cwr_toml: &Path,
    actor_ref: Option<&OsStr>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let source_text = read_exec_source(message)?;
    let actor_ref = requested_cli_actor_ref(actor_ref)?;
    let mut client = crate::remote::RemoteClient::open(cwr_toml, actor_ref)?;
    let leading = source_text
        .split_ascii_whitespace()
        .next()
        .unwrap_or_default();
    let target_ref = client
        .catalog()
        .actions
        .iter()
        .find(|action| action.activation == leading)
        .map(|action| action.actor_ref.as_str())
        .or_else(|| {
            client
                .catalog()
                .actors
                .iter()
                .find(|actor| actor.handle == leading)
                .map(|actor| actor.actor_ref.as_str())
        });
    let target_handle = target_ref.and_then(|actor_ref| {
        client
            .catalog()
            .actors
            .iter()
            .find(|actor| actor.actor_ref == actor_ref)
            .map(|actor| actor.handle.clone())
    });
    if let Some(target_handle) = &target_handle {
        print_responding(target_handle)?;
    }

    let before_messages = conversation_ref
        .as_deref()
        .and_then(|reference| client.conversation(reference))
        .map_or(0, |conversation| conversation.messages.len());
    let receipt = match client.submit_text(conversation_ref, source_text)? {
        crate::remote::RemoteSubmission::Conversation(receipt) => receipt,
        crate::remote::RemoteSubmission::Intrinsic(response) => {
            present_intrinsic_cwr_response(&response, presentation)?;
            if let Some(error) = intrinsic_cwr_command_failure(&response) {
                return Err(error.into());
            }
            return Ok(());
        }
    };
    if matches!(presentation, ExecPresentation::Json) {
        write_json_record(&serde_json::to_value(&receipt)?)?;
    }
    let mut printed_activity = client
        .conversation(&receipt.conversation_ref)
        .into_iter()
        .flat_map(|conversation| &conversation.activity)
        .map(|activity| activity.activity_ref.clone())
        .collect::<BTreeSet<_>>();
    let mut sigint = ExecSigint::install()?;
    let turn = client.wait_for_turn_interruptible(
        &receipt,
        |event| {
            if matches!(presentation, ExecPresentation::Json) {
                if crate::remote::event_updates_turn(event, &receipt, &mut printed_activity) {
                    write_json_record(&serde_json::to_value(event)?)?;
                }
                return Ok(());
            }
            crate::remote::present_turn_progress(event, &receipt, &mut printed_activity);
            Ok(())
        },
        || sigint.take_interrupted(),
    )?;
    sigint.finish()?;
    let Some(turn) = turn else {
        if matches!(presentation, ExecPresentation::Human) {
            println!(
                "Recorded message in conversation {}.",
                receipt.conversation_ref
            );
        }
        return Ok(());
    };
    if matches!(presentation, ExecPresentation::Json) {
        write_json_record(&serde_json::to_value(&turn.conversation)?)?;
    } else {
        for message in turn.conversation.messages.iter().skip(before_messages) {
            if message.reply_to_message_ref.as_deref() != receipt.message_ref.as_deref() {
                continue;
            }
            let handle = client
                .catalog()
                .actors
                .iter()
                .find(|actor| actor.actor_ref == message.author_actor_ref)
                .map_or(message.author_actor_ref.as_str(), |actor| {
                    actor.handle.as_str()
                });
            println!("{handle}: {}", message.text);
        }
        let handle = client
            .catalog()
            .actors
            .iter()
            .find(|actor| actor.actor_ref == turn.actor_ref)
            .map_or(turn.actor_ref.as_str(), |actor| actor.handle.as_str());
        let outcome = if turn.status.is_completed() {
            "completed"
        } else if turn.status.is_stopped() {
            "stopped"
        } else if turn.status.is_blocked() {
            "blocked"
        } else {
            "failed"
        };
        println!("{handle} {outcome} ({}).", turn.status.as_u16());
    }
    if turn.status.is_completed() {
        Ok(())
    } else {
        Err(turn
            .summary
            .unwrap_or_else(|| format!("work returned status {}", turn.status.as_u16()))
            .into())
    }
}

pub(super) fn intrinsic_cwr_command_failure(response: &IntrinsicCwrResponse) -> Option<String> {
    match response {
        IntrinsicCwrResponse::InvocationStopTooLate { invocation_ref } => Some(format!(
            "could not stop {invocation_ref}: it already finished"
        )),
        IntrinsicCwrResponse::InvocationStopFailed { error, .. } => {
            Some(format!("CWR invocation stop failed: {error}"))
        }
        _ => None,
    }
}

fn present_intrinsic_cwr_response(
    response: &IntrinsicCwrResponse,
    presentation: ExecPresentation,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    if matches!(presentation, ExecPresentation::Json) {
        return write_json_record(&serde_json::to_value(response)?);
    }
    match response {
        IntrinsicCwrResponse::Coworkers { coworkers } => {
            for coworker in coworkers {
                println!(
                    "{}",
                    cwr::intrinsic_cwr::render_intrinsic_cwr_coworker(coworker)
                );
            }
        }
        IntrinsicCwrResponse::Actions { actions, .. } => {
            for action in actions {
                println!("{action}");
            }
        }
        IntrinsicCwrResponse::Help { actions } => {
            for action in actions.iter().filter(|action| action.starts_with("@cwr--")) {
                println!("{action}");
            }
        }
        IntrinsicCwrResponse::Unknown { command, actions } => {
            println!("CWR does not recognize {command:?}.");
            for action in actions.iter().filter(|action| action.starts_with("@cwr--")) {
                println!("{action}");
            }
        }
        IntrinsicCwrResponse::Unavailable { activation, reason } => {
            println!("{activation} is unavailable: {reason}.");
        }
        IntrinsicCwrResponse::Reloaded { .. } => println!("CWR graph reloaded."),
        IntrinsicCwrResponse::ReloadFailed { error } => {
            println!("CWR graph reload failed: {error}");
        }
        IntrinsicCwrResponse::InvocationDebugged { report } => {
            println!("{}", serde_json::to_string_pretty(report)?);
        }
        IntrinsicCwrResponse::InvocationDebugFailed { error, .. } => {
            println!("CWR invocation inspection failed: {error}");
        }
        IntrinsicCwrResponse::InvocationStopped { invocation_ref, .. } => {
            println!("Stopped {invocation_ref}.");
        }
        IntrinsicCwrResponse::InvocationStopTooLate { invocation_ref } => {
            println!("Could not stop {invocation_ref}: it already finished.");
        }
        IntrinsicCwrResponse::InvocationStopFailed { error, .. } => {
            println!("CWR invocation stop failed: {error}");
        }
        IntrinsicCwrResponse::ConversationExported { export } => {
            println!("{}", serde_json::to_string_pretty(export)?)
        }
        IntrinsicCwrResponse::ConversationImported { conversation_ref } => {
            println!("Imported {conversation_ref}.")
        }
        IntrinsicCwrResponse::ConversationForked {
            conversation_ref, ..
        } => println!("Forked {conversation_ref}."),
        IntrinsicCwrResponse::ConversationRenamed {
            conversation_ref,
            title,
        } => println!("Renamed {conversation_ref} to {title:?}."),
        IntrinsicCwrResponse::ConversationArchived {
            conversation_ref,
            archived,
        } => println!(
            "{} {conversation_ref}.",
            if *archived { "Archived" } else { "Restored" }
        ),
        IntrinsicCwrResponse::ConversationsListed { .. }
        | IntrinsicCwrResponse::ConversationRead { .. }
        | IntrinsicCwrResponse::ConversationBrowsed { .. } => {
            println!(
                "{}",
                cwr::intrinsic_cwr::render_intrinsic_cwr_response(response)
            );
        }
    }
    Ok(())
}
