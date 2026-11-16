use super::error;
use crate::{
    action_contract::{return_result_schema, start_action_input_schema},
    adapter::{AdapterHostOperation, AdapterInvocation},
    CwrResult,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    collections::BTreeSet,
    env, fs,
    path::{Component, Path, PathBuf},
};

const DANGER_FULL_ACCESS_WORKSPACE_ENV: &str = "CWR_CLAUDE_DANGER_FULL_ACCESS_WORKSPACE";

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Params {
    pub entrypoint: String,
    pub model: String,
    #[serde(default = "default_context_window_tokens")]
    pub context_window_tokens: usize,
    #[serde(default = "default_turns")]
    pub max_turns: u16,
    #[serde(default = "default_timeout")]
    pub timeout_seconds: u64,
}

fn default_context_window_tokens() -> usize {
    128 * 1024
}

fn default_turns() -> u16 {
    64
}
fn default_timeout() -> u64 {
    900
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Integration {
    pub claude_executable: PathBuf,
    #[serde(default)]
    pub use_cli_login: bool,
}

pub(super) struct Config {
    pub working_ref: Option<String>,
    pub working_snapshot: Value,
    pub provider_store: Option<&'static str>,
    pub params: Params,
    pub integration: Integration,
    pub prompt: String,
    pub observation: Value,
    pub return_schema: Value,
    pub provider_return_schema: Value,
    pub tools: Vec<Value>,
    pub native_tools: Vec<String>,
    pub workspace_root: PathBuf,
    pub tool_namespace: String,
    pub resume: Option<String>,
    pub native_text: bool,
}

impl Config {
    pub fn new(invocation: &mut AdapterInvocation) -> CwrResult<Self> {
        let params: Params = serde_json::from_value(crate::adapter::activated::params(
            &invocation.adapter_params,
        ))?;
        if params.model.trim().is_empty()
            || params.context_window_tokens < 16 * 1024
            || !(1..=256).contains(&params.max_turns)
            || !(1..=3600).contains(&params.timeout_seconds)
        {
            return Err(error(
                "Claude requires a model, 1–256 turns, and a 1–3600 second timeout",
            ));
        }
        let integration: Integration =
            serde_json::from_value(invocation.integration_params.clone())?;
        if !integration.claude_executable.is_absolute() {
            return Err(error("Claude executable must be an absolute path"));
        }
        let coworker = env::var_os("CWR_COWORKER_DIRECTORY")
            .ok_or_else(|| error("Claude requires CWR_COWORKER_DIRECTORY"))?;
        let prompt = match crate::adapter::activated::instructions(&invocation.adapter_params)? {
            Some(instructions) => instructions,
            None => read_entrypoint(Path::new(&coworker), &params.entrypoint)?,
        };
        let native_text = invocation.action_name == "respond_to--message";
        let provider_store = crate::adapter::provider_store_marker()?;
        let return_schema = return_result_schema(&invocation.output_schema);
        // The shared action envelope also allows progress; structured output is terminal.
        let mut return_schema = return_schema;
        return_schema["oneOf"]
            .as_array_mut()
            .expect("action envelope variants")
            .remove(0);
        // Claude's StructuredOutput tool requires an object without a root
        // union. Keep the exact envelope in the observation and validate it
        // locally; the provider schema describes its fields without that union.
        let mut provider_return_schema = return_schema["oneOf"][0].clone();
        provider_return_schema["required"] = json!(["status", "summary"]);
        provider_return_schema["properties"]["status"] =
            json!({"type":"integer","minimum":200,"maximum":599});
        provider_return_schema["properties"]["error"] =
            return_schema["oneOf"][1]["properties"]["error"].clone();
        let observation = observation(invocation, native_text, &return_schema)?;
        let workspace_root = env::current_dir()?.canonicalize()?;
        if !workspace_root.is_dir() {
            return Err(error("Claude workspace must be a canonical directory"));
        }
        let tools = host_tools(invocation)?;
        let native_tools = native_tools(&invocation.authority);
        if !native_tools.is_empty() && !danger_full_access_granted(&workspace_root) {
            return Err(error(format!(
                "Claude native repository tools require the host to set \
                 {DANGER_FULL_ACCESS_WORKSPACE_ENV} to this workspace's canonical path"
            )));
        }
        let namespaces = invocation
            .host_operations
            .iter()
            .map(crate::adapter::tools::registration)
            .collect::<CwrResult<Vec<_>>>()?
            .into_iter()
            .map(|tool| tool.namespace)
            .collect::<BTreeSet<_>>();
        if namespaces.len() > 1 {
            return Err(error("Claude cannot mix retained and new tool namespaces"));
        }
        let tool_namespace = namespaces
            .into_iter()
            .next()
            .unwrap_or_else(|| "cwp".into());
        if invocation.continuation.is_some() {
            return Err(error("Claude cannot safely restore interrupted tool calls from a session ID alone; completed effects are preserved and replay is unsupported"));
        }
        let working = &crate::adapter::working_context(&observation).unwrap_or(&observation)
            ["working_conversation"];
        let working_ref = working["ref"].as_str().map(str::to_owned);
        let resume = if let Some(state) = working.get("state") {
            if !native_text
                || state["version"] != 1
                || state["provider"] != "claude"
                || !crate::adapter::provider_store_matches(state, provider_store)
                || state["idle"] != true
                || state["working_ref"] != working["ref"]
            {
                return Err(error(
                    "Claude private history is not a settled compatible conversation",
                ));
            }
            Some(
                state["thread_id"]
                    .as_str()
                    .filter(|id| !id.trim().is_empty())
                    .ok_or_else(|| error("Claude private history has no provider session"))?
                    .to_owned(),
            )
        } else {
            None
        };
        let working_snapshot = crate::adapter::working_context(&observation)
            .unwrap_or(&observation)["context_snapshot"]
            .clone();
        Ok(Self {
            working_ref,
            working_snapshot,
            provider_store,
            params,
            integration,
            prompt,
            observation,
            return_schema,
            provider_return_schema,
            tools,
            native_tools,
            workspace_root,
            tool_namespace,
            resume,
            native_text,
        })
    }
}

fn native_tools(authority: &[String]) -> Vec<String> {
    let can_read = authority.iter().any(|grant| grant == "workspace.read");
    let can_write = authority.iter().any(|grant| grant == "workspace.write");
    let can_execute = authority.iter().any(|grant| grant == "process.execute");
    let mut tools = Vec::new();
    if can_read {
        tools.extend(["Read", "Glob", "Grep"]);
    }
    if can_write {
        tools.extend(["Edit", "Write"]);
    }
    if can_execute {
        tools.push("Bash");
    }
    tools.into_iter().map(str::to_owned).collect()
}

fn danger_full_access_granted(workspace: &Path) -> bool {
    let Some(granted_workspace) = env::var_os(DANGER_FULL_ACCESS_WORKSPACE_ENV) else {
        return false;
    };
    matches!(
        (fs::canonicalize(granted_workspace), fs::canonicalize(workspace)),
        (Ok(granted), Ok(actual)) if granted == actual
    )
}

fn read_entrypoint(directory: &Path, locator: &str) -> CwrResult<String> {
    let relative = Path::new(
        locator
            .strip_prefix("./")
            .ok_or_else(|| error("Claude entrypoint must be an explicit ./... artifact path"))?,
    );
    if relative.as_os_str().is_empty()
        || relative
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(error(
            "Claude entrypoint must stay within its coworker directory",
        ));
    }
    let path = directory.join(relative);
    if !fs::symlink_metadata(&path)?.file_type().is_file() {
        return Err(error(
            "Claude entrypoint must be a regular non-symlink file",
        ));
    }
    let path = path.canonicalize()?;
    if !path.starts_with(directory.canonicalize()?) {
        return Err(error(
            "Claude entrypoint must stay within its coworker directory",
        ));
    }
    Ok(fs::read_to_string(path)?)
}

fn observation(
    invocation: &mut AdapterInvocation,
    native_text: bool,
    schema: &Value,
) -> CwrResult<Value> {
    let mut observation = std::mem::take(&mut invocation.thread);
    let object = observation
        .as_object_mut()
        .ok_or_else(|| error("Claude thread must be an object"))?;
    object
        .entry("version")
        .or_insert_with(|| json!("cwr.thread-observation/2"));
    object.entry("messages").or_insert_with(|| json!([]));
    if !object["messages"].is_array() {
        return Err(error("Claude thread messages must be an array"));
    }
    object.remove("resumed_wait");
    object.remove("resumed_host_call");
    object.insert(
        "action_request".into(),
        json!({
            "invocation_ref": invocation.invocation_ref,
            "action": {"action_name": invocation.action_name}, "params": invocation.input,
            "return_contract": if native_text { json!({"type":"assistant_text"}) }
                else { json!({"type":"structured_output", "schema":schema}) }
        }),
    );
    Ok(observation)
}

fn host_tools(invocation: &AdapterInvocation) -> CwrResult<Vec<Value>> {
    let mut names = BTreeSet::new();
    invocation.host_operations.iter().map(|operation| {
        let projected = crate::adapter::tools::registration(operation)?;
        let name = projected.name;
        if name.len() > 64 || !names.insert(name.clone()) {
            return Err(error("Claude host tool encoding is too long or duplicated"));
        }
        Ok(json!({"name":name,"description":operation.description,"inputSchema":provider_input_schema(operation)}))
    }).collect()
}

fn provider_input_schema(operation: &AdapterHostOperation) -> Value {
    // CWR describes start_action as one object variant per visible action.
    // Claude drops MCP tools with root unions. Expose the same fields with
    // nested input choices; Session still checks the exact activation/input pair.
    if operation.name == "cwr.start_action" {
        if let Some(variants) = operation.input_schema["oneOf"].as_array() {
            let mut schema = start_action_input_schema();
            schema["properties"]["activation"] = json!({"enum": variants.iter()
                .map(|variant| &variant["properties"]["activation"]["const"])
                .collect::<Vec<_>>()});
            schema["properties"]["input"] = json!({"anyOf": variants.iter()
                .map(|variant| json!({
                    "description": format!("Input for {}.", variant["properties"]["activation"]["const"]),
                    "allOf": [variant["properties"]["input"]]
                })).collect::<Vec<_>>()});
            return schema;
        }
    }
    crate::adapter::tools::provider_schema(&operation.input_schema)
}
