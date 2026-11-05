use std::{
    env,
    error::Error,
    ffi::{OsStr, OsString},
    io::BufReader,
    os::unix::net::UnixStream,
    path::Path,
};

use cwr::adapter::{read_adapter_message, write_adapter_message};
use serde_json::{json, Value};

use cwr::v2026_09::invocation::ActionResponse;

use super::action::{action_process_result, parse_action_input};

pub(super) const COWORKER_INVOKE_SOCKET_ENV: &str = "CWR_COWORKER_INVOKE_SOCKET";
const COWORKER_INVOKE_USAGE: &str =
    "usage: cwr coworker invoke <@actor--verb--object> [--key=value ...|json-file]";

pub(super) fn run_coworker_command(
    arguments: &[OsString],
) -> Result<(), Box<dyn Error + Send + Sync>> {
    match arguments {
        [command, rest @ ..] if command == OsStr::new("invoke") => {
            run_coworker_invoke(rest, env::var_os(COWORKER_INVOKE_SOCKET_ENV).as_deref())
        }
        _ => Err(COWORKER_INVOKE_USAGE.into()),
    }
}

pub(super) fn coworker_help(arguments: &[OsString]) -> &'static str {
    match arguments {
        [command, ..] if command == OsStr::new("invoke") => COWORKER_INVOKE_USAGE,
        _ => COWORKER_INVOKE_USAGE,
    }
}

fn run_coworker_invoke(
    arguments: &[OsString],
    socket: Option<&OsStr>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let (activation, input_arguments) = arguments
        .split_first()
        .ok_or("cwr coworker invoke requires an action activation")?;
    let activation = activation
        .to_str()
        .ok_or("cwr coworker invoke activation must be valid UTF-8")?;
    if input_arguments
        .iter()
        .any(|argument| argument == OsStr::new("-"))
    {
        return Err(
            "cwr coworker invoke cannot read action input from the Shell coworker's protocol stdin; use --key=value arguments or a JSON file"
                .into(),
        );
    }
    let socket = socket
        .ok_or("cwr coworker invoke is available only while a Shell coworker command is running")?;
    let mut stream = UnixStream::connect(Path::new(socket))
        .map_err(|error| format!("cannot reach the running Shell coworker: {error}"))?;
    let mut reader = BufReader::new(stream.try_clone()?);

    write_adapter_message(
        &mut stream,
        &json!({"type":"describe_action", "activation":activation}),
    )?;
    let described: Value = read_adapter_message(&mut reader)?
        .ok_or("the running Shell coworker closed before describing the action")?;
    if described["type"] == "error" {
        return Err(bridge_error(&described).into());
    }
    if described["type"] != "action" || described["activation"] != activation {
        return Err("the running Shell coworker returned an invalid action description".into());
    }
    let input_schema = described
        .get("input_schema")
        .ok_or("the running Shell coworker returned no action input schema")?;
    let input = parse_action_input(input_schema, input_arguments, "cwr coworker invoke")?;

    write_adapter_message(
        &mut stream,
        &json!({
            "type":"invoke_action",
            "activation":activation,
            "input":input
        }),
    )?;
    let response: Value = read_adapter_message(&mut reader)?
        .ok_or("the running Shell coworker closed before returning the action")?;
    if response["type"] == "error" {
        return Err(bridge_error(&response).into());
    }
    let admitted = response["type"] == "started";
    let expected_type = if admitted { "started" } else { "completed" };
    let result = response
        .get("result")
        .filter(|_| response["type"] == expected_type)
        .ok_or("the running Shell coworker returned an invalid action result")?;
    println!("{}", serde_json::to_string_pretty(result)?);
    if admitted {
        Ok(())
    } else {
        let response: ActionResponse = serde_json::from_value(result.clone())?;
        action_process_result(response.status).map_err(Into::into)
    }
}

fn bridge_error(response: &Value) -> String {
    response["message"]
        .as_str()
        .unwrap_or("the running Shell coworker rejected the request")
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{os::unix::net::UnixListener, thread};
    use tempfile::tempdir;

    #[test]
    fn invoke_requires_a_running_shell_coworker() {
        let error = run_coworker_invoke(&[OsString::from("@helper--run--task")], None).unwrap_err();
        assert!(error
            .to_string()
            .contains("only while a Shell coworker command is running"));
    }

    #[test]
    fn invoke_rejects_protocol_stdin_as_action_input() {
        let error = run_coworker_invoke(
            &[OsString::from("@helper--run--task"), OsString::from("-")],
            None,
        )
        .unwrap_err();
        assert!(error.to_string().contains("protocol stdin"));
    }

    #[test]
    fn invoke_uses_the_described_schema_and_waits_for_the_result() {
        let directory = tempdir().unwrap();
        let socket = directory.path().join("invoke.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let describe: Value = read_adapter_message(&mut reader).unwrap().unwrap();
            assert_eq!(
                describe,
                json!({
                    "type":"describe_action",
                    "activation":"@helper--run--task"
                })
            );
            write_adapter_message(
                &mut stream,
                &json!({
                    "type":"action",
                    "activation":"@helper--run--task",
                    "input_schema":{
                        "type":"object",
                        "properties":{"count":{"type":"integer"}}
                    }
                }),
            )
            .unwrap();
            let invoke: Value = read_adapter_message(&mut reader).unwrap().unwrap();
            assert_eq!(
                invoke,
                json!({
                    "type":"invoke_action",
                    "activation":"@helper--run--task",
                    "input":{"count":3}
                })
            );
            write_adapter_message(
                &mut stream,
                &json!({
                    "type":"completed",
                    "result":{
                        "invocation_ref":"~@/invocations/child",
                        "sequence":2,
                        "status":200,
                        "summary":"Finished.",
                        "result":{"ok":true}
                    }
                }),
            )
            .unwrap();
        });
        run_coworker_invoke(
            &[
                OsString::from("@helper--run--task"),
                OsString::from("--count=3"),
            ],
            Some(socket.as_os_str()),
        )
        .unwrap();
        server.join().unwrap();
    }

    #[test]
    fn invoke_async_returns_after_admission() {
        let directory = tempdir().unwrap();
        let socket = directory.path().join("invoke.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let _: Value = read_adapter_message(&mut reader).unwrap().unwrap();
            write_adapter_message(
                &mut stream,
                &json!({
                    "type":"action",
                    "activation":"@helper--run--task",
                    "input_schema":{"type":"object"}
                }),
            )
            .unwrap();
            let invoke: Value = read_adapter_message(&mut reader).unwrap().unwrap();
            assert_eq!(invoke["type"], "invoke_action");
            assert!(invoke.get("detach").is_none());
            write_adapter_message(
                &mut stream,
                &json!({
                    "type":"started",
                    "result":{
                        "invocation_ref":"~@/invocations/child",
                        "state":"admitted",
                        "queued":true
                    }
                }),
            )
            .unwrap();
        });
        run_coworker_invoke(
            &[OsString::from("@helper--run--task")],
            Some(socket.as_os_str()),
        )
        .unwrap();
        server.join().unwrap();
    }
}
