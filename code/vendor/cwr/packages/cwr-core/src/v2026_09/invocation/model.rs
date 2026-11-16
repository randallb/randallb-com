use std::{collections::BTreeMap, fmt};

use serde::{de::Error as _, Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

use super::control::InvocationWait;

#[derive(
    Debug, Clone, Copy, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq, PartialOrd, Ord,
)]
#[serde(rename_all = "snake_case")]
pub enum ActionInvocationState {
    Admitted,
    /// Retained person work; no process or execution slot is held.
    AwaitingPerson,
    Running,
    /// Legacy persisted suspension, decoded only to produce its upgrade outcome.
    Waiting,
    Completed,
    Blocked,
    Failed,
    /// Cancellation has been accepted; an executor or descendant is still settling.
    Stopping,
    Stopped,
}

impl ActionInvocationState {
    /// Closed to further execution and responses, including cancellation drain.
    /// `Stopping` has a cancellation decision but no settled finish time.
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Blocked | Self::Failed | Self::Stopping | Self::Stopped
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum InvocationOrigin {
    Ingress {
        integration_ref: String,
        provider_event_ref: String,
    },
    HostCall {
        caller_invocation_ref: String,
        call_id: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ActionResponseStatus(u16);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActionResponseStatusError {
    status: u16,
}

impl ActionResponseStatusError {
    pub fn status(self) -> u16 {
        self.status
    }
}

impl fmt::Display for ActionResponseStatusError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "unsupported action response status {}",
            self.status
        )
    }
}

impl std::error::Error for ActionResponseStatusError {}

impl ActionResponseStatus {
    pub const PROGRESS: Self = Self(102);
    pub const COMPLETED: Self = Self(200);
    pub const BLOCKED: Self = Self(422);
    pub const STOPPED: Self = Self(499);
    pub const FAILED: Self = Self(500);
    pub const PROVIDER_UNAVAILABLE: Self = Self(503);

    pub const fn new(status: u16) -> Result<Self, ActionResponseStatusError> {
        if matches!(status, 100..=299 | 400..=599) {
            Ok(Self(status))
        } else {
            Err(ActionResponseStatusError { status })
        }
    }

    pub const fn as_u16(self) -> u16 {
        self.0
    }

    pub const fn is_progress(self) -> bool {
        matches!(self.0, 100..=199)
    }

    pub const fn is_completed(self) -> bool {
        matches!(self.0, 200..=299)
    }

    pub const fn is_blocked(self) -> bool {
        matches!(self.0, 400..=498)
    }

    pub const fn is_stopped(self) -> bool {
        self.0 == 499
    }

    pub const fn is_failed(self) -> bool {
        matches!(self.0, 500..=599)
    }

    pub fn is_terminal(self) -> bool {
        !self.is_progress()
    }

    pub fn invocation_state(self) -> Option<ActionInvocationState> {
        if self.is_progress() {
            None
        } else if self.is_completed() {
            Some(ActionInvocationState::Completed)
        } else if self.is_stopped() {
            Some(ActionInvocationState::Stopped)
        } else if self.is_blocked() {
            Some(ActionInvocationState::Blocked)
        } else {
            debug_assert!(self.is_failed());
            Some(ActionInvocationState::Failed)
        }
    }
}

impl TryFrom<u16> for ActionResponseStatus {
    type Error = ActionResponseStatusError;

    fn try_from(status: u16) -> Result<Self, Self::Error> {
        Self::new(status)
    }
}

impl From<ActionResponseStatus> for u16 {
    fn from(status: ActionResponseStatus) -> Self {
        status.as_u16()
    }
}

impl Serialize for ActionResponseStatus {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_u16(self.0)
    }
}

impl<'de> Deserialize<'de> for ActionResponseStatus {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let status = u16::deserialize(deserializer)?;
        Self::new(status).map_err(D::Error::custom)
    }
}

impl schemars::JsonSchema for ActionResponseStatus {
    fn inline_schema() -> bool {
        true
    }

    fn schema_name() -> std::borrow::Cow<'static, str> {
        "ActionResponseStatus".into()
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "type": "integer",
            "anyOf": [
                {"minimum": 100, "maximum": 299},
                {"minimum": 400, "maximum": 599}
            ]
        })
    }
}

pub(crate) fn progress_status_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({"type": "integer", "minimum": 100, "maximum": 199})
}

pub(crate) fn completed_status_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({"type": "integer", "minimum": 200, "maximum": 299})
}

pub(crate) fn failure_status_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({
        "type": "integer",
        "anyOf": [
            {"minimum": 400, "maximum": 498},
            {"minimum": 500, "maximum": 599}
        ]
    })
}

pub(crate) fn stopped_status_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({"type": "integer", "const": 499})
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ActionResponseDetail {
    pub code: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present_string_option"
    )]
    pub message: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present_value_option"
    )]
    pub details: Option<Value>,
}

impl ActionResponseDetail {
    pub fn new(code: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: None,
            details: None,
        }
    }

    pub fn with_message(mut self, message: impl Into<String>) -> Self {
        self.message = Some(message.into());
        self
    }

    pub fn with_details(mut self, details: Value) -> Self {
        self.details = Some(details);
        self
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ActionResponse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<super::ActionProgress>,
    pub invocation_ref: String,
    pub sequence: u64,
    pub status: ActionResponseStatus,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ActionResponseDetail>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cause: Option<ActionResponseDetail>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionResponseValidationError(String);

impl fmt::Display for ActionResponseValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for ActionResponseValidationError {}

impl ActionResponse {
    pub fn validate_fields(&self) -> Result<(), ActionResponseValidationError> {
        if let Some(progress) = &self.progress {
            if !self.status.is_progress() {
                return response_validation_failure("terminal response must not contain progress");
            }
            progress.validate().map_err(ActionResponseValidationError)?;
        }
        if self.summary.trim().is_empty() {
            return response_validation_failure("action response summary must not be blank");
        }
        if self.status.is_progress() {
            if self.result.is_some() || self.error.is_some() || self.cause.is_some() {
                return response_validation_failure(
                    "progress response contains forbidden terminal details",
                );
            }
        } else if self.status.is_completed() {
            if self.result.is_none() || self.error.is_some() || self.cause.is_some() {
                return response_validation_failure("completed response has invalid details");
            }
        } else if self.status.is_stopped() {
            if self.result.is_some() || self.error.is_some() {
                return response_validation_failure("stopped response has invalid details");
            }
            if let Some(cause) = &self.cause {
                validate_response_detail(cause)?;
            }
        } else {
            debug_assert!(self.status.is_blocked() || self.status.is_failed());
            if self.result.is_some() || self.error.is_none() || self.cause.is_some() {
                return response_validation_failure("error response has invalid details");
            }
            validate_response_detail(self.error.as_ref().unwrap())?;
        }
        Ok(())
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ActionResponseWire {
    #[serde(default, deserialize_with = "deserialize_present")]
    progress: Present<super::ActionProgress>,
    invocation_ref: String,
    sequence: u64,
    status: ActionResponseStatus,
    summary: String,
    #[serde(default, deserialize_with = "deserialize_present")]
    result: Present<Value>,
    #[serde(default, deserialize_with = "deserialize_present")]
    error: Present<ActionResponseDetail>,
    #[serde(default, deserialize_with = "deserialize_present")]
    cause: Present<ActionResponseDetail>,
}

#[derive(Default)]
enum Present<T> {
    #[default]
    Missing,
    Value(T),
}

impl<T> Present<T> {
    fn into_option(self) -> Option<T> {
        match self {
            Self::Missing => None,
            Self::Value(value) => Some(value),
        }
    }
}

fn deserialize_present<'de, D, T>(deserializer: D) -> Result<Present<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Present::Value)
}

fn deserialize_present_value_option<'de, D>(deserializer: D) -> Result<Option<Value>, D::Error>
where
    D: Deserializer<'de>,
{
    Value::deserialize(deserializer).map(Some)
}

fn deserialize_present_string_option<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    String::deserialize(deserializer).map(Some)
}

impl<'de> Deserialize<'de> for ActionResponse {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let response = ActionResponseWire::deserialize(deserializer)?;
        let response = Self {
            progress: response.progress.into_option(),
            invocation_ref: response.invocation_ref,
            sequence: response.sequence,
            status: response.status,
            summary: response.summary,
            result: response.result.into_option(),
            error: response.error.into_option(),
            cause: response.cause.into_option(),
        };
        response.validate_fields().map_err(D::Error::custom)?;
        Ok(response)
    }
}

fn validate_response_detail(
    detail: &ActionResponseDetail,
) -> Result<(), ActionResponseValidationError> {
    if detail.code.trim().is_empty() {
        return response_validation_failure("action response detail code must not be blank");
    }
    if detail
        .message
        .as_ref()
        .is_some_and(|message| message.trim().is_empty())
    {
        return response_validation_failure("action response detail message must not be blank");
    }
    Ok(())
}

fn response_validation_failure<T>(
    message: impl Into<String>,
) -> Result<T, ActionResponseValidationError> {
    Err(ActionResponseValidationError(message.into()))
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct StartActionInvocation {
    pub invocation_ref: String,
    pub origin: InvocationOrigin,
    pub requester_actor_ref: String,
    pub target_actor_ref: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor_integration_ref: Option<String>,
    pub action_name: String,
    pub input: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_invocation_ref: Option<String>,
    pub admitted_at_unix_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ActionInvocationRecord {
    /// Immutable history copied into a fork; never an execution admission.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forked_from_invocation_ref: Option<String>,
    pub invocation_ref: String,
    pub origin: InvocationOrigin,
    pub requester_actor_ref: String,
    pub target_actor_ref: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor_integration_ref: Option<String>,
    pub action_name: String,
    pub input: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_invocation_ref: Option<String>,
    /// The directly addressed message that caused this invocation. Legacy
    /// records omit it until deterministic migration can establish the link.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_message_ref: Option<String>,
    /// Append-only shared-publication decisions keyed by canonical publication
    /// ref. An empty map identifies a legacy record, not an undisclosed
    /// invocation.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub timeline_publications:
        BTreeMap<String, crate::v2026_09::conversation::ConversationTimelineDisposition>,
    pub state: ActionInvocationState,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub responses: Vec<ActionResponse>,
    /// Exact conversation message published for this invocation, when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_message_ref: Option<String>,
    /// Runtime-wide admission order shared by root and delegated invocations.
    /// Zero identifies records written before this order was retained.
    #[serde(default)]
    pub admission_sequence: u64,
    pub admitted_at_unix_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at_unix_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_requested_at_unix_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_requested_by_actor_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_request_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<String>,
    /// The canceled execution owner has not yet confirmed release.
    #[serde(default)]
    pub stop_execution_pending: bool,
    /// Older stopped records did not distinguish cancellation from executor exit.
    #[serde(default)]
    pub stop_settlement_tracked: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at_unix_ms: Option<u64>,
    #[serde(default)]
    pub attempt_count: u64,
    /// Start of the current claim, independent of provider phase changes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt_started_at_unix_ms: Option<u64>,
    /// Last retained activity before this claim; later phases cannot move it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt_started_after_activity_sequence: Option<u64>,
    #[serde(default)]
    pub attempt_active: bool,
}

impl ActionInvocationRecord {
    pub fn terminal_response(&self) -> Option<&ActionResponse> {
        self.state
            .is_terminal()
            .then(|| self.responses.last())
            .flatten()
    }

    /// Copied history renders in timelines but never represents a new completion.
    #[cfg(test)]
    pub(crate) fn is_live_root_completion(&self) -> bool {
        self.forked_from_invocation_ref.is_none()
            && self.parent_invocation_ref.is_none()
            && self.state == ActionInvocationState::Completed
    }

    pub fn is_runnable(&self) -> bool {
        self.forked_from_invocation_ref.is_none()
            && !self.attempt_active
            && matches!(
                self.state,
                ActionInvocationState::Admitted | ActionInvocationState::Running
            )
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ActionInvocationLedger {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(super) invocations: crate::journal::JournalMap<String, ActionInvocationRecord>,
    #[serde(default, skip_serializing_if = "crate::journal::JournalVec::is_empty")]
    pub(super) waits: crate::journal::JournalVec<InvocationWait>,
}
