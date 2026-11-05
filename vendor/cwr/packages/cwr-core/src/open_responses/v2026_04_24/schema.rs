struct Validators {
    components: HashMap<&'static str, jsonschema::Validator>,
}

static VALIDATORS: OnceLock<Result<Validators, String>> = OnceLock::new();

const COMPONENTS: &[&str] = &[
    "CreateResponseBody",
    "UserMessageItemParam",
    "SystemMessageItemParam",
    "DeveloperMessageItemParam",
    "AssistantMessageItemParam",
    "FunctionCallItemParam",
    "FunctionCallOutputItemParam",
    "Message",
    "FunctionCall",
    "OutputTextContent",
    "ResponseOutputItemAddedStreamingEvent",
    "ResponseOutputItemDoneStreamingEvent",
    "ResponseContentPartAddedStreamingEvent",
    "ResponseContentPartDoneStreamingEvent",
    "ResponseOutputTextDeltaStreamingEvent",
    "ResponseOutputTextDoneStreamingEvent",
    "ErrorStreamingEvent",
];

fn validators() -> ProfileResult<&'static Validators> {
    VALIDATORS
        .get_or_init(build_validators)
        .as_ref()
        .map_err(|message| ProfileError::new(message.clone()))
}

fn build_validators() -> Result<Validators, String> {
    let openapi: Value = serde_json::from_str(OPENAPI_JSON)
        .map_err(|error| format!("embedded Open Responses OpenAPI is invalid: {error}"))?;
    let components = openapi
        .get("components")
        .cloned()
        .ok_or_else(|| "embedded Open Responses OpenAPI has no components".to_owned())?;
    let mut validators = HashMap::new();
    for &component in COMPONENTS {
        let schema = serde_json::json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "$ref": format!("#/components/schemas/{component}"),
            "components": components.clone(),
        });
        let validator = jsonschema::options()
            .build(&schema)
            .map_err(|error| format!("cannot compile pinned {component} schema: {error}"))?;
        validators.insert(component, validator);
    }
    Ok(Validators {
        components: validators,
    })
}

fn validate_component(component: &'static str, value: &Value) -> ProfileResult<()> {
    let validator = validators()?
        .components
        .get(component)
        .ok_or_else(|| ProfileError::new(format!("unsupported pinned schema {component}")))?;
    if let Some(error) = validator.iter_errors(value).next() {
        let location = error.instance_path().as_str();
        let location = if location.is_empty() { "/" } else { location };
        return Err(ProfileError::new(format!(
            "value does not satisfy pinned {component} at {location}: {error}"
        )));
    }
    Ok(())
}

/// Validate the text-only request projection admitted by this profile.
pub fn validate_request(value: &Value) -> ProfileResult<ValidatedRequest> {
    validate_component("CreateResponseBody", value)?;
    ensure_fields(
        value,
        &["model", "input", "previous_response_id"],
        "request",
    )?;

    let model = optional_string(value, "model");
    let previous_response_id = optional_string(value, "previous_response_id");

    let mut input_item_count = 0;
    let mut has_text_input = false;
    if let Some(input) = value.get("input").filter(|input| !input.is_null()) {
        match input {
            Value::String(_) => has_text_input = true,
            Value::Array(items) => {
                input_item_count = items.len();
                for item in items {
                    let validated = validate_input_item(item)?;
                    has_text_input |= matches!(
                        validated.kind,
                        InputItemKind::UserMessage
                            | InputItemKind::SystemMessage
                            | InputItemKind::DeveloperMessage
                            | InputItemKind::AssistantMessage
                    );
                }
            }
            _ => return profile_failure("request input must be text, an item array, or null"),
        }
    }

    Ok(ValidatedRequest {
        model,
        previous_response_id,
        input_item_count,
        has_text_input,
    })
}

/// Validate one input item from the profile's exact input union.
pub fn validate_input_item(value: &Value) -> ProfileResult<ValidatedInputItem> {
    let item_type = required_string(value, "type", "input item")?;
    match item_type {
        "message" => validate_input_message(value),
        "function_call" => {
            validate_component("FunctionCallItemParam", value)?;
            ensure_fields(
                value,
                &["id", "call_id", "type", "name", "arguments", "status"],
                "function-call input item",
            )?;
            let call_id = required_nonempty_string(value, "call_id", "function-call input item")?;
            Ok(ValidatedInputItem {
                kind: InputItemKind::FunctionCall,
                item_id: optional_string(value, "id"),
                phase: None,
                call_id: Some(call_id.to_owned()),
            })
        }
        "function_call_output" => {
            validate_component("FunctionCallOutputItemParam", value)?;
            ensure_fields(
                value,
                &["id", "call_id", "type", "output", "status"],
                "function-call-output input item",
            )?;
            validate_text_function_output(value.get("output").expect("schema requires output"))?;
            let call_id =
                required_nonempty_string(value, "call_id", "function-call-output input item")?;
            Ok(ValidatedInputItem {
                kind: InputItemKind::FunctionCallOutput,
                item_id: optional_string(value, "id"),
                phase: None,
                call_id: Some(call_id.to_owned()),
            })
        }
        other => profile_failure(format!("unsupported input item type {other}")),
    }
}

fn validate_input_message(value: &Value) -> ProfileResult<ValidatedInputItem> {
    let role = required_string(value, "role", "message input item")?;
    let (component, kind, allowed_fields) = match role {
        "user" => (
            "UserMessageItemParam",
            InputItemKind::UserMessage,
            &["id", "type", "role", "content", "status"][..],
        ),
        "system" => (
            "SystemMessageItemParam",
            InputItemKind::SystemMessage,
            &["id", "type", "role", "content", "status"][..],
        ),
        "developer" => (
            "DeveloperMessageItemParam",
            InputItemKind::DeveloperMessage,
            &["id", "type", "role", "content", "status"][..],
        ),
        "assistant" => (
            "AssistantMessageItemParam",
            InputItemKind::AssistantMessage,
            &["id", "type", "role", "content", "phase", "status"][..],
        ),
        other => return profile_failure(format!("unsupported input message role {other}")),
    };
    validate_component(component, value)?;
    ensure_fields(value, allowed_fields, "message input item")?;
    validate_text_message_content(
        value.get("content").expect("schema requires content"),
        role == "assistant",
    )?;
    Ok(ValidatedInputItem {
        kind,
        item_id: optional_string(value, "id"),
        phase: {
            let phase = phase(value)?;
            if role == "assistant" && phase.is_none() {
                return profile_failure("assistant replay item requires phase");
            }
            phase
        },
        call_id: None,
    })
}

fn validate_text_message_content(content: &Value, assistant: bool) -> ProfileResult<()> {
    match content {
        Value::String(_) => Ok(()),
        Value::Array(parts) => {
            let expected = if assistant {
                "output_text"
            } else {
                "input_text"
            };
            for part in parts {
                if required_string(part, "type", "message content part")? != expected {
                    return profile_failure(format!(
                        "unsupported message content type; expected {expected}"
                    ));
                }
                let allowed = if assistant {
                    &["type", "text", "annotations"][..]
                } else {
                    &["type", "text"][..]
                };
                ensure_fields(part, allowed, "message content part")?;
            }
            Ok(())
        }
        _ => profile_failure("message content must be text or an array of text parts"),
    }
}

fn validate_text_function_output(output: &Value) -> ProfileResult<()> {
    match output {
        Value::String(_) => Ok(()),
        Value::Array(parts) => {
            for part in parts {
                if required_string(part, "type", "function output content")? != "input_text" {
                    return profile_failure("function output supports input_text content only");
                }
                ensure_fields(part, &["type", "text"], "function output content")?;
            }
            Ok(())
        }
        _ => profile_failure("function output must be text or an array of input_text parts"),
    }
}

/// Validate one output item from the profile's exact output union.
pub fn validate_output_item(value: &Value) -> ProfileResult<ValidatedOutputItem> {
    let item_type = required_string(value, "type", "output item")?;
    match item_type {
        "message" => {
            validate_component("Message", value)?;
            ensure_fields(
                value,
                &["type", "id", "status", "role", "content", "phase"],
                "assistant output item",
            )?;
            if required_string(value, "role", "assistant output item")? != "assistant" {
                return profile_failure("output message role must be assistant");
            }
            let item_id = required_string(value, "id", "assistant output item")?;
            let status = item_status(value)?;
            let content = value
                .get("content")
                .and_then(Value::as_array)
                .expect("pinned Message schema requires an array");
            let mut text_parts = Vec::with_capacity(content.len());
            for part in content {
                validate_output_text(part)?;
                text_parts.push(
                    part.get("text")
                        .and_then(Value::as_str)
                        .expect("pinned OutputTextContent requires text")
                        .to_owned(),
                );
            }
            let phase = phase(value)?;
            if phase.is_none() {
                return profile_failure("assistant output item requires phase");
            }
            Ok(ValidatedOutputItem {
                kind: OutputItemKind::AssistantMessage,
                item_id: item_id.to_owned(),
                status,
                phase,
                call_id: None,
                function_name: None,
                arguments: None,
                text_parts,
            })
        }
        "function_call" => {
            validate_component("FunctionCall", value)?;
            ensure_fields(
                value,
                &["type", "id", "call_id", "name", "arguments", "status"],
                "function-call output item",
            )?;
            let item_id = required_string(value, "id", "function-call output item")?;
            let call_id = required_string(value, "call_id", "function-call output item")?;
            Ok(ValidatedOutputItem {
                kind: OutputItemKind::FunctionCall,
                item_id: item_id.to_owned(),
                status: item_status(value)?,
                phase: None,
                call_id: Some(call_id.to_owned()),
                function_name: Some(
                    required_string(value, "name", "function-call output item")?.to_owned(),
                ),
                arguments: Some(
                    required_string(value, "arguments", "function-call output item")?.to_owned(),
                ),
                text_parts: Vec::new(),
            })
        }
        other => profile_failure(format!("unsupported output item type {other}")),
    }
}

fn validate_output_text(part: &Value) -> ProfileResult<()> {
    validate_component("OutputTextContent", part)?;
    if required_string(part, "type", "output content")? != "output_text" {
        return profile_failure("assistant output supports output_text content only");
    }
    ensure_fields(
        part,
        &["type", "text", "annotations", "logprobs"],
        "output_text content",
    )
}

/// Validate one event independently of stream ordering.
pub fn validate_event(value: &Value) -> ProfileResult<ValidatedEvent> {
    let event_type = required_string(value, "type", "event")?;
    let (kind, component, fields) = match event_type {
        "response.output_item.added" => (
            EventKind::OutputItemAdded,
            "ResponseOutputItemAddedStreamingEvent",
            &["type", "sequence_number", "output_index", "item"][..],
        ),
        "response.output_item.done" => (
            EventKind::OutputItemDone,
            "ResponseOutputItemDoneStreamingEvent",
            &["type", "sequence_number", "output_index", "item"][..],
        ),
        "response.content_part.added" => (
            EventKind::ContentPartAdded,
            "ResponseContentPartAddedStreamingEvent",
            &[
                "type",
                "sequence_number",
                "item_id",
                "output_index",
                "content_index",
                "part",
            ][..],
        ),
        "response.content_part.done" => (
            EventKind::ContentPartDone,
            "ResponseContentPartDoneStreamingEvent",
            &[
                "type",
                "sequence_number",
                "item_id",
                "output_index",
                "content_index",
                "part",
            ][..],
        ),
        "response.output_text.delta" => (
            EventKind::OutputTextDelta,
            "ResponseOutputTextDeltaStreamingEvent",
            &[
                "type",
                "sequence_number",
                "item_id",
                "output_index",
                "content_index",
                "delta",
                "logprobs",
                "obfuscation",
            ][..],
        ),
        "response.output_text.done" => (
            EventKind::OutputTextDone,
            "ResponseOutputTextDoneStreamingEvent",
            &[
                "type",
                "sequence_number",
                "item_id",
                "output_index",
                "content_index",
                "text",
                "logprobs",
            ][..],
        ),
        "error" => (
            EventKind::Error,
            "ErrorStreamingEvent",
            &["type", "sequence_number", "error"][..],
        ),
        other => return profile_failure(format!("unsupported Open Responses event {other}")),
    };
    validate_component(component, value)?;
    ensure_fields(value, fields, "event")?;

    let sequence_number = required_integer(value, "sequence_number", "event")?;
    let mut event = ValidatedEvent {
        kind,
        sequence_number,
        item_id: None,
        output_index: None,
        content_index: None,
        item_kind: None,
        item_status: None,
        phase: None,
        call_id: None,
        function_name: None,
        arguments: None,
        delta: None,
        text: None,
        text_parts: Vec::new(),
        accumulated_text: None,
        error: None,
    };

    match kind {
        EventKind::OutputItemAdded | EventKind::OutputItemDone => {
            event.output_index = Some(required_index(value, "output_index", "output-item event")?);
            let item = value
                .get("item")
                .filter(|item| !item.is_null())
                .ok_or_else(|| {
                    ProfileError::new("profile output-item event requires a non-null item")
                })?;
            let item = validate_output_item(item)?;
            event.item_id = Some(item.item_id);
            event.item_kind = Some(item.kind);
            event.item_status = Some(item.status);
            event.phase = item.phase;
            event.call_id = item.call_id;
            event.function_name = item.function_name;
            event.arguments = item.arguments;
            event.text_parts = item.text_parts.clone();
            if !item.text_parts.is_empty() {
                event.text = Some(item.text_parts.concat());
            }
        }
        EventKind::ContentPartAdded | EventKind::ContentPartDone => {
            populate_content_coordinates(&mut event, value)?;
            let part = value
                .get("part")
                .expect("pinned event schema requires part");
            validate_output_text(part)?;
            event.text = optional_string(part, "text");
        }
        EventKind::OutputTextDelta => {
            populate_content_coordinates(&mut event, value)?;
            event.delta = optional_string(value, "delta");
        }
        EventKind::OutputTextDone => {
            populate_content_coordinates(&mut event, value)?;
            event.text = optional_string(value, "text");
        }
        EventKind::Error => {
            let error = value
                .get("error")
                .expect("pinned event schema requires error");
            ensure_fields(
                error,
                &["type", "code", "message", "param", "headers"],
                "error payload",
            )?;
            let headers = error
                .get("headers")
                .and_then(Value::as_object)
                .map(|headers| {
                    headers
                        .iter()
                        .filter_map(|(name, value)| {
                            value.as_str().map(|value| (name.clone(), value.to_owned()))
                        })
                        .collect()
                })
                .unwrap_or_default();
            event.error = Some(ResponseError {
                error_type: required_string(error, "type", "error payload")?.to_owned(),
                code: optional_string(error, "code"),
                message: required_string(error, "message", "error payload")?.to_owned(),
                param: optional_string(error, "param"),
                headers,
            });
        }
    }
    Ok(event)
}

fn populate_content_coordinates(event: &mut ValidatedEvent, value: &Value) -> ProfileResult<()> {
    event.item_id = Some(required_string(value, "item_id", "content event")?.to_owned());
    event.output_index = Some(required_index(value, "output_index", "content event")?);
    event.content_index = Some(required_index(value, "content_index", "content event")?);
    Ok(())
}

fn item_status(value: &Value) -> ProfileResult<ItemStatus> {
    match required_string(value, "status", "output item")? {
        "in_progress" => Ok(ItemStatus::InProgress),
        "completed" => Ok(ItemStatus::Completed),
        "incomplete" => Ok(ItemStatus::Incomplete),
        status => profile_failure(format!("unsupported output item status {status}")),
    }
}

fn phase(value: &Value) -> ProfileResult<Option<AssistantPhase>> {
    match value.get("phase") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(phase)) if phase == "commentary" => Ok(Some(AssistantPhase::Commentary)),
        Some(Value::String(phase)) if phase == "final_answer" => {
            Ok(Some(AssistantPhase::FinalAnswer))
        }
        Some(Value::String(phase)) => {
            profile_failure(format!("unsupported assistant phase {phase}"))
        }
        Some(_) => profile_failure("assistant phase must be a string"),
    }
}

fn ensure_fields(value: &Value, allowed: &[&str], label: &str) -> ProfileResult<()> {
    let object = value
        .as_object()
        .ok_or_else(|| ProfileError::new(format!("{label} must be an object")))?;
    if let Some(field) = object
        .keys()
        .find(|field| !allowed.contains(&field.as_str()))
    {
        return profile_failure(format!("{label} contains unsupported field {field}"));
    }
    Ok(())
}

fn required_string<'a>(value: &'a Value, field: &str, label: &str) -> ProfileResult<&'a str> {
    value
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| ProfileError::new(format!("{label} requires string field {field}")))
}

fn required_nonempty_string<'a>(
    value: &'a Value,
    field: &str,
    label: &str,
) -> ProfileResult<&'a str> {
    let string = required_string(value, field, label)?;
    if string.is_empty() {
        return profile_failure(format!("{label} field {field} must not be empty"));
    }
    Ok(string)
}

fn optional_string(value: &Value, field: &str) -> Option<String> {
    value.get(field).and_then(Value::as_str).map(str::to_owned)
}

fn required_integer(value: &Value, field: &str, label: &str) -> ProfileResult<i64> {
    value
        .get(field)
        .and_then(Value::as_i64)
        .ok_or_else(|| ProfileError::new(format!("{label} requires integer field {field}")))
}

fn required_index(value: &Value, field: &str, label: &str) -> ProfileResult<i64> {
    let index = required_integer(value, field, label)?;
    if index < 0 {
        return profile_failure(format!("{label} field {field} must not be negative"));
    }
    Ok(index)
}

fn profile_failure<T>(message: impl Into<String>) -> ProfileResult<T> {
    Err(ProfileError::new(message))
}
