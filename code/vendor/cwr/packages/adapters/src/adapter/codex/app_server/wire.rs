// Codex image-generation completions include inline base64 image data.
const MAX_PROVIDER_FRAME_BYTES: usize = 64 * 1024 * 1024;
// Keep provider-owned work distinct from threads authored in Codex itself.
const WORKLOOP_THREAD_SOURCE: &str = "workloop";
fn start_thread_with_resume(
    writer: &mut impl Write,
    config: &BoundedActionConfig,
    resume_thread: Option<&str>,
    fork_thread: Option<&str>,
) -> CwrResult<()> {
    let mut namespaces: std::collections::BTreeMap<String, Vec<Value>> =
        std::collections::BTreeMap::new();
    if !config.native_text_result {
        namespaces.entry("cwp".into()).or_default().push(json!({
            "type": "function",
            "name": "cwr--return--result",
            "description": "Report progress or complete, block, or fail the one active incoming CWR action.",
            "inputSchema": config.return_schema
        }));
    }
    for tool in &config.host_tools {
        namespaces
            .entry(tool.namespace.clone())
            .or_default()
            .push(json!({
                "type": "function",
                "name": tool.name,
                "description": tool.description,
                "inputSchema": crate::adapter::tools::provider_schema(&tool.input_schema)
            }));
    }
    let mut provider_config = json!({});
    if matches!(config.provider_access, ProviderAccess::CwrOnly) {
        // Codex reloads workspace requirements before inference and requires a
        // default whenever the configuration defines named permission profiles.
        provider_config["default_permissions"] = json!("cwr-only");
        provider_config["features.shell_tool"] = json!(false);
        provider_config["features.unified_exec"] = json!(false);
        provider_config["web_search"] = json!("disabled");
        provider_config["permissions.cwr-only.filesystem"] = json!({});
        provider_config["permissions.cwr-only.network.enabled"] = json!(false);
    } else {
        provider_config["sandbox_workspace_write"] =
            json!({"network_access": matches!(config.provider_access, ProviderAccess::Normal)});
    }
    if matches!(
        config.provider_access,
        ProviderAccess::Isolated | ProviderAccess::CwrOnly
    ) {
        provider_config["web_search"] = json!("disabled");
        provider_config["features.apps"] = json!(false);
    }
    if config.service_tier_file.is_some() {
        provider_config["features.fast_mode"] = json!(true);
        provider_config["service_tier"] = json!("default");
    }
    if let Some(reasoning_effort) = &config.reasoning_effort {
        provider_config["model_reasoning_effort"] = json!(reasoning_effort);
    }
    provider_config["features.code_mode.direct_only_tool_namespaces"] =
        json!(namespaces.keys().collect::<Vec<_>>());
    let mut thread_params = json!({
        "cwd": config.workspace,
        "approvalPolicy": "never",
        "developerInstructions": config.developer_instructions,
        "config": provider_config,
        "dynamicTools": namespaces.iter().map(|(namespace, tools)| json!({
            "type": "namespace", "name": namespace,
            "description": "Coworker Protocol tools for the active turn.", "tools": tools
        })).collect::<Vec<_>>(),
        "persistExtendedHistory": false
    });
    if matches!(config.provider_access, ProviderAccess::CwrOnly) {
        thread_params["permissions"] = json!("cwr-only");
    } else {
        thread_params["sandbox"] = json!(if matches!(
            config.provider_access,
            ProviderAccess::Isolated
        ) {
            "read-only"
        } else {
            config.sandbox
        });
    }
    if let Some(model) = &config.model {
        thread_params["model"] = json!(model);
    }
    let method = if let Some(thread_id) = resume_thread {
        thread_params
            .as_object_mut()
            .unwrap()
            .remove("dynamicTools");
        thread_params["threadId"] = json!(thread_id);
        thread_params["excludeTurns"] = json!(true);
        "thread/resume"
    } else if let Some(thread_id) = fork_thread {
        // Fork retains the original tools. HistoryPreparation requires the
        // current instructions, tool schemas, and provider settings to match.
        thread_params
            .as_object_mut()
            .unwrap()
            .remove("dynamicTools");
        thread_params["threadId"] = json!(thread_id);
        thread_params["excludeTurns"] = json!(true);
        thread_params["threadSource"] = json!(WORKLOOP_THREAD_SOURCE);
        "thread/fork"
    } else {
        thread_params["ephemeral"] = json!(config.ephemeral);
        thread_params["threadSource"] = json!(WORKLOOP_THREAD_SOURCE);
        "thread/start"
    };
    write_json_line(
        writer,
        &json!({"id": THREAD_START_ID, "method": method, "params": thread_params}),
    )
}
pub(crate) struct ProviderLine {
    line: String,
    received_at: Instant,
}

pub(crate) fn spawn_provider_reader(
    output: impl io::Read + Send + 'static,
) -> mpsc::Receiver<io::Result<ProviderLine>> {
    // At most two bounded provider frames can await processing.
    let (sender, receiver) = mpsc::sync_channel(2);
    thread::spawn(move || {
        let mut reader = BufReader::new(output);
        loop {
            let mut bytes = Vec::new();
            let read = (&mut reader)
                .take((MAX_PROVIDER_FRAME_BYTES + 1) as u64)
                .read_until(b'\n', &mut bytes);
            let line = match read {
                Ok(0) => return,
                Ok(_) if bytes.len() > MAX_PROVIDER_FRAME_BYTES => Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    CodexFailure::frame_too_large(),
                )),
                Ok(_) => String::from_utf8(bytes)
                    .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error)),
                Err(error) => Err(error),
            };
            let failed = line.is_err();
            let received_at = Instant::now();
            if sender
                .send(line.map(|line| ProviderLine { line, received_at }))
                .is_err()
                || failed
            {
                return;
            }
        }
    });
    receiver
}
fn write_json_line(writer: &mut impl Write, value: &Value) -> CwrResult<()> {
    serde_json::to_writer(&mut *writer, value)?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    Ok(())
}
fn write_tool_response(
    writer: &mut impl Write,
    request_id: Value,
    success: bool,
    content: &Value,
) -> CwrResult<()> {
    write_json_line(
        writer,
        &json!({
            "id": request_id,
            "result": {
                "success": success,
                "contentItems": [{
                    "type": "inputText",
                    "text": serde_json::to_string(content)?
                }]
            }
        }),
    )
}
fn validate_instance(label: &str, schema: &Value, instance: &Value) -> CwrResult<()> {
    let validator = jsonschema::validator_for(schema)
        .map_err(|error| invalid_error(format!("{label} schema is invalid: {error}")))?;
    if let Err(error) = validator.validate(instance) {
        return invalid(format!("{label} does not match its schema: {error}"));
    }
    Ok(())
}
fn invalid<T>(message: impl Into<String>) -> CwrResult<T> {
    Err(invalid_error(message))
}
fn invalid_error(message: impl Into<String>) -> Box<dyn std::error::Error + Send + Sync> {
    io::Error::new(io::ErrorKind::InvalidData, message.into()).into()
}

#[cfg(test)]
mod wire_tests {
    use super::*;

    #[test]
    fn large_image_frame_preserves_the_following_completion() {
        let image = json!({
            "method": "item/completed",
            "params": {"item": {
                "type": "image_gen.generation",
                "result": "A".repeat(4 * 1024 * 1024)
            }}
        });
        let completion = json!({"method": "turn/completed"});
        let input = format!("{image}\n{completion}\n");
        let frames = spawn_provider_reader(io::Cursor::new(input.into_bytes()));

        for expected in [image, completion] {
            let frame = frames.recv().unwrap().unwrap();
            assert_eq!(
                serde_json::from_str::<Value>(&frame.line).unwrap(),
                expected
            );
        }
        assert!(frames.recv().is_err());
    }

    #[test]
    fn provider_frame_accepts_exactly_64_mib_including_newline() {
        let input = io::repeat(b' ')
            .take(64 * 1024 * 1024 - 1)
            .chain(io::Cursor::new(b"\n"));
        let frames = spawn_provider_reader(input);
        let frame = frames.recv().unwrap().unwrap();
        assert_eq!(frame.line.len(), 64 * 1024 * 1024);
        assert!(frame.line.ends_with('\n'));
        assert!(frames.recv().is_err());
    }

    #[test]
    fn provider_frame_rejects_over_64_mib_with_or_without_newline() {
        for terminated in [false, true] {
            let tail: &[u8] = if terminated { b"\n" } else { b"" };
            let input = io::repeat(b' ')
                .take(64 * 1024 * 1024 + u64::from(!terminated))
                .chain(io::Cursor::new(tail));
            let frames = spawn_provider_reader(input);
            let error = frames.recv().unwrap().err().expect("oversized frame");
            assert_eq!(error.kind(), io::ErrorKind::InvalidData);
            assert!(error.to_string().contains("64 MiB"));
            assert!(frames.recv().is_err());
        }
    }
}

// Native dynamic-tool image results are model input, not a JSON string of bytes.
fn write_host_tool_response(
    writer: &mut impl Write,
    request_id: Value,
    operation: &str,
    success: bool,
    content: &Value,
) -> CwrResult<()> {
    if success && operation == "cwr.read_file" {
        use base64::Engine;
        let file: cwr::v2026_09::conversation::RetainedFileContent =
            serde_json::from_value(content.clone())?;
        if matches!(
            file.metadata.media_type.as_str(),
            "image/png" | "image/jpeg" | "image/webp" | "image/gif"
        ) {
            let bytes = hex::decode(&file.content_hex)?;
            let url = format!(
                "data:{};base64,{}",
                file.metadata.media_type,
                base64::engine::general_purpose::STANDARD.encode(bytes)
            );
            return write_json_line(
                writer,
                &json!({"id":request_id,"result":{
                "success":true,"contentItems":[
                    {"type":"inputText","text":serde_json::to_string(&file.metadata)?},
                    {"type":"inputImage","imageUrl":url}]}}),
            );
        }
    }
    write_tool_response(writer, request_id, success, content)
}

#[cfg(test)]
mod image_tests {
    use super::*;
    #[test]
    fn image_tool_results_deliver_native_image_content_without_hex_in_text() {
        let mut wire = Vec::new();
        let content = json!({"metadata":{"file_ref":"cwp://test/files/1","owner_actor_ref":"cwp://test/actors/1",
            "filename":"shot.png","media_type":"image/png","size_bytes":3,"sha256":"test"},"content_hex":"010203"});
        write_host_tool_response(&mut wire, json!(5), "cwr.read_file", true, &content).unwrap();
        let result: Value = serde_json::from_slice(&wire).unwrap();
        assert_eq!(
            result["result"]["contentItems"][1],
            json!({"type":"inputImage","imageUrl":"data:image/png;base64,AQID"})
        );
        assert!(!result["result"]["contentItems"][0]["text"]
            .as_str()
            .unwrap()
            .contains("content_hex"));
        wire.clear();
        write_host_tool_response(
            &mut wire,
            json!(6),
            "cwr.read_file",
            false,
            &json!({"error":"unavailable"}),
        )
        .unwrap();
        let result: Value = serde_json::from_slice(&wire).unwrap();
        assert_eq!(result["result"]["success"], false);
        assert_eq!(
            result["result"]["contentItems"].as_array().unwrap().len(),
            1
        );
    }
}
