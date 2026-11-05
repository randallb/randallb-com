//! Provider-neutral admission for 2026-09 conversational messages.

mod availability;
mod backend;
mod context;
pub use availability::ConversationContextError;
mod files;
pub use files::{RetainedFile, RetainedFileContent, MAX_FILE_BYTES};
mod inspection;
mod invocation;
mod read;
pub(crate) use read::HistoryProjection;
pub(crate) use read::ReadStoreLocation;

/// Metadata from a stored message that cannot be decoded by bounded browse reads.
/// The public error text deliberately omits references and message content.
#[derive(Debug)]
pub(crate) struct BrowseMessageOverage {
    pub conversation_ref: String,
    pub message_ref: String,
    pub operation: &'static str,
    pub payload_bytes: usize,
    pub limit_bytes: usize,
}

impl BrowseMessageOverage {
    pub(crate) fn in_error<'a>(error: &'a (dyn std::error::Error + 'static)) -> Option<&'a Self> {
        let mut current = Some(error);
        while let Some(cause) = current {
            if let Some(overage) = cause.downcast_ref::<Self>() {
                return Some(overage);
            }
            current = cause.source();
        }
        None
    }
}

impl std::fmt::Display for BrowseMessageOverage {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .write_str("stored message exceeds the 8 MiB browse limit; use a full archive read")
    }
}

impl std::error::Error for BrowseMessageOverage {}
mod source;
mod streaming;
mod tools;
pub use streaming::ConversationStreamingMessage;
#[cfg(test)]
mod sqlite;
mod timeline;
mod visualization;
mod working;

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use cwr_core::CwrRef;
use serde::{de::Error as _, Deserialize, Deserializer, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{
    adapter::{AdapterCall, AdapterCompletedCommentary, AdapterInvocation},
    v2026_09::{
        graph::{ResolvedCwrActorIntegration, ResolvedCwrGraph},
        invocation::{ActionInvocationLedger, ActionResponseStatus, InvocationWaitState},
    },
    CwrResult,
};

pub use crate::action_contract::{
    INTRINSIC_CWR_COWORKER_REF, RESPOND_TO_MESSAGE_ACTION_NAME, SEND_MESSAGE_ACTION_NAME,
};

const STATE_VERSION: &str = "cwp://org.coworkerprotocol/cwpdb/schemas/runtime-state/2026-09";
const EXPLICIT_TIMELINE_STATE_VERSION: &str =
    "cwp://org.coworkerprotocol/cwpdb/schemas/runtime-state/2026-09/conversation-timeline-v1";
pub const MAX_CONVERSATION_TITLE_CHARS: usize = 120;

#[derive(Debug, Clone)]
pub struct ConversationRenameMutation {
    pub(crate) conversation_ref: String,
    pub(crate) caller_actor_ref: String,
    pub(crate) title: String,
}

#[derive(Debug, Clone)]
pub struct ConversationArchiveMutation {
    pub(crate) conversation_ref: String,
    pub(crate) caller_actor_ref: String,
    pub(crate) archived: bool,
}

impl ConversationArchiveMutation {
    pub fn conversation_ref(&self) -> &str {
        &self.conversation_ref
    }

    pub fn archived(&self) -> bool {
        self.archived
    }
}

#[derive(Debug, Clone)]
pub(crate) enum ConversationIntrinsicMutation {
    Rename(ConversationRenameMutation),
    Archive(ConversationArchiveMutation),
}

impl ConversationRenameMutation {
    pub fn conversation_ref(&self) -> &str {
        &self.conversation_ref
    }

    pub fn title(&self) -> &str {
        &self.title
    }
}

pub fn normalize_conversation_title(title: &str) -> CwrResult<String> {
    let title =
        title.trim_matches(|character: char| character.is_whitespace() || character == '\u{feff}');
    if title.is_empty() {
        return invalid("conversation title must contain non-whitespace text");
    }
    if title
        .chars()
        .any(|character| character.is_control() || matches!(character, '\u{2028}' | '\u{2029}'))
    {
        return invalid("conversation title must not contain line breaks or control characters");
    }
    if title.chars().count() > MAX_CONVERSATION_TITLE_CHARS {
        return invalid(format!(
            "conversation title must be at most {MAX_CONVERSATION_TITLE_CHARS} characters"
        ));
    }
    Ok(title.to_owned())
}

mod fork;
mod terminal;
mod transfer;
pub use transfer::ConversationExport;

pub use inspection::{
    InvocationDebugIdentity, InvocationDebugLifecycle, InvocationDebugReport,
    InvocationDebugTerminal, InvocationDiagnosticEvidence, InvocationDiagnosticRecord,
    InvocationRecordExcerpt, ProviderRequestDiagnostic, ProviderRequestRateLimit,
    ProviderTurnDiagnostic,
};
pub use invocation::{
    ClaimedConversationInvocation, ConversationInvocationClaimOutcome, InvocationStopSelection,
};
#[cfg(feature = "test-read-metrics")]
pub use read::measure_path_reads;
pub use read::{
    CompletedWorkSummary, ConversationActivityTiming, ConversationActorFilter,
    ConversationActorPresentation, ConversationAttachmentFilter,
    ConversationAttachmentPresentation, ConversationBrowseEntry, ConversationBrowsePage,
    ConversationCompletionPage, ConversationContent, ConversationContentPage,
    ConversationCurrentSnapshot, ConversationExecution, ConversationExecutionOutput,
    ConversationHistoryEntry, ConversationHistoryPage, ConversationInspectorCollection,
    ConversationInspectorEntry, ConversationInspectorKey, ConversationInspectorPage,
    ConversationInspectorRequest, ConversationInspectorText, ConversationInspectorTextField,
    ConversationInvocation, ConversationInvocationOutcome, ConversationLegacyTimelinePage,
    ConversationMessageLinks, ConversationMessageMetadata, ConversationOutcomeDetail,
    ConversationPositionKey, ConversationPublication, ConversationPublicationFilter,
    ConversationPublicationPage, ConversationPublicationSnapshot, ConversationReadCancellation,
    ConversationReadOperation, ConversationReadStore, ConversationRecordPage,
    ConversationRecordPageRequest, ConversationRecordReadRequest, ConversationRecordWindow,
    ConversationResource, ConversationResourceSnapshot, ConversationSearchPage,
    ConversationSnapshotPosition, ConversationSummary, ConversationSummaryKey,
    ConversationSummaryPage, ConversationSummaryPageRequest, ConversationSummaryWindow,
    ConversationTimelineAnchor, ConversationTimelineChildRole, ConversationTimelinePage,
    ConversationTimelineSourceType, InvocationStopControl, InvocationStopDisposition,
    MAX_CONVERSATION_ACTION_EXCERPT_BYTES, MAX_CONVERSATION_BROWSE_PAGE_SIZE,
    MAX_CONVERSATION_BROWSE_RESPONSE_BYTES, MAX_CONVERSATION_EXCERPT_BYTES,
    MAX_CONVERSATION_RECORD_PAGE_SIZE, MAX_CONVERSATION_SEARCH_QUERY_CHARACTERS,
    MAX_CONVERSATION_SUMMARY_PAGE_SIZE,
};
pub use terminal::{PublishInvocationStopOutcome, PublishInvocationTerminalOutcome};
pub use timeline::{
    ConversationTimeline, ConversationTimelineConnection, ConversationTimelineConnectionOwner,
    ConversationTimelineConnectionPage, ConversationTimelineContent, ConversationTimelineCursor,
    ConversationTimelineDisposition, ConversationTimelineEdge, ConversationTimelineItem,
    ConversationTimelineMigrationOutcome, ConversationTimelineMigrationSkipReason,
    ConversationTimelinePageInfo, ConversationTimelinePageRequest,
    ConversationTimelinePublishOutcome, ConversationTimelineRollout, ConversationTimelineStore,
    CONVERSATION_TIMELINE_CURSOR_VERSION, MAX_CONVERSATION_TIMELINE_PAGE_SIZE,
};

include!("model.rs");
include!("admission_model.rs");
include!("store.rs");
include!("admission.rs");
include!("ingress_replay.rs");
include!("admission_recovery.rs");
include!("participation.rs");
include!("query.rs");
include!("message.rs");
include!("transaction.rs");
include!("person.rs");
include!("helpers.rs");

#[cfg(test)]
mod tests;

include!("provider_question.rs");
