use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::v2026_09::conversation::{
    ConversationRecordPageRequest, ConversationSummaryPage, ConversationSummaryPageRequest,
    ConversationTimelinePage, InvocationDebugReport,
};

pub const INTRINSIC_CWR_HANDLE: &str = "@cwr";
pub const INVITE_COWORKER_ACTIVATION: &str = "@cwr--invite--coworker";
pub const LIST_COWORKERS_ACTIVATION: &str = "@cwr--list--coworkers";
pub const LIST_ACTIONS_ACTIVATION: &str = "@cwr--list--actions";
pub const SHOW_HELP_ACTIVATION: &str = "@cwr--show--help";
pub const RELOAD_GRAPH_ACTIVATION: &str = "@cwr--reload--graph";
pub const DEBUG_INVOCATION_ACTIVATION: &str = "@cwr--debug--invocation";
pub const STOP_INVOCATION_ACTIVATION: &str = "@cwr--stop--invocation";
pub const LIST_CONVERSATIONS_ACTIVATION: &str = "@cwr--list--conversations";
pub const READ_CONVERSATION_ACTIVATION: &str = "@cwr--read--conversation";
pub const BROWSE_CONVERSATION_ACTIVATION: &str = "@cwr--browse--conversation";
pub const RENAME_CONVERSATION_ACTIVATION: &str = "@cwr--rename--conversation";
pub const ARCHIVE_CONVERSATION_ACTIVATION: &str = "@cwr--archive--conversation";
pub const RESTORE_CONVERSATION_ACTIVATION: &str = "@cwr--restore--conversation";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum IntrinsicCwrCommand {
    ListCoworkers,
    ListActions,
    ShowHelp,
    ReloadGraph,
    DebugInvocation {
        invocation_ref: Option<String>,
    },
    StopInvocation {
        invocation_ref: Option<String>,
    },
    ListConversations {
        page: ConversationSummaryPageRequest,
        archived: Option<bool>,
    },
    ReadConversation {
        conversation_ref: Option<String>,
        page: ConversationRecordPageRequest,
    },
    BrowseConversation {
        request: ConversationBrowseRequest,
    },
    RenameConversation {
        title: String,
    },
    ArchiveConversation,
    RestoreConversation,
    ExportConversation,
    ImportConversation {
        path: String,
    },
    ForkConversation {
        message_ref: String,
    },
    Unknown(String),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "Intrinsic host capabilities are independent feature flags, not mutually exclusive states."
)]
pub struct IntrinsicCwrHostCapabilities {
    pub reload_graph: bool,
    pub debug_invocation: bool,
    pub stop_invocation: bool,
    pub read_conversations: bool,
    pub rename_conversation: bool,
    pub archive_conversation: bool,
    pub transfer_conversation: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct IntrinsicCwrReloadInput {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IntrinsicCwrHostAction {
    ReloadGraph,
    DebugInvocation {
        invocation_ref: Option<String>,
    },
    StopInvocation {
        invocation_ref: Option<String>,
    },
    ListConversations {
        page: ConversationSummaryPageRequest,
        archived: Option<bool>,
    },
    ReadConversation {
        conversation_ref: Option<String>,
        page: ConversationRecordPageRequest,
    },
    BrowseConversation {
        request: ConversationBrowseRequest,
    },
    RenameConversation {
        title: String,
    },
    SetConversationArchived {
        archived: bool,
    },
    ExportConversation,
    ImportConversation {
        path: String,
    },
    ForkConversation {
        message_ref: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IntrinsicCwrDispatch {
    Response(IntrinsicCwrResponse),
    HostAction(IntrinsicCwrHostAction),
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct IntrinsicCwrCatalogCoworker {
    #[serde(default, flatten)]
    pub details: BTreeMap<String, Value>,
    pub handle: String,
    pub name: String,
    pub actions: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct IntrinsicCwrCatalog {
    pub coworkers: Vec<IntrinsicCwrCatalogCoworker>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct IntrinsicCwrReloadChanges {
    pub added_coworkers: Vec<String>,
    pub removed_coworkers: Vec<String>,
    pub updated_coworkers: Vec<String>,
    pub added_actions: Vec<String>,
    pub removed_actions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ConversationListResponse {
    pub page: ConversationSummaryPage,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ConversationReadResponse {
    pub conversation_ref: String,
    pub records: ConversationReadRecords,
}

mod browse;
pub use browse::*;

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ConversationReadRecords {
    Timeline { page: ConversationTimelinePage },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum IntrinsicCwrResponse {
    Coworkers {
        coworkers: Vec<IntrinsicCwrCatalogCoworker>,
    },
    Actions {
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        targets: BTreeMap<String, crate::action_contract::ExactActionTarget>,
        actions: Vec<String>,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        input_schemas: BTreeMap<String, Value>,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        descriptions: BTreeMap<String, String>,
    },
    Help {
        actions: Vec<String>,
    },
    Unknown {
        command: String,
        actions: Vec<String>,
    },
    Unavailable {
        activation: String,
        reason: String,
    },
    Reloaded {
        changes: IntrinsicCwrReloadChanges,
    },
    ReloadFailed {
        error: String,
    },
    InvocationDebugged {
        report: Box<InvocationDebugReport>,
    },
    InvocationDebugFailed {
        invocation_ref: Option<String>,
        error: String,
    },
    InvocationStopped {
        invocation_ref: String,
        stopped_invocation_refs: Vec<String>,
    },
    InvocationStopTooLate {
        invocation_ref: String,
    },
    InvocationStopFailed {
        invocation_ref: Option<String>,
        error: String,
    },
    ConversationsListed {
        #[serde(flatten)]
        response: ConversationListResponse,
    },
    ConversationRead {
        #[serde(flatten)]
        response: ConversationReadResponse,
    },
    ConversationBrowsed {
        #[serde(flatten)]
        response: ConversationBrowseResponse,
    },
    ConversationExported {
        export: Box<crate::v2026_09::conversation::ConversationExport>,
    },
    ConversationImported {
        conversation_ref: String,
    },
    ConversationForked {
        conversation_ref: String,
        source_conversation_ref: String,
        source_message_ref: String,
    },
    ConversationRenamed {
        conversation_ref: String,
        title: String,
    },
    ConversationArchived {
        conversation_ref: String,
        archived: bool,
    },
}

pub fn parse_intrinsic_cwr_command(text: &str) -> Option<IntrinsicCwrCommand> {
    let trimmed = text.trim();
    match trimmed {
        LIST_COWORKERS_ACTIVATION => return Some(IntrinsicCwrCommand::ListCoworkers),
        LIST_ACTIONS_ACTIVATION => return Some(IntrinsicCwrCommand::ListActions),
        SHOW_HELP_ACTIVATION => return Some(IntrinsicCwrCommand::ShowHelp),
        RELOAD_GRAPH_ACTIVATION => return Some(IntrinsicCwrCommand::ReloadGraph),
        DEBUG_INVOCATION_ACTIVATION => {
            return Some(IntrinsicCwrCommand::DebugInvocation {
                invocation_ref: None,
            });
        }
        STOP_INVOCATION_ACTIVATION => {
            return Some(IntrinsicCwrCommand::StopInvocation {
                invocation_ref: None,
            });
        }
        LIST_CONVERSATIONS_ACTIVATION => {
            return Some(IntrinsicCwrCommand::ListConversations {
                page: default_conversation_summary_page(),
                archived: None,
            });
        }
        READ_CONVERSATION_ACTIVATION => {
            return Some(IntrinsicCwrCommand::ReadConversation {
                conversation_ref: None,
                page: default_conversation_record_page(),
            });
        }
        BROWSE_CONVERSATION_ACTIVATION => {
            return Some(IntrinsicCwrCommand::BrowseConversation {
                request: default_conversation_browse_request(),
            });
        }
        RENAME_CONVERSATION_ACTIVATION => {
            return Some(IntrinsicCwrCommand::RenameConversation {
                title: String::new(),
            });
        }
        ARCHIVE_CONVERSATION_ACTIVATION => return Some(IntrinsicCwrCommand::ArchiveConversation),
        RESTORE_CONVERSATION_ACTIVATION => return Some(IntrinsicCwrCommand::RestoreConversation),
        _ => {}
    }
    if let Some(remainder) = trimmed.strip_prefix(DEBUG_INVOCATION_ACTIVATION) {
        if remainder.starts_with(char::is_whitespace) {
            return Some(IntrinsicCwrCommand::DebugInvocation {
                invocation_ref: invocation_ref(remainder.trim()),
            });
        }
    }
    if let Some(remainder) = trimmed.strip_prefix(STOP_INVOCATION_ACTIVATION) {
        if remainder.starts_with(char::is_whitespace) {
            return Some(IntrinsicCwrCommand::StopInvocation {
                invocation_ref: invocation_ref(remainder.trim()),
            });
        }
    }
    if let Some(remainder) = trimmed.strip_prefix(READ_CONVERSATION_ACTIVATION) {
        if remainder.starts_with(char::is_whitespace) {
            return Some(IntrinsicCwrCommand::ReadConversation {
                conversation_ref: nonblank(remainder.trim()),
                page: default_conversation_record_page(),
            });
        }
    }
    if let Some(remainder) = trimmed.strip_prefix(RENAME_CONVERSATION_ACTIVATION) {
        if remainder.starts_with(char::is_whitespace) {
            let argument = remainder.trim();
            if argument.starts_with("--title")
                && !argument.starts_with("--title=")
                && !argument
                    .strip_prefix("--title")
                    .is_some_and(|remainder| remainder.starts_with(char::is_whitespace))
            {
                return Some(IntrinsicCwrCommand::Unknown(trimmed.into()));
            }
            return Some(IntrinsicCwrCommand::RenameConversation {
                title: parse_title_argument(argument),
            });
        }
    }
    if trimmed.starts_with("@cwr--") {
        return Some(IntrinsicCwrCommand::Unknown(trimmed.into()));
    }
    let (leading, remainder) = trimmed
        .split_once(char::is_whitespace)
        .map(|(leading, remainder)| (leading, remainder.trim()))
        .unwrap_or((trimmed, ""));
    if !leading.eq_ignore_ascii_case(INTRINSIC_CWR_HANDLE) {
        return None;
    }
    if let Some(invocation_ref) = parse_conversational_debug_invocation(remainder) {
        return Some(IntrinsicCwrCommand::DebugInvocation { invocation_ref });
    }
    if let Some(invocation_ref) = parse_conversational_stop_invocation(remainder) {
        return Some(IntrinsicCwrCommand::StopInvocation { invocation_ref });
    }
    if let Some(title) = parse_conversational_rename(remainder) {
        return Some(IntrinsicCwrCommand::RenameConversation { title });
    }
    if let Some(conversation_ref) = parse_conversational_read_conversation(remainder) {
        return Some(IntrinsicCwrCommand::ReadConversation {
            conversation_ref,
            page: default_conversation_record_page(),
        });
    }
    if remainder.eq_ignore_ascii_case("browse conversation") {
        return Some(IntrinsicCwrCommand::BrowseConversation {
            request: default_conversation_browse_request(),
        });
    }
    Some(match remainder.to_ascii_lowercase().as_str() {
        "" | "help" | "show help" => IntrinsicCwrCommand::ShowHelp,
        "coworkers" | "list coworkers" => IntrinsicCwrCommand::ListCoworkers,
        "actions" | "list actions" => IntrinsicCwrCommand::ListActions,
        "conversations" | "list conversations" => IntrinsicCwrCommand::ListConversations {
            page: default_conversation_summary_page(),
            archived: None,
        },
        "reload" | "reload graph" => IntrinsicCwrCommand::ReloadGraph,
        _ => IntrinsicCwrCommand::Unknown(remainder.into()),
    })
}

pub fn default_conversation_summary_page() -> ConversationSummaryPageRequest {
    ConversationSummaryPageRequest {
        first: Some(50),
        ..Default::default()
    }
}

pub fn default_conversation_record_page() -> ConversationRecordPageRequest {
    ConversationRecordPageRequest {
        last: Some(100),
        ..Default::default()
    }
}

pub fn default_conversation_browse_request() -> ConversationBrowseRequest {
    ConversationBrowseRequest {
        conversation_ref: None,
        operation: ConversationBrowseOperation::Recent {
            page: ConversationRecordPageRequest {
                last: Some(25),
                ..Default::default()
            },
            excerpt_bytes: default_excerpt_bytes(),
        },
    }
}

fn nonblank(value: &str) -> Option<String> {
    (!value.is_empty()).then(|| value.to_owned())
}

fn parse_title_argument(argument: &str) -> String {
    let value = argument
        .strip_prefix("--title=")
        .or_else(|| {
            argument.strip_prefix("--title").and_then(|remainder| {
                remainder
                    .starts_with(char::is_whitespace)
                    .then(|| remainder.trim_start())
            })
        })
        .unwrap_or(argument)
        .trim();
    unquote(value).to_owned()
}

fn unquote(value: &str) -> &str {
    if value.len() >= 2 {
        let bytes = value.as_bytes();
        if (bytes[0] == b'"' && bytes[value.len() - 1] == b'"')
            || (bytes[0] == b'\'' && bytes[value.len() - 1] == b'\'')
        {
            return &value[1..value.len() - 1];
        }
    }
    value
}

pub fn intrinsic_cwr_reload_succeeded(
    before: &IntrinsicCwrCatalog,
    after: &IntrinsicCwrCatalog,
) -> IntrinsicCwrResponse {
    let before_coworkers = coworkers_by_handle(before);
    let after_coworkers = coworkers_by_handle(after);
    let before_actions = catalog_actions(before);
    let after_actions = catalog_actions(after);
    IntrinsicCwrResponse::Reloaded {
        changes: IntrinsicCwrReloadChanges {
            added_coworkers: after_coworkers
                .keys()
                .filter(|handle| !before_coworkers.contains_key(*handle))
                .map(|handle| (*handle).to_owned())
                .collect(),
            removed_coworkers: before_coworkers
                .keys()
                .filter(|handle| !after_coworkers.contains_key(*handle))
                .map(|handle| (*handle).to_owned())
                .collect(),
            updated_coworkers: after_coworkers
                .iter()
                .filter_map(|(handle, coworker)| {
                    before_coworkers
                        .get(handle)
                        .filter(|before| *before != coworker)
                        .map(|_| (*handle).to_owned())
                })
                .collect(),
            added_actions: after_actions.difference(&before_actions).cloned().collect(),
            removed_actions: before_actions.difference(&after_actions).cloned().collect(),
        },
    }
}

pub fn intrinsic_cwr_reload_input_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false
    })
}

pub fn intrinsic_cwr_reload_output_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["kind", "changes"],
        "properties": {
            "kind": {"const": "reloaded"},
            "changes": {
                "type": "object",
                "additionalProperties": false,
                "required": [
                    "added_coworkers",
                    "removed_coworkers",
                    "updated_coworkers",
                    "added_actions",
                    "removed_actions"
                ],
                "properties": {
                    "added_coworkers": {"type": "array", "items": {"type": "string"}},
                    "removed_coworkers": {"type": "array", "items": {"type": "string"}},
                    "updated_coworkers": {"type": "array", "items": {"type": "string"}},
                    "added_actions": {"type": "array", "items": {"type": "string"}},
                    "removed_actions": {"type": "array", "items": {"type": "string"}}
                }
            }
        }
    })
}

pub fn intrinsic_cwr_reload_failed(error: impl Into<String>) -> IntrinsicCwrResponse {
    IntrinsicCwrResponse::ReloadFailed {
        error: error.into(),
    }
}

pub fn intrinsic_cwr_invocation_debug_succeeded(
    report: InvocationDebugReport,
) -> IntrinsicCwrResponse {
    IntrinsicCwrResponse::InvocationDebugged {
        report: Box::new(report),
    }
}

pub fn intrinsic_cwr_invocation_debug_failed(
    invocation_ref: Option<String>,
    error: impl Into<String>,
) -> IntrinsicCwrResponse {
    IntrinsicCwrResponse::InvocationDebugFailed {
        invocation_ref,
        error: error.into(),
    }
}

pub fn intrinsic_cwr_invocation_stop_succeeded(
    invocation_ref: impl Into<String>,
    stopped_invocation_refs: Vec<String>,
) -> IntrinsicCwrResponse {
    IntrinsicCwrResponse::InvocationStopped {
        invocation_ref: invocation_ref.into(),
        stopped_invocation_refs,
    }
}

pub fn intrinsic_cwr_invocation_stop_too_late(
    invocation_ref: impl Into<String>,
) -> IntrinsicCwrResponse {
    IntrinsicCwrResponse::InvocationStopTooLate {
        invocation_ref: invocation_ref.into(),
    }
}

pub fn intrinsic_cwr_invocation_stop_failed(
    invocation_ref: Option<String>,
    error: impl Into<String>,
) -> IntrinsicCwrResponse {
    IntrinsicCwrResponse::InvocationStopFailed {
        invocation_ref,
        error: error.into(),
    }
}

pub fn intrinsic_cwr_conversation_renamed(
    conversation_ref: impl Into<String>,
    title: impl Into<String>,
) -> IntrinsicCwrResponse {
    IntrinsicCwrResponse::ConversationRenamed {
        conversation_ref: conversation_ref.into(),
        title: title.into(),
    }
}

pub fn intrinsic_cwr_conversation_archived(
    conversation_ref: impl Into<String>,
    archived: bool,
) -> IntrinsicCwrResponse {
    IntrinsicCwrResponse::ConversationArchived {
        conversation_ref: conversation_ref.into(),
        archived,
    }
}

include!("intrinsic_cwr/render.rs");

include!("intrinsic_cwr/dispatch.rs");

include!("intrinsic_cwr/parsing.rs");

include!("intrinsic_cwr/tests.rs");
