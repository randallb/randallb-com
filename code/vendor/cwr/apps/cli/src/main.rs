use std::{
    env,
    error::Error,
    ffi::{OsStr, OsString},
    process::ExitCode,
};

mod action;
mod adapter;
mod check;
mod composition;
mod coworker;
mod credentials;
mod entity;
mod exec;
mod exec_io;
mod exec_signal;
mod graphql;
mod host;
mod identity;
mod init;
mod remote;
mod resident;
mod runtime_data;
mod sms_action;
mod startup_timing;
mod status;
mod tui;
mod workspace;

use action::run_actions_command;
use check::run_check_command;
use coworker::run_coworker_command;
use exec::run_exec_command;
use init::run_init_command;
use resident::run_runtime_command;
use tui::run_tui;

const TOP_LEVEL_USAGE: &str = "usage: cwr [--workspace <workspace-directory|CWR.toml>] [--actor <exact-actor-ref>] | cwr [--workspace <workspace-directory|CWR.toml>] [--actor <exact-actor-ref>] resume <last|conversation-ref> | cwr init [workspace-directory] | cwr adapter <configure|status|repair> <adapter-ref> [provider_login] | cwr adapter import <adapter-ref> --source <absolute-path> [--dry-run] [--replace] | cwr [--workspace <workspace-directory|CWR.toml>] runtime <status|stop> | cwr [--workspace <workspace-directory|CWR.toml>] check <~@|~@/resource|workspace-directory|2026-09-resource.toml> | cwr [--workspace <workspace-directory|CWR.toml>] [--actor <exact-actor-ref>] exec [--json] [resume <conversation-ref>] <message> | cwr [--workspace <workspace-directory|CWR.toml>] [--actor <exact-actor-ref>] actions list | cwr [--workspace <workspace-directory|CWR.toml>] [--actor <exact-actor-ref>] actions invoke <@actor--verb--object> [--key=value ...|json-file|-] | cwr coworker invoke <@actor--verb--object> [--key=value ...|json-file|-] | cwr [--workspace <workspace-directory|CWR.toml>] graphql [--watch] <query|-> [variables-json] | cwr entity --help";
const GRAPHQL_USAGE: &str = "usage: cwr [--workspace <path>] [--actor <startup-actor-ref>] graphql [--watch] <query|-> [variables-json]";
const ATTENDED_USAGE: &str = "usage: cwr [--workspace <workspace-directory|CWR.toml>] [--actor <exact-actor-ref>] [resume <last|conversation-ref>]";
const INIT_USAGE: &str = "usage: cwr init [workspace-directory]";
const RUNTIME_USAGE: &str =
    "usage: cwr [--workspace <workspace-directory|CWR.toml>] runtime <status|stop>";
const CHECK_USAGE: &str =
    "usage: cwr [--workspace <workspace-directory|CWR.toml>] check <~@|~@/resource|workspace-directory|2026-09-resource.toml>";
const GATEWAY_FINGERPRINT_USAGE: &str =
    "usage: cwr --workspace <workspace-directory|CWR.toml|WORKSPACE.toml> gateway-fingerprint <connection-ref> <runtime-ref> <working-directory>";
const SMS_ACTION_USAGE: &str =
    "usage: cwr sms-action <receive--message|deliver--message|handle--request>";
const EXEC_USAGE: &str = "usage: cwr [--workspace <workspace-directory|CWR.toml>] [--actor <exact-actor-ref>] exec [--json] [resume <conversation-ref>] <message>";
const ACTIONS_USAGE: &str = "usage: cwr [--workspace <workspace-directory|CWR.toml>] [--actor <exact-actor-ref>] actions list | cwr [--workspace <workspace-directory|CWR.toml>] [--actor <exact-actor-ref>] actions invoke <@actor--verb--object> [--key=value ...|json-file|-]";
const ACTIONS_LIST_USAGE: &str = "usage: cwr [--workspace <workspace-directory|CWR.toml>] [--actor <exact-actor-ref>] actions list";
const ACTIONS_INVOKE_USAGE: &str =
    "usage: cwr [--workspace <workspace-directory|CWR.toml>] [--actor <exact-actor-ref>] actions invoke <@actor--verb--object> [--key=value ...|json-file|-]";

#[derive(Default)]
struct GlobalOptions<'a> {
    workspace: Option<&'a OsStr>,
    actor: Option<&'a OsStr>,
}

fn main() -> ExitCode {
    startup_timing::initialize();
    let result = run();
    startup_timing::flush(result.is_ok());
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("cwr: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), Box<dyn Error + Send + Sync>> {
    let preparation = startup_timing::span("cli_preparation");
    runtime_data::configure_packaged_module_path()?;
    let arguments: Vec<_> = env::args_os().skip(1).collect();
    preparation.finish();
    run_arguments(&arguments)
}

fn run_arguments(arguments: &[OsString]) -> Result<(), Box<dyn Error + Send + Sync>> {
    if let Some(help) = help_for_arguments(arguments) {
        println!("{help}");
        return Ok(());
    }
    let (options, arguments) = parse_global_options(arguments)?;
    if is_tui_arguments(arguments) {
        return run_tui(arguments, options.workspace, options.actor);
    }
    match arguments {
        [command, rest @ ..] if command == OsStr::new("adapter") && options_is_empty(&options) => {
            adapter::run(rest)
        }
        [command, rest @ ..] if command == OsStr::new("entity") && options_is_empty(&options) => {
            entity::run(rest)
        }
        [command, rest @ ..] if command == OsStr::new("graphql") => {
            let workspace = workspace::selected_workspace_manifest(options.workspace)?;
            graphql::run(rest, &workspace, options.actor)
        }
        [command] if command == OsStr::new("credentials") && options_is_empty(&options) => {
            credentials::run()
        }
        [command] if command == OsStr::new("connections") && options_is_empty(&options) => {
            credentials::connections()
        }
        [command] if command == OsStr::new("environment") && options_is_empty(&options) => {
            credentials::environment()
        }
        [command] if command == OsStr::new("identity") && options_is_empty(&options) => {
            identity::run()
        }
        [command, rest @ ..]
            if command == OsStr::new("workspace") && options_is_empty(&options) =>
        {
            composition::run(rest)
        }
        [command] if command == OsStr::new("init") && options_is_empty(&options) => {
            run_init_command(None)
        }
        [command, workspace_directory] if command == OsStr::new("init") => {
            if !options_is_empty(&options) {
                return Err(
                    format!("cwr init does not accept global options; {INIT_USAGE}").into(),
                );
            }
            run_init_command(Some(workspace_directory))
        }
        [command, rest @ ..] if command == OsStr::new("runtime") && options.actor.is_none() => {
            let selection = startup_timing::span("workspace_selection");
            let workspace = workspace::selected_workspace_manifest(options.workspace)?;
            selection.finish();
            run_runtime_command(rest, &workspace)
        }
        [command, input] if command == OsStr::new("check") && options.actor.is_none() => {
            run_check_command(input, options.workspace)
        }
        [command, connection_ref, runtime_ref, working_path]
            if (command == OsStr::new("gateway-fingerprint")
                || command == OsStr::new("gateway-context"))
                && options.actor.is_none() =>
        {
            check::run_gateway_fingerprint_command(
                connection_ref,
                runtime_ref,
                working_path,
                options.workspace,
                command == OsStr::new("gateway-context"),
            )
        }
        [command, rest @ ..]
            if command == OsStr::new("sms-action") && options_is_empty(&options) =>
        {
            sms_action::run(rest)
        }
        [command, rest @ ..] if command == OsStr::new("exec") => {
            let selection = startup_timing::span("workspace_selection");
            let workspace = workspace::selected_workspace_manifest(options.workspace)?;
            selection.finish();
            run_exec_command(rest, &workspace, options.actor)
        }
        [command, rest @ ..] if command == OsStr::new("actions") => {
            let selection = startup_timing::span("workspace_selection");
            let workspace = workspace::selected_workspace_manifest(options.workspace)?;
            selection.finish();
            run_actions_command(rest, &workspace, options.actor)
        }
        [command, rest @ ..] if command == OsStr::new("coworker") => {
            if !options_is_empty(&options) {
                return Err("cwr coworker invoke does not accept global options".into());
            }
            run_coworker_command(rest)
        }
        _ => usage(),
    }
}

fn parse_global_options(
    arguments: &[OsString],
) -> Result<(GlobalOptions<'_>, &[OsString]), Box<dyn Error + Send + Sync>> {
    let mut options = GlobalOptions::default();
    let mut index = 0;
    while let Some(argument) = arguments.get(index) {
        let destination = if argument == OsStr::new("--workspace") {
            &mut options.workspace
        } else if argument == OsStr::new("--actor") {
            &mut options.actor
        } else {
            break;
        };
        if destination.is_some() {
            return Err(format!("{} may only be provided once", argument.to_string_lossy()).into());
        }
        let value = arguments
            .get(index + 1)
            .ok_or_else(|| format!("{} requires a value", argument.to_string_lossy()))?;
        *destination = Some(value);
        index += 2;
    }
    Ok((options, &arguments[index..]))
}

fn options_is_empty(options: &GlobalOptions<'_>) -> bool {
    options.workspace.is_none() && options.actor.is_none()
}

fn is_tui_arguments(arguments: &[OsString]) -> bool {
    match arguments {
        [] => true,
        [command, _] => command == OsStr::new("resume"),
        _ => false,
    }
}

fn help_for_arguments(arguments: &[OsString]) -> Option<&'static str> {
    if !arguments.iter().any(|argument| is_help_flag(argument)) {
        return None;
    }
    let arguments = parse_global_options(arguments)
        .map(|(_, arguments)| arguments)
        .unwrap_or(arguments);
    Some(match arguments {
        [] => ATTENDED_USAGE,
        [command, ..] if command == OsStr::new("entity") => entity::USAGE,
        [command, ..] if command == OsStr::new("adapter") => adapter::USAGE,
        [command, ..] if command == OsStr::new("init") => INIT_USAGE,
        [command, ..] if command == OsStr::new("graphql") => GRAPHQL_USAGE,
        [command, ..] if command == OsStr::new("workspace") => composition::USAGE,
        [command, ..] if command == OsStr::new("runtime") => RUNTIME_USAGE,
        [command, ..] if command == OsStr::new("check") => CHECK_USAGE,
        [command, ..]
            if command == OsStr::new("gateway-fingerprint")
                || command == OsStr::new("gateway-context") =>
        {
            GATEWAY_FINGERPRINT_USAGE
        }
        [command, ..] if command == OsStr::new("sms-action") => SMS_ACTION_USAGE,
        [command, ..] if command == OsStr::new("exec") => EXEC_USAGE,
        [command, ..] if command == OsStr::new("credentials") => credentials::USAGE,
        [command, ..] if command == OsStr::new("connections") => {
            "usage: cwr connections (JSON request on stdin; operations: inspect, save)"
        }
        [command, ..] if command == OsStr::new("environment") => {
            "usage: cwr environment (JSON request on stdin; operations: list, save, remove)"
        }
        [command, subcommand, ..]
            if command == OsStr::new("actions") && subcommand == OsStr::new("list") =>
        {
            ACTIONS_LIST_USAGE
        }
        [command, subcommand, ..]
            if command == OsStr::new("actions") && subcommand == OsStr::new("invoke") =>
        {
            ACTIONS_INVOKE_USAGE
        }
        [command, ..] if command == OsStr::new("actions") => ACTIONS_USAGE,
        [command, rest @ ..] if command == OsStr::new("coworker") => coworker::coworker_help(rest),
        [command, ..] if command == OsStr::new("resume") => ATTENDED_USAGE,
        _ => TOP_LEVEL_USAGE,
    })
}

fn is_help_flag(argument: &OsStr) -> bool {
    matches!(argument.to_str(), Some("-h" | "--help"))
}

pub(crate) fn usage() -> Result<(), Box<dyn Error + Send + Sync>> {
    Err(TOP_LEVEL_USAGE.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    #[test]
    fn removed_commands_show_current_usage() {
        for legacy in [
            "list",
            "action",
            "resource",
            "runtime-data-smoke",
            "desktop",
        ] {
            let result = run_arguments(&[OsString::from(legacy)]);
            assert!(result.unwrap_err().to_string().starts_with("usage: cwr ["));
        }
    }

    #[test]
    fn resume_requires_an_attended_conversation_selector() {
        let result = run_arguments(&[OsString::from("resume")]);
        assert_eq!(result.unwrap_err().to_string(), TOP_LEVEL_USAGE);
    }

    #[test]
    fn attended_invocations_are_identified_after_global_options_are_removed() {
        assert!(is_tui_arguments(&[]));
        assert!(is_tui_arguments(&[
            OsString::from("resume"),
            OsString::from("last"),
        ]));
        assert!(!is_tui_arguments(&[
            OsString::from("resume"),
            OsString::from("last"),
            OsString::from("unexpected"),
        ]));
    }

    #[test]
    fn workspace_and_actor_are_order_independent_global_options() {
        let arguments = [
            OsString::from("--actor"),
            OsString::from("~@/actors/person"),
            OsString::from("--workspace"),
            OsString::from("/tmp/example"),
            OsString::from("exec"),
        ];
        let (options, remaining) = parse_global_options(&arguments).unwrap();
        assert_eq!(options.workspace, Some(OsStr::new("/tmp/example")));
        assert_eq!(options.actor, Some(OsStr::new("~@/actors/person")));
        assert_eq!(remaining, &[OsString::from("exec")]);
    }

    #[test]
    fn init_rejects_more_than_one_workspace_directory() {
        let result = run_arguments(&[
            OsString::from("init"),
            OsString::from("one"),
            OsString::from("two"),
        ]);
        assert!(result.unwrap_err().to_string().starts_with("usage: cwr ["));
    }

    #[test]
    fn help_is_recognized_before_any_positional_argument() {
        let cases: &[(&[&str], &str)] = &[
            (&["--help"], TOP_LEVEL_USAGE),
            (&["-h"], TOP_LEVEL_USAGE),
            (&["init", "--help"], INIT_USAGE),
            (&["adapter", "--help"], adapter::USAGE),
            (&["graphql", "--help"], GRAPHQL_USAGE),
            (&["init", "workspace", "-h"], INIT_USAGE),
            (&["check", "--help"], CHECK_USAGE),
            (
                &["gateway-fingerprint", "--help"],
                GATEWAY_FINGERPRINT_USAGE,
            ),
            (&["exec", "--help"], EXEC_USAGE),
            (&["exec", "--json", "resume", "--help"], EXEC_USAGE),
            (&["exec", "resume", "--help", "message"], EXEC_USAGE),
            (
                &["exec", "resume", "~@/conversations/example", "--help"],
                EXEC_USAGE,
            ),
            (&["actions", "--help"], ACTIONS_USAGE),
            (&["actions", "list", "--help"], ACTIONS_LIST_USAGE),
            (&["actions", "invoke", "--help"], ACTIONS_INVOKE_USAGE),
            (
                &["actions", "invoke", "@helper--run--task", "--help"],
                ACTIONS_INVOKE_USAGE,
            ),
            (
                &["coworker", "invoke", "@helper--run--task", "--help"],
                coworker::coworker_help(&[OsString::from("invoke")]),
            ),
            (&["resume", "--help"], ATTENDED_USAGE),
            (
                &["--actor", "~@/actors/person", "resume", "--help"],
                ATTENDED_USAGE,
            ),
            (&["--actor", "--help", "resume", "last"], ATTENDED_USAGE),
            (
                &["--actor", "~@/actors/person", "resume", "last", "-h"],
                ATTENDED_USAGE,
            ),
            (&["--actor", "--help"], ATTENDED_USAGE),
        ];
        for (arguments, expected) in cases {
            let arguments = arguments.iter().map(OsString::from).collect::<Vec<_>>();
            assert_eq!(help_for_arguments(&arguments), Some(*expected));
        }
    }

    #[test]
    fn help_like_values_remain_ordinary_arguments() {
        for value in ["--help=true", "./--help", "please --help me"] {
            assert_eq!(help_for_arguments(&[OsString::from(value)]), None);
        }
    }
}
