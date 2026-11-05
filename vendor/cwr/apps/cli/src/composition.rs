use std::{
    error::Error,
    ffi::{OsStr, OsString},
};

use cwr::v2026_09::local::LocalWorkspaceComposition;
use serde_json::{json, Value};

pub(super) const USAGE: &str = "usage: cwr workspace check <WORKSPACE.toml> [--json] | cwr workspace <graph|coworkers|actions> <WORKSPACE.toml> <runtime> | cwr workspace run <WORKSPACE.toml> <runtime> <message> | cwr workspace invoke <WORKSPACE.toml> <runtime> <@coworker--verb--object> <json-input>";
type Result<T> = std::result::Result<T, Box<dyn Error + Send + Sync>>;

pub(super) fn run(arguments: &[OsString]) -> Result<()> {
    let [command, path, rest @ ..] = arguments else {
        return Err(USAGE.into());
    };
    let composition = LocalWorkspaceComposition::load(path)?;
    if command == OsStr::new("check") && (rest.is_empty() || rest == [OsString::from("--json")]) {
        let report = composition.requirements()?;
        if rest.is_empty() {
            print!("{report}");
        } else {
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
        return if report.readiness == cwr::v2026_09::local::WorkspaceReadiness::Ready {
            Ok(())
        } else {
            Err("workspace has configuration requirements".into())
        };
    }
    let [name, rest @ ..] = rest else {
        return Err(USAGE.into());
    };
    let configured =
        composition.configured_runtime(name.to_str().ok_or("runtime name must be UTF-8")?)?;
    match (command.to_str(), rest) {
        (Some("graph"), []) => {
            let graph = cwr::v2026_09::graph::checked_resolved_graph(
                configured.resolve_graph()?,
                &configured.cwr_toml,
            )?;
            println!("{}", serde_json::to_string_pretty(&graph)?);
        }
        (Some("coworkers"), []) => {
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "runtime_ref": configured.runtime_ref,
                    "coworkers": configured.catalog()?.known_actors,
                }))?
            );
        }
        (Some("actions"), []) => {
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "runtime_ref": configured.runtime_ref,
                    "actions": configured.catalog()?.actions,
                }))?
            );
        }
        (Some("run"), [message]) => {
            let text = message.to_str().ok_or("message must be UTF-8")?;
            let mut client = crate::remote::RemoteClient::open_selected(
                &composition.path,
                Some(&configured.name),
                None,
            )?;
            let submission = client.submit_text(None, text.into())?;
            execute(&mut client, submission)?;
        }
        (Some("invoke"), [activation, input]) => {
            let activation = activation
                .to_str()
                .ok_or("action activation must be UTF-8")?;
            let input: Value =
                serde_json::from_str(input.to_str().ok_or("action input must be UTF-8")?)?;
            let mut client = crate::remote::RemoteClient::open_selected(
                &composition.path,
                Some(&configured.name),
                None,
            )?;
            let submission = client.invoke_action(activation.into(), input)?;
            execute(&mut client, submission)?;
        }
        _ => return Err(USAGE.into()),
    }
    Ok(())
}

fn execute(
    client: &mut crate::remote::RemoteClient,
    submission: crate::remote::RemoteSubmission,
) -> Result<()> {
    let crate::remote::RemoteSubmission::Conversation(receipt) = submission else {
        if let crate::remote::RemoteSubmission::Intrinsic(response) = submission {
            println!("{}", serde_json::to_string_pretty(&response)?);
        }
        return Ok(());
    };
    let mut interrupt = crate::exec_signal::ExecSigint::install()?;
    let turn = client.wait_for_turn_interruptible(
        &receipt,
        |_| Ok(()),
        || interrupt.take_interrupted(),
    )?;
    interrupt.finish()?;
    if let Some(turn) = turn {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "conversation": turn.conversation,
                "receipt": receipt,
                "response": crate::remote::action_response_json(&turn, &receipt),
            }))?
        );
        if !turn.status.is_completed() {
            return Err(format!("invocation finished with status {}", turn.status.as_u16()).into());
        }
    } else {
        println!("{}", serde_json::to_string_pretty(&receipt)?);
    }
    Ok(())
}
