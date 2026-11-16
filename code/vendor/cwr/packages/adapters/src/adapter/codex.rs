//! Codex app-server translation for the process adapter protocol.
pub(crate) mod app_server;
pub mod configuration;
pub(crate) mod open_responses;
pub(crate) mod runtime;
use self::{
    app_server::{
        BoundedActionConfig, BoundedActionHandler, DynamicTool, ProviderAccess, ToolCallResult,
    },
    runtime::{run_codex_action, CodexRuntime},
};
use crate::{
    action_contract::return_result_schema,
    adapter::{
        read_adapter_message, validate_adapter_invocation, write_adapter_message,
        AdapterActivityEvent, AdapterError, AdapterEvent, AdapterHostMessage, AdapterHostOperation,
        AdapterInvocation, AdapterModelOutput, PROCESS_ADAPTER_PROTOCOL_REF,
    },
    CwrResult,
};
pub use cwr::adapter::CODEX_ADAPTER_REF;
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    collections::BTreeSet,
    env, fs,
    io::{self, BufRead, Write},
    path::{Component, Path, PathBuf},
    time::Duration,
};
fn legacy_context_window_tokens() -> usize {
    128 * 1024
}
const RESPOND_TO_MESSAGE_ACTION: &str = "respond_to--message";
const DANGER_FULL_ACCESS_WORKSPACE_ENV: &str = "CWR_CODEX_DANGER_FULL_ACCESS_WORKSPACE";
const PROVIDER_INACTIVITY_TIMEOUT_ENV: &str = "CWR_CODEX_PROVIDER_INACTIVITY_TIMEOUT_MS";
const DEFAULT_PROVIDER_INACTIVITY_TIMEOUT: Duration = Duration::from_secs(240);
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CodexAdapterParams {
    entrypoint: String,
    model: Option<String>,
    reasoning_effort: Option<String>,
    #[serde(default)]
    isolated: bool,
    selective_history: Option<bool>,
    // Deprecated; retained only to recognize existing private-history digests.
    #[serde(default = "legacy_context_window_tokens")]
    context_window_tokens: usize,
}
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct CodexActorIntegrationParams {
    #[serde(default)]
    sandbox: CodexSandbox,
    #[serde(default)]
    cwr_only: bool,
    // The runtime filters host operations using this allowlist; empty denies all.
    #[serde(default, rename = "cwr_action_actor_refs")]
    _cwr_action_actor_refs: Vec<String>,
}
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum CodexSandbox {
    WorkspaceWrite,
    #[default]
    DangerFullAccess,
}
impl CodexSandbox {
    const fn as_str(&self) -> &'static str {
        match self {
            Self::WorkspaceWrite => "workspace-write",
            Self::DangerFullAccess => "danger-full-access",
        }
    }
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CodexIntegrationParams {
    codex_executable: PathBuf,
}
pub fn run_codex_adapter(mut input: impl BufRead, mut output: impl Write) -> CwrResult<()> {
    let preparation_started = std::time::Instant::now();
    let message: AdapterHostMessage = read_adapter_message(&mut input)?
        .ok_or_else(|| adapter_error("Codex adapter received no invocation"))?;
    let (protocol, mut invocation) = match message {
        AdapterHostMessage::Invoke {
            protocol,
            invocation,
        } => (protocol, invocation),
        AdapterHostMessage::CallResult { .. } => {
            return invalid("Codex adapter received a call result before invocation")
        }
        AdapterHostMessage::ActivityResult { .. } => {
            return invalid("Codex adapter received an activity result before invocation")
        }
    };
    if protocol != PROCESS_ADAPTER_PROTOCOL_REF {
        return invalid(format!(
            "Codex adapter supports {PROCESS_ADAPTER_PROTOCOL_REF}, not {protocol}"
        ));
    }
    validate_adapter_invocation(&invocation)?;
    let adapter_params: CodexAdapterParams = serde_json::from_value(
        crate::adapter::activated::params(&invocation.adapter_params),
    )
    .map_err(|error| adapter_error(format!("Codex adapter params are invalid: {error}")))?;
    if adapter_params.model.is_some() != adapter_params.reasoning_effort.is_some() {
        return invalid("Codex model and reasoning effort must be set together");
    }
    validate_codex_history_mode(&adapter_params)?;
    let workspace = env::current_dir()?;
    let actor_integration_params: CodexActorIntegrationParams =
        serde_json::from_value(invocation.actor_integration_params.clone()).map_err(|error| {
            adapter_error(format!(
                "Codex actor integration params are invalid: {error}"
            ))
        })?;
    if !adapter_params.isolated
        && !actor_integration_params.cwr_only
        && matches!(
            actor_integration_params.sandbox,
            CodexSandbox::DangerFullAccess
        )
        && !danger_full_access_granted(&workspace)
    {
        return invalid(format!(
            "Codex danger-full-access requires the host to set \
             {DANGER_FULL_ACCESS_WORKSPACE_ENV} to this workspace's canonical path"
        ));
    }
    let integration: CodexIntegrationParams =
        serde_json::from_value(invocation.integration_params.clone()).map_err(|error| {
            adapter_error(format!("Codex integration params are invalid: {error}"))
        })?;
    validate_executable_path("Codex", &integration.codex_executable)?;
    let coworker_directory = env::var_os("CWR_COWORKER_DIRECTORY")
        .map(PathBuf::from)
        .ok_or_else(|| adapter_error("Codex adapter requires CWR_COWORKER_DIRECTORY"))?;
    let developer_instructions =
        match crate::adapter::activated::instructions(&invocation.adapter_params)? {
            Some(instructions) => instructions,
            None => read_entrypoint(&coworker_directory, &adapter_params.entrypoint)?,
        };
    let native_text_result = invocation.action_name == RESPOND_TO_MESSAGE_ACTION;
    let observation = build_observation(&mut invocation, native_text_result)?;
    // The runtime may keep full history when this retained provider session
    // cannot browse, even though the binding now defaults to selective mode.
    let return_schema = return_result_schema(&invocation.output_schema);
    let host_tools = if adapter_params.isolated {
        Vec::new()
    } else {
        codex_host_tools(&invocation.host_operations)?
    };
    let home = env::var_os("HOME")
        .ok_or_else(|| adapter_error("Codex adapter requires an explicit HOME"))?;
    let user = env::var_os("USER");
    let path = env::var_os("PATH")
        .ok_or_else(|| adapter_error("Codex adapter requires an explicit PATH"))?;
    let runtime = CodexRuntime::inherited_process_group(
        &integration.codex_executable,
        home,
        if env::var_os("CWR_PROVIDER_STORE").as_deref() == Some(std::ffi::OsStr::new("shared")) {
            Some(configuration::configure()?.provider_home.into_os_string())
        } else {
            env::var_os("CODEX_HOME")
        },
        user,
        path,
        provider_inactivity_timeout()?,
    );
    let mut config = BoundedActionConfig {
        client_name: "cwr-adapter-codex".into(),
        client_title: "CWR Codex Adapter".into(),
        model: adapter_params.model,
        reasoning_effort: adapter_params.reasoning_effort,
        service_tier_file: env::var_os("CWR_CODEX_SERVICE_TIER_FILE").map(PathBuf::from),
        workspace: workspace.clone(),
        developer_instructions,
        legacy_context_window_tokens: adapter_params.context_window_tokens,
        observation,
        return_schema,
        host_tools,
        model_run_ref: invocation.model_run_ref.clone(),
        continuation: invocation.continuation.clone(),
        provider_store: crate::adapter::provider_store_marker()?,
        ephemeral: false,
        native_text_result,
        provider_access: if actor_integration_params.cwr_only {
            ProviderAccess::CwrOnly
        } else if adapter_params.isolated {
            ProviderAccess::Isolated
        } else {
            ProviderAccess::Normal
        },
        sandbox: actor_integration_params.sandbox.as_str(),
    };
    let (completed, sequence) = {
        let mut handler = AdapterActionHandler {
            input: &mut input,
            output: &mut output,
            invocation_ref: &invocation.invocation_ref,
            model_run_ref: invocation.model_run_ref.as_deref(),
            sequence: 1,
            activity_sequence: 1,
        };
        handler.on_timing(crate::adapter::AdapterTiming::single(
            crate::adapter::TimingCheckpoint::AdapterPrepared,
            crate::adapter::TimingDuration::AdapterPreparation,
            preparation_started.elapsed().as_millis() as u64,
        ));
        let completed = run_codex_action(&runtime, &mut config, &mut handler);
        (completed, handler.sequence)
    };
    let completed = match completed {
        Ok(completed) => completed,
        Err(error) => {
            let Some(failure) = app_server::CodexFailure::from_error(error.as_ref()) else {
                return Err(error);
            };
            return write_adapter_message(
                &mut output,
                &failure.result(invocation.invocation_ref, sequence),
            );
        }
    };
    let returned = completed.returned;
    let status = returned["status"]
        .as_u64()
        .and_then(|status| u16::try_from(status).ok())
        .ok_or_else(|| adapter_error("Codex action return has no valid status"))?;
    let summary = returned["summary"]
        .as_str()
        .ok_or_else(|| adapter_error("Codex action return has no summary"))?
        .to_owned();
    let result = returned.get("result").cloned();
    let error = returned
        .get("error")
        .cloned()
        .map(serde_json::from_value::<AdapterError>)
        .transpose()
        .map_err(|error| {
            adapter_error(format!("Codex action return has an invalid error: {error}"))
        })?;
    write_adapter_message(
        &mut output,
        &AdapterEvent::Result {
            invocation_ref: invocation.invocation_ref,
            sequence,
            status,
            summary,
            result,
            error,
            continuation: Some(completed.continuation),
        },
    )
}

fn provider_inactivity_timeout() -> CwrResult<Duration> {
    let Some(value) = env::var_os(PROVIDER_INACTIVITY_TIMEOUT_ENV) else {
        return Ok(DEFAULT_PROVIDER_INACTIVITY_TIMEOUT);
    };
    let value = value.to_str().ok_or_else(|| {
        adapter_error(format!(
            "{PROVIDER_INACTIVITY_TIMEOUT_ENV} must be UTF-8 milliseconds"
        ))
    })?;
    let milliseconds = value.parse::<u64>().map_err(|_| {
        adapter_error(format!(
            "{PROVIDER_INACTIVITY_TIMEOUT_ENV} must be an integer number of milliseconds"
        ))
    })?;
    if milliseconds == 0 || milliseconds > 3_600_000 {
        return invalid(format!(
            "{PROVIDER_INACTIVITY_TIMEOUT_ENV} must be between 1 and 3600000 milliseconds"
        ));
    }
    Ok(Duration::from_millis(milliseconds))
}

fn danger_full_access_granted(workspace: &Path) -> bool {
    let Some(granted_workspace) = env::var_os(DANGER_FULL_ACCESS_WORKSPACE_ENV) else {
        return false;
    };
    matches!(
        (
            fs::canonicalize(granted_workspace),
            fs::canonicalize(workspace)
        ),
        (Ok(granted), Ok(actual)) if granted == actual
    )
}
struct AdapterActionHandler<'a, I, O> {
    input: &'a mut I,
    output: &'a mut O,
    invocation_ref: &'a str,
    model_run_ref: Option<&'a str>,
    sequence: u64,
    activity_sequence: u64,
}
impl<I: BufRead, O: Write> BoundedActionHandler for AdapterActionHandler<'_, I, O> {
    fn on_timing(&mut self, timing: crate::adapter::AdapterTiming) {
        let _ = write_adapter_message(
            self.output,
            &AdapterEvent::Timing {
                invocation_ref: self.invocation_ref.into(),
                model_run_ref: self.model_run_ref.map(str::to_owned),
                timing,
            },
        );
    }
    fn on_progress(&mut self, status: u16, summary: &str) -> CwrResult<()> {
        write_adapter_message(
            self.output,
            &AdapterEvent::Progress {
                progress: None,
                published: false,
                invocation_ref: self.invocation_ref.into(),
                sequence: self.sequence,
                status,
                summary: summary.into(),
            },
        )?;
        self.sequence += 1;
        Ok(())
    }
    fn on_activity(&mut self, event: AdapterActivityEvent) -> CwrResult<()> {
        let sequence = self.activity_sequence;
        write_adapter_message(
            self.output,
            &AdapterEvent::Activity {
                invocation_ref: self.invocation_ref.into(),
                sequence,
                event,
            },
        )?;
        read_activity_result(self.input, self.invocation_ref, sequence)?;
        self.activity_sequence += 1;
        Ok(())
    }
    fn on_public_progress(&mut self, status: u16, summary: &str) -> CwrResult<()> {
        write_adapter_message(
            self.output,
            &AdapterEvent::Progress {
                progress: None,
                invocation_ref: self.invocation_ref.into(),
                sequence: self.sequence,
                status,
                summary: summary.into(),
                published: true,
            },
        )?;
        self.sequence += 1;
        Ok(())
    }
    fn on_model_output(&mut self, output: AdapterModelOutput) -> CwrResult<()> {
        write_adapter_message(
            self.output,
            &AdapterEvent::ModelOutput {
                invocation_ref: self.invocation_ref.into(),
                output,
            },
        )
    }
    fn on_host_call(
        &mut self,
        call_id: &str,
        operation: &str,
        input: Value,
    ) -> CwrResult<ToolCallResult> {
        self.call_host(call_id, operation, input)
    }
}
impl<I: BufRead, O: Write> AdapterActionHandler<'_, I, O> {
    fn call_host(
        &mut self,
        call_id: &str,
        operation: &str,
        input: Value,
    ) -> CwrResult<ToolCallResult> {
        write_adapter_message(
            self.output,
            &AdapterEvent::Call {
                invocation_ref: self.invocation_ref.into(),
                call_id: call_id.into(),
                operation: operation.into(),
                input,
            },
        )?;
        Ok(
            match read_host_call_result(self.input, self.invocation_ref, call_id)? {
                Ok(result) => ToolCallResult::success(result),
                Err(error) => ToolCallResult::failure(json!({
                    "accepted": false,
                    "error": error
                })),
            },
        )
    }
}
fn build_observation(
    invocation: &mut AdapterInvocation,
    native_text_result: bool,
) -> CwrResult<Value> {
    let mut observation = std::mem::take(&mut invocation.thread);
    let object = observation
        .as_object_mut()
        .ok_or_else(|| adapter_error("Codex invocation thread must be an object"))?;
    object
        .entry("version")
        .or_insert_with(|| json!("cwr.thread-observation/2"));
    object.entry("messages").or_insert_with(|| json!([]));
    if !object["messages"].is_array() {
        return invalid("Codex invocation thread messages must be an array");
    }
    // Resume data is host-owned. Never allow an opaque retained thread to
    // impersonate either the legacy or current resume contract.
    object.remove("resumed_wait");
    object.remove("resumed_host_call");
    let return_contract = if native_text_result {
        json!({"type": "assistant_text"})
    } else {
        json!({
            "tool": "cwp.cwr--return--result",
            "schema": return_result_schema(&invocation.output_schema)
        })
    };
    object.insert(
        "action_request".into(),
        json!({
            "invocation_ref": invocation.invocation_ref,
            "integration_ref": invocation.integration_ref,
            "action": {"action_name": invocation.action_name},
            "params": invocation.input,
            "return_contract": return_contract
        }),
    );
    Ok(observation)
}
fn codex_host_tools(operations: &[AdapterHostOperation]) -> CwrResult<Vec<DynamicTool>> {
    let mut names = BTreeSet::new();
    let mut tools = Vec::with_capacity(operations.len());
    for operation in operations {
        if operation.name == cwr::adapter::user_input::USER_INPUT_OPERATION {
            continue;
        }
        let projected = crate::adapter::tools::registration(operation)?;
        let name = projected.name;
        if !names.insert((projected.namespace.clone(), name.clone())) {
            return invalid(format!(
                "Codex host operation {} is declared more than once",
                operation.name
            ));
        }
        tools.push(DynamicTool {
            namespace: projected.namespace,
            name,
            operation: operation.name.clone(),
            description: operation.description.clone(),
            input_schema: operation.input_schema.clone(),
        });
    }
    Ok(tools)
}

#[cfg(test)]
#[path = "codex/unit_tests.rs"]
mod tests;

fn read_host_call_result(
    input: &mut impl BufRead,
    invocation_ref: &str,
    call_id: &str,
) -> CwrResult<Result<Value, Value>> {
    let response: AdapterHostMessage = read_adapter_message(input)?
        .ok_or_else(|| adapter_error("Codex adapter received no host call result"))?;
    let AdapterHostMessage::CallResult {
        invocation_ref: response_ref,
        call_id: response_call_id,
        result,
        error,
    } = response
    else {
        return invalid("Codex adapter received an invocation while awaiting a host call result");
    };
    if response_ref != invocation_ref || response_call_id != call_id {
        return invalid("Codex adapter received the wrong host call result");
    }
    match (result, error) {
        (Some(result), None) => Ok(Ok(result)),
        (None, Some(error)) => Ok(Err(json!({
            "code": error.code,
            "message": error.message,
            "details": error.details
        }))),
        _ => invalid("Codex host call result must contain exactly one of result or error"),
    }
}
fn read_activity_result(
    input: &mut impl BufRead,
    invocation_ref: &str,
    sequence: u64,
) -> CwrResult<()> {
    let response: AdapterHostMessage = read_adapter_message(input)?
        .ok_or_else(|| adapter_error("Codex adapter received no activity result"))?;
    let AdapterHostMessage::ActivityResult {
        invocation_ref: response_ref,
        sequence: response_sequence,
        error,
    } = response
    else {
        return invalid("Codex adapter received the wrong activity result message");
    };
    if response_ref != invocation_ref || response_sequence != sequence {
        return invalid("Codex adapter received the wrong activity result");
    }
    if let Some(error) = error {
        return invalid(error.message.unwrap_or(error.code));
    }
    Ok(())
}
fn read_entrypoint(coworker_directory: &Path, locator: &str) -> CwrResult<String> {
    let relative = locator.strip_prefix("./").ok_or_else(|| {
        adapter_error("Codex adapter entrypoint must be an explicit ./... artifact path")
    })?;
    let relative = Path::new(relative);
    if relative.as_os_str().is_empty()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return invalid("Codex adapter entrypoint must stay within its coworker directory");
    }
    let entrypoint = coworker_directory.join(relative);
    let metadata = fs::symlink_metadata(&entrypoint).map_err(|error| {
        adapter_error(format!(
            "Codex adapter entrypoint is unavailable at {}: {error}",
            entrypoint.display()
        ))
    })?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return invalid("Codex adapter entrypoint must be a regular non-symlink file");
    }
    let canonical_directory = coworker_directory.canonicalize()?;
    let canonical_entrypoint = entrypoint.canonicalize()?;
    if !canonical_entrypoint.starts_with(&canonical_directory) {
        return invalid("Codex adapter entrypoint must stay within its coworker directory");
    }
    Ok(fs::read_to_string(canonical_entrypoint)?)
}
fn validate_executable_path(label: &str, executable: &Path) -> CwrResult<()> {
    if !executable.is_absolute() {
        return invalid(format!(
            "Codex integration {label} executable must be an absolute path"
        ));
    }
    Ok(())
}
fn validate_codex_history_mode(params: &CodexAdapterParams) -> CwrResult<()> {
    if params.selective_history == Some(true) && params.isolated {
        return invalid(
            "selective Codex history requires host browsing tools; isolated mode disables them",
        );
    }
    Ok(())
}
fn invalid<T>(message: impl Into<String>) -> CwrResult<T> {
    Err(adapter_error(message))
}
fn adapter_error(message: impl Into<String>) -> Box<dyn std::error::Error + Send + Sync> {
    io::Error::new(io::ErrorKind::InvalidData, message.into()).into()
}
