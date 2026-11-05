use std::{
    collections::BTreeSet,
    error::Error,
    ffi::{OsStr, OsString},
    fs,
    io::{self, Read},
    path::Path,
};

#[cfg(test)]
use cwr::v2026_09::local::LocalAction;
use cwr::v2026_09::{
    invocation::ActionResponseStatus,
    local::{load_local_catalog, parse_schema_arguments},
};
use serde_json::Value;

use super::{
    exec_signal::ExecSigint,
    host::{cli_host_identity, requested_cli_actor_ref},
    status::print_responding,
};

const MAX_ACTION_INPUT_BYTES: u64 = 1024 * 1024;

pub(super) fn run_actions_command(
    arguments: &[OsString],
    cwr_toml: &Path,
    actor_ref: Option<&OsStr>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    match arguments {
        [command] if command == OsStr::new("list") => {
            run_actions_list_command(cwr_toml, actor_ref)
        }
        [command, rest @ ..] if command == OsStr::new("invoke") => {
            run_action_command(rest, cwr_toml, actor_ref)
        }
        _ => Err("usage: cwr actions list | cwr actions invoke <@actor--verb--object> [--key=value ...|json-file|-]".into()),
    }
}

fn run_actions_list_command(
    cwr_toml: &Path,
    actor_ref: Option<&OsStr>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    for (activation, title) in [
        (
            cwr::intrinsic_cwr::LIST_COWORKERS_ACTIVATION,
            "List coworkers",
        ),
        (cwr::intrinsic_cwr::LIST_ACTIONS_ACTIVATION, "List actions"),
        (cwr::intrinsic_cwr::SHOW_HELP_ACTIVATION, "Show help"),
        (cwr::intrinsic_cwr::RELOAD_GRAPH_ACTIVATION, "Reload graph"),
        (
            cwr::intrinsic_cwr::DEBUG_INVOCATION_ACTIVATION,
            "Debug invocation",
        ),
        (
            cwr::intrinsic_cwr::STOP_INVOCATION_ACTIVATION,
            "Stop invocation",
        ),
    ] {
        println!("{activation} — {title}");
    }
    let identity = cli_host_identity(cwr_toml, actor_ref)?;
    let mut actions = load_local_catalog(cwr_toml, &identity.acting_actor_ref)?.actions;
    actions.sort_unstable_by(|left, right| left.activation.cmp(&right.activation));
    for action in actions {
        println!("{} — {}", action.activation, action.title);
    }
    Ok(())
}

fn run_action_command(
    arguments: &[OsString],
    cwr_toml: &Path,
    actor_ref: Option<&OsStr>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let (activation, input_arguments) = arguments
        .split_first()
        .ok_or("cwr actions invoke requires an action activation")?;
    let activation = activation
        .to_str()
        .ok_or("cwr actions invoke activation must be valid UTF-8")?;
    let actor_ref = requested_cli_actor_ref(actor_ref)?;
    let mut client = crate::remote::RemoteClient::open(cwr_toml, actor_ref)?;
    let action = client
        .catalog()
        .actions
        .iter()
        .find(|action| action.activation == activation)
        .cloned()
        .ok_or_else(|| format!("unknown action activation {activation}"))?;
    let input = parse_action_input(&action.input_schema, input_arguments, "cwr actions invoke")?;
    let target = client
        .catalog()
        .actors
        .iter()
        .find(|actor| actor.actor_ref == action.actor_ref)
        .ok_or("action target is absent from the local catalog")?;
    let target_handle = target.handle.clone();
    print_responding(&target_handle)?;
    let receipt = match client.invoke_action(activation.into(), input)? {
        crate::remote::RemoteSubmission::Conversation(receipt) => receipt,
        crate::remote::RemoteSubmission::Intrinsic(_) => {
            return Err("ordinary action returned an intrinsic response".into())
        }
    };
    let mut observed_activity = client
        .conversation(&receipt.conversation_ref)
        .into_iter()
        .flat_map(|conversation| &conversation.activity)
        .map(|activity| activity.activity_ref.clone())
        .collect::<BTreeSet<_>>();
    let mut sigint = ExecSigint::install()?;
    let turn = client
        .wait_for_turn_interruptible(
            &receipt,
            |event| {
                crate::remote::present_turn_progress(event, &receipt, &mut observed_activity);
                Ok(())
            },
            || sigint.take_interrupted(),
        )?
        .ok_or("action invocation did not start work")?;
    sigint.finish()?;
    println!(
        "{}",
        serde_json::to_string_pretty(&crate::remote::action_response_json(&turn, &receipt))?
    );
    action_process_result(turn.status).map_err(Into::into)
}

pub(super) fn action_process_result(status: ActionResponseStatus) -> Result<(), String> {
    if status.is_completed() {
        Ok(())
    } else {
        Err(format!("action returned status {}", status.as_u16()))
    }
}

pub(super) fn parse_action_input(
    input_schema: &Value,
    arguments: &[OsString],
    command: &str,
) -> Result<Value, Box<dyn Error + Send + Sync>> {
    if matches!(arguments, [source] if !os_starts_with(source, "--")) {
        return read_json_source(&arguments[0], command);
    }
    if arguments
        .iter()
        .any(|argument| !os_starts_with(argument, "--"))
    {
        return Err(format!(
            "{command} accepts either one JSON source or only --key=value arguments"
        )
        .into());
    }
    let arguments = arguments
        .iter()
        .map(|argument| argument.to_str())
        .collect::<Option<Vec<_>>>()
        .ok_or("action arguments must be valid UTF-8")?;
    parse_schema_arguments(input_schema, &arguments)
}

fn os_starts_with(value: &OsStr, prefix: &str) -> bool {
    value
        .to_str()
        .is_some_and(|value| value.starts_with(prefix))
}

fn read_json_source(source: &OsStr, command: &str) -> Result<Value, Box<dyn Error + Send + Sync>> {
    let mut bytes = Vec::new();
    if source == OsStr::new("-") {
        io::stdin()
            .take(MAX_ACTION_INPUT_BYTES + 1)
            .read_to_end(&mut bytes)?;
    } else {
        let path = Path::new(source);
        let metadata = fs::metadata(path)
            .map_err(|error| format!("cannot read action input {}: {error}", path.display()))?;
        if metadata.len() > MAX_ACTION_INPUT_BYTES {
            return Err(format!("{command} JSON input exceeds 1 MiB").into());
        }
        bytes = fs::read(path)
            .map_err(|error| format!("cannot read action input {}: {error}", path.display()))?;
    }
    if bytes.len() as u64 > MAX_ACTION_INPUT_BYTES {
        return Err(format!("{command} JSON input exceeds 1 MiB").into());
    }
    serde_json::from_slice(&bytes)
        .map_err(|error| format!("action input is not valid JSON: {error}").into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn action() -> LocalAction {
        LocalAction {
            activation: "@bruce--prepare--campaign".into(),
            actor_ref: "~@/actors/bruce".into(),
            actor_integration_ref: "~@/actor_integrations/bruce".into(),
            action_name: "prepare--campaign".into(),
            title: "Prepare campaign".into(),
            description: "Prepare a campaign.".into(),
            input_schema: json!({
                "type": "object",
                "additionalProperties": false,
                "required": ["campaign_name", "max_contacts", "enabled"],
                "properties": {
                    "campaign_name": {"type": "string"},
                    "max_contacts": {"type": "integer"},
                    "enabled": {"type": "boolean"},
                    "input": {"type": "string"},
                    "nested": {"type": "object"}
                }
            }),
            output_schema: json!({"type": "object"}),
        }
    }

    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    #[test]
    fn schema_arguments_require_equals_and_parse_declared_scalar_types() {
        let input = parse_action_input(
            &action().input_schema,
            &args(&[
                "--campaign-name=Organoid researchers",
                "--max-contacts=10",
                "--enabled=false",
                "--input=literal schema property",
            ]),
            "cwr actions invoke",
        )
        .expect("parse action flags");
        assert_eq!(
            input,
            json!({
                "campaign_name": "Organoid researchers",
                "max_contacts": 10,
                "enabled": false,
                "input": "literal schema property"
            })
        );
        assert!(parse_action_input(
            &action().input_schema,
            &args(&["--max-contacts", "10"]),
            "cwr actions invoke",
        )
        .unwrap_err()
        .to_string()
        .contains("either one JSON source"));
    }

    #[test]
    fn schema_arguments_reject_duplicates_unknowns_and_structured_properties() {
        for values in [
            vec!["--enabled=true", "--enabled=false"],
            vec!["--unknown=value"],
            vec!["--nested={}"],
        ] {
            assert!(
                parse_action_input(&action().input_schema, &args(&values), "cwr actions invoke",)
                    .is_err(),
                "{values:?}"
            );
        }
    }

    #[test]
    fn action_process_result_uses_the_numeric_status_class() {
        assert!(action_process_result(ActionResponseStatus::new(202).unwrap()).is_ok());
        for status in [451, 499, 503] {
            assert_eq!(
                action_process_result(ActionResponseStatus::new(status).unwrap()),
                Err(format!("action returned status {status}"))
            );
        }
    }
}
