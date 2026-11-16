use super::error;
use crate::{
    action_contract::{return_result_schema, start_action_input_schema},
    adapter::{AdapterHostOperation, AdapterInvocation},
    CwrResult,
};
use reqwest::Url;
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    path::{Component, Path},
    time::Duration,
};

const RETURN_TOOL: &str = "cwp__cwr--return--result";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Params {
    entrypoint: String,
    model: String,
    #[serde(default = "default_context_window_tokens")]
    context_window_tokens: usize,
    #[serde(default = "default_turns")]
    max_turns: u16,
    #[serde(default = "default_output_tokens")]
    max_output_tokens: u32,
    #[serde(default = "default_timeout")]
    timeout_seconds: u64,
}

fn default_context_window_tokens() -> usize {
    128 * 1024
}

const fn default_turns() -> u16 {
    16
}

const fn default_output_tokens() -> u32 {
    4096
}

const fn default_timeout() -> u64 {
    300
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Integration {
    endpoint: String,
}

pub(super) struct Config {
    pub endpoint: Url,
    pub api_key: Option<String>,
    pub model: String,
    pub max_turns: u16,
    pub max_output_tokens: u32,
    pub timeout: Duration,
    pub instructions: String,
    pub observation: Value,
    pub context_budget_bytes: usize,
    pub tools: Vec<Value>,
    pub tool_operations: BTreeMap<String, String>,
    pub return_schema: Value,
    pub native_text: bool,
}

impl Config {
    pub fn new(invocation: &mut AdapterInvocation) -> CwrResult<Self> {
        let params: Params = serde_json::from_value(crate::adapter::activated::params(
            &invocation.adapter_params,
        ))
        .map_err(|cause| {
            error(format!(
                "Open Responses adapter params are invalid: {cause}"
            ))
        })?;
        if params.model.trim().is_empty()
            || params.context_window_tokens < 16 * 1024
            || !(1..=256).contains(&params.max_turns)
            || !(16..=65_536).contains(&params.max_output_tokens)
            || !(1..=3600).contains(&params.timeout_seconds)
        {
            return Err(error(
                "Open Responses requires a model, 1–256 turns, 16–65536 output tokens, and a 1–3600 second timeout",
            ));
        }
        if invocation.continuation.is_some() {
            return Err(error(
                "Open Responses adapter does not accept provider continuation; CWR thread context is replayed",
            ));
        }
        let integration: Integration =
            serde_json::from_value(invocation.integration_params.clone()).map_err(|cause| {
                error(format!(
                    "Open Responses integration params are invalid: {cause}"
                ))
            })?;
        let endpoint = endpoint(&integration.endpoint)?;
        let coworker = env::var_os("CWR_COWORKER_DIRECTORY")
            .ok_or_else(|| error("Open Responses requires CWR_COWORKER_DIRECTORY"))?;
        let prompt = match crate::adapter::activated::instructions(&invocation.adapter_params)? {
            Some(instructions) => instructions,
            None => read_entrypoint(Path::new(&coworker), &params.entrypoint)?,
        };
        let native_text = invocation.action_name == "respond_to--message";
        let observation = observation(invocation, native_text)?;
        let (tools, tool_operations, return_schema) = tools(invocation, native_text)?;
        let completion_instruction = if native_text {
            "Finish with one assistant message containing the response for the requester."
        } else {
            "Finish by calling cwp__cwr--return--result exactly once with the terminal CWR action result. Do not print the result as ordinary assistant text."
        };
        let instructions = format!(
            "{prompt}\n\nYou are running as a Coworker Protocol coworker. The user input is a JSON thread observation and action request. Use only the function tools declared in this request. {completion_instruction}"
        );
        Ok(Self {
            endpoint,
            api_key: env::var("OPEN_RESPONSES_API_KEY")
                .ok()
                .filter(|value| !value.is_empty()),
            model: params.model,
            max_turns: params.max_turns,
            max_output_tokens: params.max_output_tokens,
            timeout: Duration::from_secs(params.timeout_seconds),
            instructions: instructions.clone(),
            context_budget_bytes: params.context_window_tokens.saturating_sub(
                instructions.len()
                    + crate::adapter::context::encoded_bytes(&json!(tools))?
                    + crate::adapter::context::encoded_bytes(&observation["action_request"])?
                    + params.max_output_tokens as usize
                    + 16 * 1024,
            ),
            observation,
            tools,
            tool_operations,
            return_schema,
            native_text,
        })
    }
}

fn endpoint(value: &str) -> CwrResult<Url> {
    let endpoint = Url::parse(value)
        .map_err(|cause| error(format!("Open Responses endpoint is invalid: {cause}")))?;
    if !endpoint.username().is_empty()
        || endpoint.password().is_some()
        || endpoint.fragment().is_some()
    {
        return Err(error(
            "Open Responses endpoint must not contain credentials or a fragment",
        ));
    }
    match endpoint.scheme() {
        "https" => {}
        "http" if endpoint.host_str().is_some_and(is_loopback_host) => {}
        _ => {
            return Err(error(
                "Open Responses endpoint must use HTTPS or loopback HTTP",
            ))
        }
    }
    Ok(endpoint)
}

fn is_loopback_host(host: &str) -> bool {
    matches!(host, "localhost" | "127.0.0.1" | "::1")
}

fn read_entrypoint(directory: &Path, locator: &str) -> CwrResult<String> {
    let relative = Path::new(
        locator
            .strip_prefix("./")
            .ok_or_else(|| error("Open Responses entrypoint must be an explicit ./... path"))?,
    );
    if relative.as_os_str().is_empty()
        || relative
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(error(
            "Open Responses entrypoint must stay within its coworker directory",
        ));
    }
    let path = directory.join(relative);
    if !fs::symlink_metadata(&path)?.file_type().is_file() {
        return Err(error(
            "Open Responses entrypoint must be a regular non-symlink file",
        ));
    }
    let path = path.canonicalize()?;
    if !path.starts_with(directory.canonicalize()?) {
        return Err(error(
            "Open Responses entrypoint must stay within its coworker directory",
        ));
    }
    Ok(fs::read_to_string(path)?)
}

fn observation(invocation: &mut AdapterInvocation, native_text: bool) -> CwrResult<Value> {
    let mut observation = std::mem::take(&mut invocation.thread);
    let object = observation
        .as_object_mut()
        .ok_or_else(|| error("Open Responses thread must be an object"))?;
    object
        .entry("version")
        .or_insert_with(|| json!("cwr.thread-observation/2"));
    object.entry("messages").or_insert_with(|| json!([]));
    if !object["messages"].is_array() {
        return Err(error("Open Responses thread messages must be an array"));
    }
    object.remove("resumed_wait");
    object.remove("resumed_host_call");
    object.insert(
        "action_request".into(),
        json!({
            "invocation_ref": invocation.invocation_ref,
            "action": {"action_name": invocation.action_name},
            "params": invocation.input,
            "return_contract": if native_text {
                json!({"type":"assistant_text"})
            } else {
                json!({"type":"function_call","name":RETURN_TOOL})
            },
        }),
    );
    Ok(observation)
}

fn tools(
    invocation: &mut AdapterInvocation,
    native_text: bool,
) -> CwrResult<(Vec<Value>, BTreeMap<String, String>, Value)> {
    let mut tools = Vec::new();
    let mut tool_operations = BTreeMap::new();
    let mut wire_names = BTreeSet::new();
    for operation in &invocation.host_operations {
        let wire_name = wire_name(operation)?;
        if !wire_names.insert(wire_name.clone()) {
            return Err(error("Open Responses host tool names collide"));
        }
        tools.push(json!({
            "type": "function",
            "name": wire_name,
            "description": operation.description,
            "parameters": provider_input_schema(operation),
            "strict": false,
        }));
        tool_operations.insert(wire_name, operation.name.clone());
    }
    let mut return_schema = return_result_schema(&invocation.output_schema);
    return_schema["oneOf"]
        .as_array_mut()
        .expect("CWR return envelope variants")
        .remove(0);
    if !native_text {
        let variants = return_schema["oneOf"]
            .as_array()
            .expect("terminal CWR return envelope variants");
        let provider_schema = json!({
            "type": "object",
            "additionalProperties": false,
            "required": ["status", "summary"],
            "properties": {
                "status": {"type":"integer","minimum":200,"maximum":599},
                "summary": variants[0]["properties"]["summary"],
                "result": invocation.output_schema,
                "error": variants[1]["properties"]["error"],
            },
        });
        tools.push(json!({
            "type": "function",
            "name": RETURN_TOOL,
            "description": "Return the terminal result of the requested CWR action.",
            "parameters": provider_schema,
            "strict": false,
        }));
    }
    Ok((tools, tool_operations, return_schema))
}

fn wire_name(operation: &AdapterHostOperation) -> CwrResult<String> {
    let projected = crate::adapter::tools::registration(operation)?;
    let name = projected.flat_name();
    if name.len() > 64 {
        return Err(error("Open Responses tool encoding exceeds 64 bytes"));
    }
    Ok(name)
}

fn provider_input_schema(operation: &AdapterHostOperation) -> Value {
    if operation.name == "cwr.start_action" {
        if let Some(variants) = operation.input_schema["oneOf"].as_array() {
            let mut schema = start_action_input_schema();
            schema["properties"]["activation"] = json!({"enum": variants
                .iter()
                .map(|variant| &variant["properties"]["activation"]["const"])
                .collect::<Vec<_>>()});
            schema["properties"]["input"] = json!({"anyOf": variants
                .iter()
                .map(|variant| variant["properties"]["input"].clone())
                .collect::<Vec<_>>()});
            return schema;
        }
    }
    crate::adapter::tools::provider_schema(&operation.input_schema)
}
