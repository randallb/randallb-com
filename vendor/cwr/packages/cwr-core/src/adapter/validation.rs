fn validate_launch(adapter: &ResolvedProcessAdapter) -> CwrResult<()> {
    if !adapter.executable.is_absolute() {
        return protocol_failure("resolved adapter executable must be an absolute path");
    }
    if adapter
        .current_directory
        .as_ref()
        .is_some_and(|directory| !directory.is_absolute())
    {
        return protocol_failure("resolved adapter working directory must be an absolute path");
    }
    Ok(())
}

pub fn validate_adapter_invocation(invocation: &AdapterInvocation) -> CwrResult<()> {
    CwrRef::parse(&invocation.invocation_ref)
        .map_err(|error| protocol_error(format!("adapter invocation ref is invalid: {error}")))?;
    if let Some(model_run_ref) = &invocation.model_run_ref {
        CwrRef::parse(model_run_ref).map_err(|error| {
            protocol_error(format!("adapter model run ref is invalid: {error}"))
        })?;
    }
    if let Some(integration_ref) = &invocation.integration_ref {
        CwrRef::parse(integration_ref).map_err(|error| {
            protocol_error(format!("adapter integration ref is invalid: {error}"))
        })?;
    }
    if invocation.action_name.trim().is_empty() {
        return protocol_failure("adapter action name must not be empty");
    }
    validate_action_value("adapter action input", &invocation.input)?;
    for (label, params) in [
        ("adapter", &invocation.adapter_params),
        ("integration", &invocation.integration_params),
        ("actor integration", &invocation.actor_integration_params),
    ] {
        if !params.is_object() {
            return protocol_failure(format!("{label} params must be an object"));
        }
    }
    jsonschema::validator_for(&invocation.output_schema)
        .map_err(|error| protocol_error(format!("adapter output schema is invalid: {error}")))?;
    let mut operation_names = BTreeSet::new();
    for operation in &invocation.host_operations {
        if operation.name.trim().is_empty() {
            return protocol_failure("adapter host operation name must not be empty");
        }
        if operation.description.trim().is_empty() {
            return protocol_failure("adapter host operation description must not be empty");
        }
        if !operation_names.insert(&operation.name) {
            return protocol_failure(format!(
                "adapter host operation {} is declared more than once",
                operation.name
            ));
        }
        jsonschema::validator_for(&operation.input_schema).map_err(|error| {
            protocol_error(format!(
                "adapter host operation {} input schema is invalid: {error}",
                operation.name
            ))
        })?;
        jsonschema::validator_for(&operation.output_schema).map_err(|error| {
            protocol_error(format!(
                "adapter host operation {} output schema is invalid: {error}",
                operation.name
            ))
        })?;
    }
    Ok(())
}

fn validate_adapter_call<'a>(
    expected_ref: &str,
    actual_ref: &str,
    call_id: &str,
    operation: &str,
    input: &Value,
    host_operations: &'a BTreeMap<String, AdapterHostOperation>,
    handled_calls: &mut BTreeSet<String>,
) -> CwrResult<&'a AdapterHostOperation> {
    if actual_ref != expected_ref {
        return protocol_failure("adapter call invocation ref does not match the request");
    }
    if call_id.trim().is_empty() {
        return protocol_failure("adapter call ID must not be empty");
    }
    if !handled_calls.insert(call_id.to_owned()) {
        return protocol_failure(format!("adapter call ID {call_id} was already used"));
    }
    let contract = host_operations.get(operation).ok_or_else(|| {
        protocol_error(format!(
            "adapter requested undeclared host operation {operation}"
        ))
    })?;
    if contract.input_schema["x-cwr-decoder-version"] == "cwr/1"
        && matches!(operation, "cwr.invoke_action" | "cwr.start_action")
    {
        let decoded: crate::action_contract::InvokeActionInput =
            serde_json::from_value(input.clone()).map_err(|error| {
                protocol_error(format!("invalid decoded historical action input: {error}"))
            })?;
        validate_action_value("nested action input", &decoded.input)?;
        let target = serde_json::to_value(&decoded.target)?;
        let mappings = contract.input_schema["x-cwr-action-targets"]
            .as_object()
            .ok_or_else(|| {
                protocol_error("unresolved historical target: exact mappings were not retained")
            })?;
        let matches_retained = mappings.iter().any(|(activation, retained)| {
            retained == &target
                && jsonschema::is_valid(
                    &contract.input_schema,
                    &json!({"activation":activation, "input":decoded.input}),
                )
        });
        if !matches_retained {
            return protocol_failure("unresolved historical target: decoded input does not match the retained mapping and schema");
        }
        return Ok(contract);
    }
    // Gateway requests have a protocol-level response envelope for malformed
    // inputs. Let the host handler validate this one operation so adapters
    // receive that correlated 400 response instead of a transport failure.
    if operation != "cwr.send_request" {
        let validator = jsonschema::validator_for(&contract.input_schema).map_err(|error| {
            protocol_error(format!(
                "adapter host operation {operation} input schema is invalid: {error}"
            ))
        })?;
        if let Err(error) = validator.validate(input) {
            return protocol_failure(format!(
                "adapter call to {operation} does not match its input schema: {error}"
            ));
        }
    }
    if contract.input_schema["x-cwr-decoder-version"] == "cwp/1"
        && matches!(operation, "cwr.invoke_action" | "cwr.start_action")
    {
        let action_input = input
            .get("input")
            .ok_or_else(|| protocol_error("nested action call requires an input value"))?;
        validate_action_value("nested action input", action_input)?;
    } else {
        validate_action_value("adapter host operation input", input)?;
    }
    Ok(contract)
}

fn validate_adapter_call_result(operation: &AdapterHostOperation, result: &Value) -> CwrResult<()> {
    let validator = jsonschema::validator_for(&operation.output_schema).map_err(|error| {
        protocol_error(format!(
            "adapter host operation {} output schema is invalid: {error}",
            operation.name
        ))
    })?;
    if let Err(error) = validator.validate(result) {
        return protocol_failure(format!(
            "adapter host operation {} result does not match its output schema: {error}",
            operation.name
        ));
    }
    Ok(())
}

fn validate_adapter_call_result_value(
    operation: &AdapterHostOperation,
    result: &Value,
) -> CwrResult<()> {
    let decoder = &operation.input_schema["x-cwr-decoder-version"];
    if operation.name == "cwr.invoke_action"
        && matches!(decoder.as_str(), Some("cwp/1" | "cwr/1"))
    {
        return result.get("result").map_or(Ok(()), |action_result| {
            validate_action_value("nested action result", action_result)
        });
    }
    validate_action_value("adapter host operation result", result)
}

fn validate_event_header(
    expected_ref: &str,
    actual_ref: &str,
    expected_sequence: u64,
    actual_sequence: u64,
    summary: &str,
) -> CwrResult<()> {
    if actual_ref != expected_ref {
        return protocol_failure("adapter event invocation ref does not match the request");
    }
    if actual_sequence != expected_sequence {
        return protocol_failure(format!(
            "adapter event sequence must be {expected_sequence}, got {actual_sequence}"
        ));
    }
    if summary.trim().is_empty() {
        return protocol_failure("adapter event summary must not be empty");
    }
    Ok(())
}

fn validate_terminal(terminal: &AdapterTerminal, output_schema: &Value) -> CwrResult<()> {
    let status = crate::v2026_09::invocation::ActionResponseStatus::new(terminal.status)
        .map_err(|error| protocol_error(error.to_string()))?;
    if status.is_stopped() {
        return protocol_failure("an adapter result cannot create host status 499");
    }
    if status.is_progress() {
        return protocol_failure("adapter terminal status must not be 1xx");
    }
    if status.is_completed() {
        if terminal.error.is_some() {
            return protocol_failure("successful adapter result must not include an error");
        }
        let result = terminal
            .result
            .as_ref()
            .ok_or_else(|| protocol_error("successful adapter result requires a result"))?;
        validate_action_value("adapter result", result)?;
        let validator = jsonschema::validator_for(output_schema).map_err(|error| {
            protocol_error(format!("adapter output schema is invalid: {error}"))
        })?;
        if let Err(error) = validator.validate(result) {
            return protocol_failure(format!(
                "adapter result does not match the action output schema: {error}"
            ));
        }
    } else {
        debug_assert!(status.is_blocked() || status.is_failed());
        if terminal.result.is_some() {
            return protocol_failure("failed adapter result must not include a result");
        }
        let error = terminal
            .error
            .as_ref()
            .ok_or_else(|| protocol_error("failed adapter result requires an error"))?;
        if error.code.trim().is_empty() {
            return protocol_failure("adapter error code must not be empty");
        }
        if error
            .message
            .as_ref()
            .is_some_and(|message| message.trim().is_empty())
        {
            return protocol_failure("adapter error message must not be empty");
        }
    }
    Ok(())
}

struct ActionValueSizeWriter {
    bytes: usize,
}

impl Write for ActionValueSizeWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.bytes = self.bytes.saturating_add(bytes.len());
        if self.bytes > MAX_ACTION_VALUE_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "serialized action value exceeds 16 MiB",
            ));
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn validate_action_value(label: &str, value: &Value) -> CwrResult<()> {
    serde_json::to_writer(&mut ActionValueSizeWriter { bytes: 0 }, value).map_err(|error| {
        protocol_error(format!("{label} exceeds the 16 MiB serialized JSON limit: {error}"))
    })
}

fn validate_activity_header(
    invocation_ref: &str,
    event_ref: &str,
    expected_sequence: u64,
    sequence: u64,
) -> CwrResult<()> {
    if event_ref != invocation_ref {
        return protocol_failure(format!(
            "adapter activity is for invocation {event_ref}, not {invocation_ref}"
        ));
    }
    if sequence != expected_sequence {
        return protocol_failure(format!(
            "adapter activity sequence is {sequence}, expected {expected_sequence}"
        ));
    }
    Ok(())
}

fn validate_activity_event(
    expected_model_run_ref: Option<&str>,
    event: &AdapterActivityEvent,
) -> CwrResult<()> {
    let AdapterActivityEvent::CompletedCommentary { commentary } = event else {
        return Ok(());
    };
    let expected_model_run_ref = expected_model_run_ref.ok_or_else(|| {
        protocol_error("adapter emitted completed commentary without a model run")
    })?;
    if commentary.model_run_ref != expected_model_run_ref {
        return protocol_failure("adapter completed commentary has the wrong model run ref");
    }
    for (label, value) in [
        ("provider", commentary.provider.as_str()),
        (
            "response or turn ID",
            commentary.response_or_turn_id.as_str(),
        ),
        ("item ID", commentary.item_id.as_str()),
    ] {
        if value.trim().is_empty() {
            return protocol_failure(format!(
                "adapter completed commentary {label} must not be empty"
            ));
        }
    }
    if commentary.text.trim().is_empty() {
        return protocol_failure("adapter completed commentary text must not be empty");
    }
    if commentary
        .thread_id
        .as_deref()
        .is_some_and(|thread_id| thread_id.trim().is_empty())
    {
        return protocol_failure("adapter completed commentary thread ID must not be empty");
    }
    Ok(())
}
