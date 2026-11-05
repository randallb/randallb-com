//! Provider-neutral action names and lifecycle contracts shared by runtimes.

use serde::{de::Error as _, Deserialize, Deserializer, Serialize};
use serde_json::{json, Value};

use crate::CwrRef;

/// Where an action performs its collaboration.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionScope {
    #[default]
    RequestingConversation,
    OwnConversation,
}

/// Whether invocation returns the final response or an admitted invocation ref.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ResponseMode {
    #[default]
    Sync,
    Async,
}

pub const INTRINSIC_CWR_COWORKER_REF: &str = "cwp://!cwr";
pub const INTRINSIC_CWR_ACTOR_REF: &str = "cwp://!cwr/actors/cwr";
pub const INTRINSIC_CWR_HOST_INTEGRATION_REF: &str = "cwp://!cwr/actor_integrations/host";
pub const INTRINSIC_CWR_INTEGRATION_REF: &str = "cwp://!cwr/integrations/runtime";
pub const INTRINSIC_CWR_ADAPTER_REF: &str = "cwp://!cwr/adapters/runtime";
pub const SEND_MESSAGE_ACTION_NAME: &str = "send--message";
pub const RESPOND_TO_MESSAGE_ACTION_NAME: &str = "respond_to--message";
pub const THREAD_OBSERVATION_VERSION: &str = "cwr.thread-observation/2";
pub const START_ACTION_HOST_OPERATION: &str = "cwr.start_action";
pub const INVOKE_ACTION_HOST_OPERATION: &str = "cwr.invoke_action";
pub const STOP_INVOCATION_HOST_OPERATION: &str = "cwr.stop_invocation";

const CWR_REF_PATTERN: &str =
    r"^(?:~@(?:/[^\s/#]+)*(?:#[^\s#]+)?|cwp://(?!~)[^\s#]+(?:#[^\s#]+)?)$";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct StartActionInput {
    #[serde(deserialize_with = "deserialize_nonblank")]
    pub activation: String,
    pub input: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct InvokeActionInput {
    pub target: ExactActionTarget,
    pub input: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

/// The target returned by discovery. Names and defaults never retarget it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ExactActionTarget {
    #[serde(deserialize_with = "deserialize_cwr_ref")]
    #[serde(alias = "loop_ref")]
    pub runtime_ref: String,
    #[serde(deserialize_with = "deserialize_cwr_ref")]
    pub coworker_ref: String,
    #[serde(deserialize_with = "deserialize_cwr_ref")]
    pub role_ref: String,
    #[serde(deserialize_with = "deserialize_nonblank")]
    pub action_name: String,
    #[serde(deserialize_with = "deserialize_cwr_ref")]
    pub binding_ref: String,
    #[serde(deserialize_with = "deserialize_nonblank")]
    pub contract_id: String,
}

pub fn exact_action_target_schema() -> Value {
    json!({
        "type": "object", "additionalProperties": false,
        "required": ["runtime_ref", "coworker_ref", "role_ref", "action_name", "binding_ref", "contract_id"],
        "properties": {
            "runtime_ref": cwr_ref_schema(), "coworker_ref": cwr_ref_schema(),
            "role_ref": cwr_ref_schema(), "binding_ref": cwr_ref_schema(),
            "action_name": nonblank_string_schema(), "contract_id": nonblank_string_schema()
        }
    })
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StartActionResultState {
    Admitted,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct StartActionResult {
    #[serde(deserialize_with = "deserialize_cwr_ref")]
    pub invocation_ref: String,
    pub state: StartActionResultState,
    #[serde(deserialize_with = "deserialize_true")]
    queued: bool,
}

impl StartActionResult {
    pub fn admitted(invocation_ref: impl Into<String>) -> Self {
        Self {
            invocation_ref: invocation_ref.into(),
            state: StartActionResultState::Admitted,
            queued: true,
        }
    }

    pub fn is_queued(&self) -> bool {
        self.queued
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct StopInvocationInput {
    #[serde(deserialize_with = "deserialize_cwr_ref")]
    pub invocation_ref: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_optional_nonblank"
    )]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopInvocationResultState {
    Stopped,
    TooLate,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum StopInvocationResult {
    Stopped {
        #[serde(deserialize_with = "deserialize_cwr_ref")]
        invocation_ref: String,
    },
    TooLate {
        #[serde(deserialize_with = "deserialize_cwr_ref")]
        invocation_ref: String,
    },
}

impl StopInvocationResult {
    pub fn invocation_ref(&self) -> &str {
        match self {
            Self::Stopped { invocation_ref } | Self::TooLate { invocation_ref } => invocation_ref,
        }
    }

    pub fn state(&self) -> StopInvocationResultState {
        match self {
            Self::Stopped { .. } => StopInvocationResultState::Stopped,
            Self::TooLate { .. } => StopInvocationResultState::TooLate,
        }
    }
}

pub fn start_action_input_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["activation", "input"],
        "properties": {
            "activation": nonblank_string_schema(),
            "input": {}
        }
    })
}

pub fn invoke_action_input_schema() -> Value {
    json!({
        "type": "object", "additionalProperties": false,
        "required": ["target", "input"],
        "properties": {
            "target": exact_action_target_schema(),
            "input": {},
            "timeout_ms": {"type":"integer", "minimum":1, "maximum":3_600_000}
        }
    })
}

pub fn invoke_action_output_schema() -> Value {
    json!({"oneOf": [terminal_action_response_schema(), start_action_output_schema()]})
}

pub fn start_action_output_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["invocation_ref", "state", "queued"],
        "properties": {
            "invocation_ref": cwr_ref_schema(),
            "state": {"const": "admitted"},
            "queued": {"const": true}
        }
    })
}

pub fn stop_invocation_input_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["invocation_ref"],
        "properties": {
            "invocation_ref": cwr_ref_schema(),
            "reason": nonblank_string_schema()
        }
    })
}

pub fn stop_invocation_output_schema() -> Value {
    json!({
        "oneOf": [
            {
                "type": "object",
                "additionalProperties": false,
                "required": ["invocation_ref", "state"],
                "properties": {
                    "invocation_ref": cwr_ref_schema(),
                    "state": {"const": "stopped"}
                }
            },
            {
                "type": "object",
                "additionalProperties": false,
                "required": ["invocation_ref", "state"],
                "properties": {
                    "invocation_ref": cwr_ref_schema(),
                    "state": {"const": "too_late"}
                }
            }
        ]
    })
}

fn terminal_action_response_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["invocation_ref", "sequence", "status", "summary"],
        "properties": {
            "invocation_ref": cwr_ref_schema(),
            "sequence": {"type": "integer", "minimum": 1},
            "status": {"type": "integer", "minimum": 200, "maximum": 599},
            "summary": nonblank_string_schema(),
            "result": {},
            "error": action_response_detail_schema(),
            "cause": action_response_detail_schema()
        },
        "oneOf": [
            {
                "properties": {
                    "status": {"type": "integer", "minimum": 200, "maximum": 299},
                    "result": {},
                    "error": false,
                    "cause": false
                },
                "required": ["result"]
            },
            {
                "properties": {
                    "status": {"type": "integer", "minimum": 400, "maximum": 498},
                    "result": false,
                    "error": action_response_detail_schema(),
                    "cause": false
                },
                "required": ["error"]
            },
            {
                "properties": {
                    "status": {"const": 499},
                    "result": false,
                    "error": false,
                    "cause": action_response_detail_schema()
                }
            },
            {
                "properties": {
                    "status": {"type": "integer", "minimum": 500, "maximum": 599},
                    "result": false,
                    "error": action_response_detail_schema(),
                    "cause": false
                },
                "required": ["error"]
            }
        ]
    })
}

fn action_response_detail_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["code"],
        "properties": {
            "code": nonblank_string_schema(),
            "message": {"type": "string", "minLength": 1, "pattern": "\\S"},
            "details": {}
        }
    })
}

fn cwr_ref_schema() -> Value {
    json!({
        "type": "string",
        "format": "cwr-ref",
        "pattern": CWR_REF_PATTERN
    })
}

fn nonblank_string_schema() -> Value {
    json!({"type": "string", "minLength": 1, "pattern": "\\S"})
}

fn deserialize_nonblank<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    if value.trim().is_empty() {
        return Err(D::Error::custom("value must not be blank"));
    }
    Ok(value)
}

fn deserialize_optional_nonblank<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<String>::deserialize(deserializer)?;
    if value.as_ref().is_some_and(|value| value.trim().is_empty()) {
        return Err(D::Error::custom("value must not be blank"));
    }
    Ok(value)
}

fn deserialize_cwr_ref<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    CwrRef::parse(&value).map_err(D::Error::custom)?;
    Ok(value)
}

fn deserialize_true<'de, D>(deserializer: D) -> Result<bool, D::Error>
where
    D: Deserializer<'de>,
{
    match bool::deserialize(deserializer)? {
        true => Ok(true),
        false => Err(D::Error::custom("value must be true")),
    }
}

pub fn return_result_schema(output_schema: &Value) -> Value {
    json!({
        "oneOf": [
            {
                "type": "object",
                "additionalProperties": false,
                "required": ["status", "summary"],
                "properties": {
                    "status": {"type": "integer", "minimum": 100, "maximum": 199},
                    "summary": nonblank_string_schema(),
                }
            },
            {
                "type": "object",
                "additionalProperties": false,
                "required": ["status", "summary", "result"],
                "properties": {
                    "status": {"type": "integer", "minimum": 200, "maximum": 299},
                    "summary": nonblank_string_schema(),
                    "result": output_schema,
                }
            },
            {
                "type": "object",
                "additionalProperties": false,
                "required": ["status", "summary", "error"],
                "properties": {
                    "status": {"type": "integer", "minimum": 400, "maximum": 498},
                    "summary": nonblank_string_schema(),
                    "error": action_response_detail_schema(),
                }
            },
            {
                "type": "object",
                "additionalProperties": false,
                "required": ["status", "summary", "error"],
                "properties": {
                    "status": {"type": "integer", "minimum": 500, "maximum": 599},
                    "summary": nonblank_string_schema(),
                    "error": action_response_detail_schema(),
                }
            }
        ]
    })
}

include!("action_contract/tests.rs");
