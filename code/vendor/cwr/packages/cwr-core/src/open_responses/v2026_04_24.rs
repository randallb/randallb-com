//! CWP Open Responses subset (2026-04-24).
//!
//! This module validates the standard Open Responses shapes against the pinned
//! upstream OpenAPI document first, then applies the narrower CWP profile. It
//! intentionally does not implement HTTP, SSE, WebSocket, or CWP action state.

use std::collections::{BTreeMap, HashMap};
use std::error::Error;
use std::fmt;
use std::sync::OnceLock;

use serde_json::Value;

/// Public name of this compatibility profile.
pub const PROFILE_NAME: &str = "CWP Open Responses subset (2026-04-24)";
/// Version of the pinned Open Responses specification.
pub const PROFILE_VERSION: &str = "2026-04-24";
/// Immutable upstream OpenAPI source used by this profile.
pub const UPSTREAM_OPENAPI_URL: &str =
    "https://www.openresponses.org/openapi/2026-04-24/openapi.json";
/// Git commit containing the pinned dated OpenAPI document.
pub const UPSTREAM_GIT_COMMIT: &str = "4849f32442bc380852bb4068f0b6cfff347c2baa";
/// SHA-256 of the exact checked-in upstream OpenAPI bytes.
pub const UPSTREAM_OPENAPI_SHA256: &str =
    "d598753c3a86fd8a2434828fe39dcc8786a7a694bbe46080a0d24c9fa40e72df";
/// Byte length of the exact checked-in upstream OpenAPI document.
pub const UPSTREAM_OPENAPI_BYTES: usize = 138_849;

const PROFILE_JSON: &str = include_str!(
    "../../../../packages/cwr-core/contracts/cwp/open-responses/2026-04-24/profile.json"
);
const OPENAPI_JSON: &str = include_str!(
    "../../../../packages/cwr-core/contracts/cwp/open-responses/2026-04-24/openapi.json"
);

/// The assistant-message phase defined by Open Responses 2026-04-24.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssistantPhase {
    Commentary,
    FinalAnswer,
}

impl AssistantPhase {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Commentary => "commentary",
            Self::FinalAnswer => "final_answer",
        }
    }
}

/// Input-item variants admitted by the CWP profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputItemKind {
    UserMessage,
    SystemMessage,
    DeveloperMessage,
    AssistantMessage,
    FunctionCall,
    FunctionCallOutput,
}

/// Output-item variants admitted by the CWP profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputItemKind {
    AssistantMessage,
    FunctionCall,
}

/// Standard item lifecycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemStatus {
    InProgress,
    Completed,
    Incomplete,
}

impl ItemStatus {
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Incomplete)
    }
}

/// The complete standard event union admitted by this profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    OutputItemAdded,
    OutputItemDone,
    ContentPartAdded,
    ContentPartDone,
    OutputTextDelta,
    OutputTextDone,
    Error,
}

impl EventKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::OutputItemAdded => "response.output_item.added",
            Self::OutputItemDone => "response.output_item.done",
            Self::ContentPartAdded => "response.content_part.added",
            Self::ContentPartDone => "response.content_part.done",
            Self::OutputTextDelta => "response.output_text.delta",
            Self::OutputTextDone => "response.output_text.done",
            Self::Error => "error",
        }
    }
}

/// A validated response-creation request from the supported request projection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedRequest {
    pub model: Option<String>,
    pub previous_response_id: Option<String>,
    pub input_item_count: usize,
    pub has_text_input: bool,
}

/// Metadata extracted from one validated request item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedInputItem {
    pub kind: InputItemKind,
    pub item_id: Option<String>,
    pub phase: Option<AssistantPhase>,
    pub call_id: Option<String>,
}

/// Metadata extracted from one validated model output item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedOutputItem {
    pub kind: OutputItemKind,
    pub item_id: String,
    pub status: ItemStatus,
    pub phase: Option<AssistantPhase>,
    pub call_id: Option<String>,
    pub function_name: Option<String>,
    pub arguments: Option<String>,
    pub text_parts: Vec<String>,
}

/// Structured error payload carried by the standard `error` event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResponseError {
    pub error_type: String,
    pub code: Option<String>,
    pub message: String,
    pub param: Option<String>,
    pub headers: BTreeMap<String, String>,
}

/// Standard event metadata, optionally enriched by [`StreamValidator`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedEvent {
    pub kind: EventKind,
    pub sequence_number: i64,
    pub item_id: Option<String>,
    pub output_index: Option<i64>,
    pub content_index: Option<i64>,
    pub item_kind: Option<OutputItemKind>,
    pub item_status: Option<ItemStatus>,
    pub phase: Option<AssistantPhase>,
    pub call_id: Option<String>,
    pub function_name: Option<String>,
    pub arguments: Option<String>,
    /// The fragment carried by `response.output_text.delta`.
    pub delta: Option<String>,
    /// A full text snapshot carried by a done or completed shape.
    pub text: Option<String>,
    /// Ordered assistant text parts carried by an output-item snapshot.
    pub text_parts: Vec<String>,
    /// Current repaired text after stateful correlation. Standalone validation
    /// leaves this unset.
    pub accumulated_text: Option<String>,
    pub error: Option<ResponseError>,
}

/// Validation failure from either the pinned schema or the narrower profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileError {
    message: String,
}

impl ProfileError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for ProfileError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for ProfileError {}

type ProfileResult<T> = Result<T, ProfileError>;

struct Documents {
    profile: Value,
    openapi: Value,
}

static DOCUMENTS: OnceLock<Result<Documents, String>> = OnceLock::new();

fn documents() -> ProfileResult<&'static Documents> {
    DOCUMENTS
        .get_or_init(|| {
            let profile = serde_json::from_str(PROFILE_JSON)
                .map_err(|error| format!("embedded Open Responses profile is invalid: {error}"))?;
            let openapi = serde_json::from_str(OPENAPI_JSON)
                .map_err(|error| format!("embedded Open Responses OpenAPI is invalid: {error}"))?;
            Ok(Documents { profile, openapi })
        })
        .as_ref()
        .map_err(|message| ProfileError::new(message.clone()))
}

/// Parsed checked-in CWP profile document.
pub fn profile_document() -> ProfileResult<&'static Value> {
    Ok(&documents()?.profile)
}

/// Parsed checked-in upstream OpenAPI document.
pub fn upstream_openapi_document() -> ProfileResult<&'static Value> {
    Ok(&documents()?.openapi)
}

include!("v2026_04_24/schema.rs");
include!("v2026_04_24/stream.rs");
include!("v2026_04_24/tests.rs");
