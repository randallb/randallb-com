//! Test process impersonating the CLI while exercising the real MCP helper.
use super::{trace, SESSION};
use cwr::adapter::{read_adapter_message, write_adapter_message};
use serde_json::{json, Value};
use std::{
    fs,
    io::{self, BufReader, Read},
    os::fd::FromRawFd,
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    thread,
    time::Duration,
};

#[test]
fn provider_process() {
    if std::env::var_os("CWR_CLAUDE_FIXTURE").is_none() {
        return;
    }
    run().unwrap();
}

fn run() -> cwr::CwrResult<()> {
    // SAFETY: our shell fixture transfers its original stdout on descriptor 3.
    // The test harness stdout is discarded so it cannot corrupt the CLI stream.
    let mut output = unsafe { fs::File::from_raw_fd(3) };
    fs::write("provider.pid", std::process::id().to_string())?;
    let args = fs::read_to_string("arguments")?;
    let args: Vec<_> = args.lines().collect();
    let argument = |name| {
        args.windows(2)
            .find(|pair| pair[0] == name)
            .map(|pair| pair[1])
            .unwrap()
    };
    let scenario = argument("--model");
    trace(format!("provider scenario={scenario} starting"));
    if args.contains(&"--json-schema") {
        let schema: Value = serde_json::from_str(argument("--json-schema"))?;
        assert_eq!(
            schema["type"], "object",
            "Anthropic tool schemas require an object root"
        );
        for keyword in ["oneOf", "anyOf", "allOf"] {
            assert!(
                schema.get(keyword).is_none(),
                "Anthropic rejects root unions"
            );
        }
    }
    let mut observation = String::new();
    io::stdin().read_to_string(&mut observation)?;
    fs::write("observation.json", observation)?;
    if scenario == "auth-failure" {
        write_adapter_message(
            &mut output,
            &json!({"type":"system","subtype":"api_retry","error":"authentication_failed"}),
        )?;
        thread::sleep(Duration::from_secs(30));
        return Ok(());
    }
    let config: Value = serde_json::from_str(argument("--mcp-config"))?;
    let (mut mcp, mut input, mut reader, tools) = start_mcp(&config, scenario)?;
    let server_name = config["mcpServers"]
        .as_object()
        .unwrap()
        .keys()
        .next()
        .unwrap();
    let mut names: Vec<_> = tools
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| {
            json!(format!(
                "mcp__{server_name}__{}",
                tool["name"].as_str().unwrap()
            ))
        })
        .collect();
    names.extend(
        argument("--tools")
            .split(',')
            .filter(|name| !name.is_empty())
            .map(|name| json!(name)),
    );
    if scenario == "missing-tool" {
        names.clear();
    }
    if scenario == "unexpected-tool" {
        names.push(json!("Bash"));
    }
    let session = if args.contains(&"--fork-session") {
        "c2bce70c-28b5-4dd0-92e7-fdd0aa601db3"
    } else {
        args.windows(2)
            .find(|pair| pair[0] == "--resume")
            .map_or(SESSION, |pair| pair[1])
    };
    let mut init = json!({"type":"system","subtype":"init","session_id":session,
        "tools":names,"mcp_servers":[{"name":server_name,"status":"connected"}]});
    if scenario == "bad-mcp" {
        init["mcp_servers"][0]["status"] = json!("failed");
    }
    write_adapter_message(&mut output, &init)?;
    trace(format!("provider scenario={scenario} init event sent"));
    if scenario == "progress" {
        for event in [
            json!({"type":"system","subtype":"status","status":"requesting","session_id":session}),
            json!({"type":"system","subtype":"status","status":"requesting","session_id":session}),
            json!({"type":"stream_event","session_id":session,"event":{"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"MUST_NOT_APPEAR"}}}),
            json!({"type":"stream_event","session_id":session,"event":{"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"MUST_NOT_APPEAR"}}}),
            json!({"type":"stream_event","session_id":session,"event":{"type":"content_block_start","index":0,"content_block":{"type":"text","text":"MUST_NOT_APPEAR"}}}),
            json!({"type":"system","subtype":"status","status":"compacting","session_id":session}),
        ] {
            write_adapter_message(&mut output, &event)?;
        }
    }
    emit_presentation(&mut output, scenario, session)?;
    if scenario.starts_with("host-") {
        emit_host_tool(
            &mut output,
            &mut input,
            &mut reader,
            session,
            init["tools"][0].as_str().unwrap(),
            tools[0]["name"].as_str().unwrap(),
            scenario,
        )?;
    }
    if scenario == "commentary" {
        for content in [
            json!([{"type":"text","text":"Created the environment; startup is unverified."}]),
            json!([{"type":"thinking","thinking":"private reasoning"},{"type":"tool_use","name":"test--probe--value","input":{}}]),
        ] {
            write_adapter_message(
                &mut output,
                &json!({"type":"assistant","session_id":session,
                "message":{"id":"assistant-1","content":content}}),
            )?;
        }
    }
    fs::write("ready", "ready")?;
    if scenario == "ignore-term" {
        // SAFETY: SIG_IGN is the platform-provided signal disposition constant.
        unsafe {
            libc::signal(libc::SIGTERM, libc::SIG_IGN);
        }
    }
    if matches!(scenario, "stop" | "ignore-term") {
        trace(format!("provider scenario={scenario} entering stop sleep"));
        thread::sleep(Duration::from_secs(30));
        trace(format!("provider scenario={scenario} stop sleep finished"));
    }
    if matches!(scenario, "call" | "unread-reply") {
        write_adapter_message(
            &mut input,
            &json!({"jsonrpc":"2.0","id":"call-1","method":"tools/call","params":{"name":tools[0]["name"],"arguments":{"value":7}}}),
        )?;
        if scenario == "unread-reply" {
            thread::sleep(Duration::from_secs(30));
        }
        // Fill more than an OS pipe while the host callback is still pending.
        for _ in 0..128 {
            write_adapter_message(
                &mut output,
                &json!({"type":"assistant","message":{"content":[{"type":"tool_use","name":tools[0]["name"],"input":{"value":7}},{"type":"thinking","thinking":"x".repeat(2048)}]}}),
            )?;
        }
        let reply: Value = read_adapter_message(&mut reader)?.unwrap();
        trace(format!("provider scenario={scenario} tool reply received"));
        fs::write("reply.json", reply.to_string())?;
    }
    if scenario == "start-action" {
        let valid = json!({"activation":"@probe--double","input":{"value":7}});
        let mut extra = valid.clone();
        extra["unexpected"] = json!(true);
        let mut missing = valid.clone();
        missing.as_object_mut().unwrap().remove("input");
        let mut crossed = valid.clone();
        crossed["input"] = json!({"name":"Ada"});
        let mut unknown = valid.clone();
        unknown["activation"] = json!("@probe--unknown");
        for (index, arguments) in [extra, missing, crossed, unknown, valid].iter().enumerate() {
            write_adapter_message(
                &mut input,
                &json!({
                    "jsonrpc":"2.0","id":10 + index,"method":"tools/call",
                    "params":{"name":"cwr--start--action","arguments":arguments}
                }),
            )?;
            let reply: Value = read_adapter_message(&mut reader)?.unwrap();
            assert_eq!(reply["result"]["isError"], index != 4, "{reply}");
        }
    }
    let result = fixture_result(session, scenario);
    if scenario != "missing-result" {
        write_adapter_message(&mut output, &result)?;
        trace(format!("provider scenario={scenario} terminal event sent"));
    }
    if scenario == "duplicate-result" {
        write_adapter_message(&mut output, &result)?;
    }
    drop(input);
    trace(format!(
        "provider scenario={scenario} MCP stdin closed; waiting for MCP"
    ));
    let mcp_status = mcp.wait()?;
    trace(format!(
        "provider scenario={scenario} MCP exited status={mcp_status}"
    ));
    if matches!(
        scenario,
        "nonzero" | "api-error-result" | "execution-error" | "long-error"
    ) {
        std::process::exit(1);
    }
    Ok(())
}

fn fixture_result(session: &str, scenario: &str) -> Value {
    let mut result = json!({"type":"result","subtype":"success","is_error":false,"session_id":session,"permission_denials":[],
        "result":"Final answer.","structured_output":{"status":200,"summary":"Completed fixture.","result":{"message":"completed"}}});
    match scenario {
        "bad-schema" => result["structured_output"]["result"] = json!({"invalid":true}),
        "host-only-status" => result["structured_output"]["status"] = json!(499),
        "missing-error" => result["structured_output"]["status"] = json!(400),
        "both-payloads" => {
            result["structured_output"]["error"] = json!({"code":"conflicting_result"})
        }
        "api-error-result" => {
            result["is_error"] = json!(true);
            result["result"] = json!("API Error: 400: selected model is unavailable.");
        }
        "execution-error" => {
            result["subtype"] = json!("error_during_execution");
            result["is_error"] = json!(true);
            result.as_object_mut().unwrap().remove("result");
            result["errors"] = json!(["Provider capacity exhausted.", "Please retry later."]);
            result["api_error_status"] = json!(429);
        }
        "long-error" => {
            result["is_error"] = json!(true);
            result["result"] = json!(format!("bad model\n\u{1b}[31m{}", "😶".repeat(2000)));
            result["private_metadata"] = json!("MUST_NOT_APPEAR_IN_DIAGNOSTICS");
        }
        "denied" => {
            result["permission_denials"] = json!([{"tool_name":"mcp__cwp__test--probe--value"}])
        }
        "wrong-session" => result["session_id"] = json!("00000000-0000-0000-0000-000000000000"),
        _ => {}
    }
    result
}

fn start_mcp(
    config: &Value,
    scenario: &str,
) -> cwr::CwrResult<(Child, ChildStdin, BufReader<ChildStdout>, Value)> {
    let server = config["mcpServers"]
        .as_object()
        .unwrap()
        .values()
        .next()
        .unwrap();
    let mut mcp = Command::new(server["command"].as_str().unwrap())
        .args(
            server["args"]
                .as_array()
                .unwrap()
                .iter()
                .map(|value| value.as_str().unwrap()),
        )
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()?;
    trace(format!(
        "provider scenario={scenario} MCP spawned pid={}",
        mcp.id()
    ));
    fs::write("mcp.pid", mcp.id().to_string())?;
    let mut input = mcp.stdin.take().unwrap();
    let mut reader = BufReader::new(mcp.stdout.take().unwrap());
    write_adapter_message(
        &mut input,
        &json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","clientInfo":{"name":"fixture","version":"1"},"capabilities":{}}}),
    )?;
    trace(format!(
        "provider scenario={scenario} waiting for initialize reply"
    ));
    let _: Value = read_adapter_message(&mut reader)?.unwrap();
    trace(format!(
        "provider scenario={scenario} initialize reply received"
    ));
    write_adapter_message(
        &mut input,
        &json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
    )?;
    write_adapter_message(
        &mut input,
        &json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
    )?;
    trace(format!(
        "provider scenario={scenario} waiting for tools list"
    ));
    let listed: Value = read_adapter_message(&mut reader)?.unwrap();
    trace(format!("provider scenario={scenario} tools list received"));
    let tools = listed["result"]["tools"].clone();
    fs::write("tools.json", tools.to_string())?;
    Ok((mcp, input, reader, tools))
}

fn emit_presentation(
    output: &mut impl std::io::Write,
    scenario: &str,
    session: &str,
) -> cwr::CwrResult<()> {
    if scenario == "mixed-tools" {
        for event in [
            json!({"type":"assistant","session_id":session,"message":{"id":"mixed","content":[{"type":"tool_use","id":"a","name":"Bash"},{"type":"tool_use","id":"b","name":"Read"}]}}),
            json!({"type":"user","session_id":session,"message":{"content":[{"type":"tool_result","tool_use_id":"b"}]}}),
            json!({"type":"user","session_id":session,"message":{"content":[{"type":"tool_result","tool_use_id":"a"}]}}),
        ] {
            write_adapter_message(output, &event)?;
        }
    }
    if scenario == "parallel-tools" {
        for event in [
            json!({"type":"assistant","session_id":session,"message":{"id":"parallel","content":[{"type":"tool_use","id":"a","name":"Bash"},{"type":"tool_use","id":"b","name":"Bash"}]}}),
            json!({"type":"user","session_id":session,"message":{"content":[{"type":"tool_result","tool_use_id":"a"}]}}),
            json!({"type":"stream_event","session_id":session,"event":{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}}),
            json!({"type":"user","session_id":session,"message":{"content":[{"type":"tool_result","tool_use_id":"b"}]}}),
            json!({"type":"stream_event","session_id":session,"event":{"type":"content_block_stop","index":0}}),
        ] {
            write_adapter_message(output, &event)?;
        }
    }
    if scenario == "presentation" {
        for event in [
            json!({"type":"stream_event","session_id":session,"event":{"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"PRIVATE_THINKING"}}}),
            json!({"type":"stream_event","session_id":session,"event":{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"PRIVATE_THINKING"}}}),
            json!({"type":"stream_event","session_id":session,"event":{"type":"content_block_stop","index":0}}),
            json!({"type":"assistant","session_id":"foreign-session","message":{"id":"foreign","content":[{"type":"tool_use","id":"foreign-tool","name":"Bash"}]}}),
            json!({"type":"assistant","session_id":session,"message":{"id":"commentary","content":[{"type":"text","text":"Checking the workspace."}]}}),
            json!({"type":"assistant","session_id":session,"message":{"id":"commentary","content":[{"type":"tool_use","id":"bash-1","name":"Bash","input":{"command":"SECRET_ARGUMENT"}}]}}),
            json!({"type":"assistant","session_id":session,"message":{"id":"commentary","content":[{"type":"tool_use","id":"bash-1","name":"Bash","input":{"command":"SECRET_ARGUMENT"}}]}}),
            json!({"type":"user","session_id":session,"message":{"content":[{"type":"tool_result","tool_use_id":"unknown","content":"SECRET_RESULT"},{"type":"tool_result","tool_use_id":"bash-1","is_error":true,"content":"SECRET_RESULT"}]}}),
            json!({"type":"user","session_id":session,"message":{"content":[{"type":"tool_result","tool_use_id":"bash-1","content":"SECRET_RESULT"}]}}),
            json!({"type":"assistant","session_id":session,"message":{"id":"final-text","content":[{"type":"text","text":"Final answer."}]}}),
        ] {
            write_adapter_message(output, &event)?;
        }
    }
    Ok(())
}

fn emit_host_tool(
    output: &mut impl std::io::Write,
    input: &mut impl std::io::Write,
    reader: &mut impl std::io::BufRead,
    session: &str,
    provider_name: &str,
    local_name: &str,
    scenario: &str,
) -> cwr::CwrResult<()> {
    let arguments = if scenario == "host-rejected" {
        json!({"value":"invalid"})
    } else {
        json!({"value":7})
    };
    let start = json!({"type":"assistant","session_id":session,"message":{"id":"host","content":[{"type":"tool_use","id":"host-tool","name":provider_name,"input":arguments}]}});
    if scenario != "host-before" {
        write_adapter_message(output, &start)?;
    }
    write_adapter_message(
        input,
        &json!({"jsonrpc":"2.0","id":"host-call","method":"tools/call","params":{"name":local_name,"arguments":arguments}}),
    )?;
    let reply: Value = read_adapter_message(reader)?.unwrap();
    if scenario == "host-before" {
        write_adapter_message(output, &start)?;
    }
    write_adapter_message(
        output,
        &json!({"type":"user","session_id":session,"message":{"content":[{"type":"tool_result","tool_use_id":"host-tool","is_error":reply["result"]["isError"]}]}}),
    )
}
