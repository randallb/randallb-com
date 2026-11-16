use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use cwr_core::CwrRef;

use super::{
    invalid_error, ConversationActivityEvent, ConversationAdmissionRecord,
    ConversationAdmissionState, ConversationAdmissionStore,
};
use crate::{
    v2026_09::invocation::{ActionInvocationRecord, ActionInvocationState, ActionResponse},
    CwrResult,
};

mod excerpt;
mod provider;
pub use excerpt::InvocationRecordExcerpt;

use provider::{
    collect_error_code, diagnostics_from_provider_event, sanitized_token, MAX_PROVIDER_DIAGNOSTICS,
};
pub use provider::{ProviderRequestDiagnostic, ProviderRequestRateLimit, ProviderTurnDiagnostic};

const MAX_ERROR_CODES: usize = 20;
const MAX_INVOCATION_SELECTOR_BYTES: usize = 2_048;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct InvocationDebugReport {
    /// Included only when the runtime explicitly grants retained-data reads to all callers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_invocation: Option<Box<InvocationRecordExcerpt>>,
    pub admission_state: ConversationAdmissionState,
    pub execution_conversation_ref: String,
    pub invocation_ref: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<Box<InvocationDebugIdentity>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lifecycle: Option<Box<InvocationDebugLifecycle>>,
    pub provider_requests: Vec<ProviderRequestDiagnostic>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub provider_turns: Vec<ProviderTurnDiagnostic>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnostics_directory: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnostic_evidence: Option<Box<InvocationDiagnosticEvidence>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal: Option<InvocationDebugTerminal>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct InvocationDebugIdentity {
    pub target_actor_ref: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adapter_ref: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct InvocationDiagnosticEvidence {
    pub availability: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub records: Vec<InvocationDiagnosticRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct InvocationDiagnosticRecord {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recorded_at_unix_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signal: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stdout_state: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stderr_tail: Option<String>,
    pub raw_log_locator: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct InvocationDebugLifecycle {
    pub admitted_at_unix_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at_unix_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_requested_at_unix_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at_unix_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_runtime_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_to_finish_latency_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct InvocationDebugTerminal {
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub error_codes: Vec<String>,
}

impl ConversationAdmissionStore {
    pub(crate) fn include_original_invocation(
        &self,
        report: &mut InvocationDebugReport,
    ) -> CwrResult<()> {
        report.original_invocation = self
            .state
            .invocation_ledger
            .invocation(&report.invocation_ref)
            .map(InvocationRecordExcerpt::new)
            .transpose()?
            .map(Box::new);
        Ok(())
    }

    pub fn inspect_invocation(&self, selector: &str) -> CwrResult<InvocationDebugReport> {
        self.inspect_invocation_if_retained(selector)?
            .ok_or_else(|| invalid_error("invocation was not found"))
    }

    pub fn inspect_invocation_if_retained(
        &self,
        selector: &str,
    ) -> CwrResult<Option<InvocationDebugReport>> {
        if let Some(invocation) = self.find_invocation_record(selector)? {
            let mut report = self.inspect_invocation_record(invocation)?;
            if let Some(adapter_ref) = self
                .state
                .admissions
                .iter()
                .find(|admission| {
                    admission.invocation_ref.as_deref() == Some(&invocation.invocation_ref)
                })
                .and_then(|admission| admission.adapter_ref.clone())
            {
                if let Some(identity) = &mut report.identity {
                    identity.adapter_ref = Some(adapter_ref);
                }
            }
            return Ok(Some(report));
        }
        let Some(admission) = self.find_invocation_admission(selector)? else {
            return Ok(None);
        };
        self.inspect_admission(admission).map(Some)
    }

    pub fn inspect_latest_invocation_for_conversation(
        &self,
        conversation_ref: &str,
    ) -> CwrResult<InvocationDebugReport> {
        let admission = self
            .state
            .admissions
            .iter()
            .rev()
            .find(|record| {
                record.conversation_ref == conversation_ref && record.invocation_ref.is_some()
            })
            .ok_or_else(|| invalid_error("this conversation has no retained invocation"))?;
        self.inspect_admission(admission)
    }

    pub fn inspect_latest_failed_invocation_for_conversation(
        &self,
        conversation_ref: &str,
    ) -> CwrResult<InvocationDebugReport> {
        let admission = self
            .state
            .admissions
            .iter()
            .rev()
            .find(|record| {
                let failed = record
                    .invocation_ref
                    .as_deref()
                    .and_then(|invocation_ref| {
                        self.state.invocation_ledger.invocation(invocation_ref)
                    })
                    .map(|invocation| invocation.state == ActionInvocationState::Failed)
                    .unwrap_or(record.state == ConversationAdmissionState::Failed);
                record.conversation_ref == conversation_ref
                    && record.invocation_ref.is_some()
                    && failed
            })
            .ok_or_else(|| invalid_error("this conversation has no retained failed invocation"))?;
        self.inspect_admission(admission)
    }

    fn find_invocation_admission(
        &self,
        selector: &str,
    ) -> CwrResult<Option<&ConversationAdmissionRecord>> {
        let (selector, full_ref) = validate_invocation_selector(selector)?;
        let matches = self
            .state
            .admissions
            .iter()
            .filter(|record| {
                record
                    .invocation_ref
                    .as_deref()
                    .is_some_and(|reference| invocation_matches(reference, selector, full_ref))
            })
            .collect::<Vec<_>>();
        match matches.as_slice() {
            [] => Ok(None),
            [admission] => Ok(Some(*admission)),
            _ => Err(invalid_error(format!(
                "invocation ID is ambiguous across {} retained records; use the full invocation ref",
                matches.len()
            ))),
        }
    }

    fn find_invocation_record(&self, selector: &str) -> CwrResult<Option<&ActionInvocationRecord>> {
        let (selector, full_ref) = validate_invocation_selector(selector)?;
        let matches = self
            .state
            .invocation_ledger
            .invocations()
            .filter(|record| invocation_matches(&record.invocation_ref, selector, full_ref))
            .collect::<Vec<_>>();
        match matches.as_slice() {
            [] => Ok(None),
            [invocation] => Ok(Some(*invocation)),
            _ => Err(invalid_error(format!(
                "invocation ID is ambiguous across {} retained records; use the full invocation ref",
                matches.len()
            ))),
        }
    }

    fn inspect_admission(
        &self,
        admission: &ConversationAdmissionRecord,
    ) -> CwrResult<InvocationDebugReport> {
        let invocation_ref = admission
            .invocation_ref
            .as_deref()
            .ok_or_else(|| invalid_error("admission has no invocation ref"))?;
        let execution_conversation_ref = self
            .execution_conversation_ref(invocation_ref)
            .ok_or_else(|| invalid_error("invocation has no execution conversation"))?;
        let invocation = self.state.invocation_ledger.invocation(invocation_ref);
        let terminal_response = invocation.and_then(ActionInvocationRecord::terminal_response);
        let admission_state = invocation
            .map(|invocation| invocation_admission_state(invocation.state))
            .unwrap_or(admission.state);
        let mut report = self.inspect_retained_invocation(
            invocation_ref,
            execution_conversation_ref,
            admission_state,
            invocation,
            terminal_response,
        )?;
        if report.identity.is_none() {
            report.identity = admission.target_actor_ref.as_ref().map(|target_actor_ref| {
                Box::new(InvocationDebugIdentity {
                    target_actor_ref: target_actor_ref.clone(),
                    action_name: None,
                    adapter_ref: admission.adapter_ref.clone(),
                })
            });
        } else if let Some(identity) = &mut report.identity {
            identity.adapter_ref = admission.adapter_ref.clone();
        }
        Ok(report)
    }

    fn inspect_invocation_record(
        &self,
        invocation: &ActionInvocationRecord,
    ) -> CwrResult<InvocationDebugReport> {
        let execution_conversation_ref = self
            .execution_conversation_ref(&invocation.invocation_ref)
            .ok_or_else(|| invalid_error("invocation has no execution conversation"))?;
        let admission_state = invocation_admission_state(invocation.state);
        self.inspect_retained_invocation(
            &invocation.invocation_ref,
            execution_conversation_ref,
            admission_state,
            Some(invocation),
            invocation.terminal_response(),
        )
    }

    fn inspect_retained_invocation(
        &self,
        invocation_ref: &str,
        execution_conversation_ref: &str,
        admission_state: ConversationAdmissionState,
        invocation: Option<&ActionInvocationRecord>,
        terminal_response: Option<&ActionResponse>,
    ) -> CwrResult<InvocationDebugReport> {
        let conversation = self
            .full_conversation(execution_conversation_ref)?
            .ok_or_else(|| invalid_error("invocation execution conversation is unavailable"))?;
        let mut provider_requests = Vec::new();
        let mut provider_turns = Vec::new();
        let mut terminal = None;
        let mut error_codes = BTreeSet::new();
        if let Some(response) = terminal_response {
            for detail in [response.error.as_ref(), response.cause.as_ref()]
                .into_iter()
                .flatten()
            {
                if let Some(code) = sanitized_token(&detail.code, 200) {
                    error_codes.insert(code);
                }
            }
        }
        for activity in &conversation.activity {
            match &activity.event {
                ConversationActivityEvent::ProviderEvent { method, data, .. } => {
                    if matches!(method.as_str(), "turn/started" | "turn/completed") {
                        provider_turns.push(ProviderTurnDiagnostic {
                            event: method.clone(),
                            sequence: activity.sequence,
                            occurred_at_unix_ms: activity.occurred_at_unix_ms,
                            thread_id: data
                                .get("thread_id")
                                .and_then(Value::as_str)
                                .map(str::to_owned),
                            turn_id: data
                                .pointer("/turn/id")
                                .and_then(Value::as_str)
                                .map(str::to_owned),
                            scope: data.get("scope").and_then(Value::as_str).map(str::to_owned),
                        });
                        let overflow = provider_turns
                            .len()
                            .saturating_sub(MAX_PROVIDER_DIAGNOSTICS);
                        provider_turns.drain(..overflow);
                    }
                    // Legacy state has no canonical terminal response, so its
                    // scoped provider return remains the only available code.
                    // Once a ledger response exists, it is authoritative: an
                    // earlier rejected provider attempt must not be reported as
                    // the invocation's terminal reason.
                    if terminal_response.is_none() {
                        collect_error_code(data, &mut error_codes);
                    }
                    if method == "item/completed" {
                        provider_requests.extend(diagnostics_from_provider_event(
                            data,
                            activity.sequence,
                            activity.occurred_at_unix_ms,
                        ));
                        let overflow = provider_requests
                            .len()
                            .saturating_sub(MAX_PROVIDER_DIAGNOSTICS);
                        provider_requests.drain(..overflow);
                    }
                }
                ConversationActivityEvent::TurnCompleted { status, .. } => {
                    terminal = Some(InvocationDebugTerminal {
                        kind: "turn_completed".into(),
                        status: Some(status.as_u16()),
                        error_codes: Vec::new(),
                    });
                }
                ConversationActivityEvent::TurnFailed { status, .. } => {
                    terminal = Some(InvocationDebugTerminal {
                        kind: "turn_failed".into(),
                        status: Some(status.as_u16()),
                        error_codes: Vec::new(),
                    });
                }
                ConversationActivityEvent::TurnStopped { status, .. } => {
                    terminal = Some(InvocationDebugTerminal {
                        kind: "turn_stopped".into(),
                        status: Some(status.as_u16()),
                        error_codes: Vec::new(),
                    });
                }
                _ => {}
            }
        }
        if let Some(terminal) = &mut terminal {
            terminal.error_codes = error_codes.into_iter().take(MAX_ERROR_CODES).collect();
        }
        Ok(InvocationDebugReport {
            original_invocation: None,
            admission_state,
            execution_conversation_ref: execution_conversation_ref.into(),
            invocation_ref: invocation_ref.into(),
            identity: invocation.map(|record| {
                Box::new(InvocationDebugIdentity {
                    target_actor_ref: record.target_actor_ref.clone(),
                    action_name: Some(record.action_name.clone()),
                    adapter_ref: None,
                })
            }),
            lifecycle: invocation.map(invocation_debug_lifecycle).map(Box::new),
            provider_requests,
            provider_turns,
            diagnostics_directory: None,
            diagnostic_evidence: None,
            terminal,
        })
    }
}

fn invocation_debug_lifecycle(invocation: &ActionInvocationRecord) -> InvocationDebugLifecycle {
    InvocationDebugLifecycle {
        admitted_at_unix_ms: invocation.admitted_at_unix_ms,
        started_at_unix_ms: invocation.started_at_unix_ms,
        stop_requested_at_unix_ms: invocation.stop_requested_at_unix_ms,
        finished_at_unix_ms: invocation.finished_at_unix_ms,
        total_runtime_ms: invocation.started_at_unix_ms.and_then(|started| {
            invocation
                .finished_at_unix_ms
                .and_then(|finished| finished.checked_sub(started))
        }),
        stop_to_finish_latency_ms: invocation.stop_requested_at_unix_ms.and_then(|stopped| {
            invocation
                .finished_at_unix_ms
                .and_then(|finished| finished.checked_sub(stopped))
        }),
    }
}

pub(super) fn validate_invocation_selector(selector: &str) -> CwrResult<(&str, bool)> {
    let selector = selector.trim();
    if selector.is_empty() {
        return Err(invalid_error("invocation ref or ID is required"));
    }
    if selector.len() > MAX_INVOCATION_SELECTOR_BYTES {
        return Err(invalid_error("invocation ref or ID is too long"));
    }
    let full_ref = selector.starts_with("cwp://");
    if full_ref {
        let reference =
            CwrRef::parse(selector).map_err(|_| invalid_error("invalid invocation ref"))?;
        let segments = reference.segments();
        if reference.fragment().is_some()
            || !matches!(segments, [.., collection, _] if collection == "invocations")
        {
            return Err(invalid_error("ref does not identify an invocation"));
        }
    }
    if !full_ref
        && !selector
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "._-".contains(character))
    {
        return Err(invalid_error(
            "invocation ID contains unsupported characters",
        ));
    }
    Ok((selector, full_ref))
}

fn invocation_admission_state(state: ActionInvocationState) -> ConversationAdmissionState {
    match state {
        ActionInvocationState::AwaitingPerson
        | ActionInvocationState::Admitted
        | ActionInvocationState::Running
        | ActionInvocationState::Waiting => ConversationAdmissionState::Admitted,
        ActionInvocationState::Completed => ConversationAdmissionState::Completed,
        ActionInvocationState::Blocked | ActionInvocationState::Failed => {
            ConversationAdmissionState::Failed
        }
        ActionInvocationState::Stopping | ActionInvocationState::Stopped => {
            ConversationAdmissionState::Stopped
        }
    }
}

pub(super) fn invocation_matches(reference: &str, selector: &str, full_ref: bool) -> bool {
    if full_ref {
        return reference == selector;
    }
    let Ok(reference) = CwrRef::parse(reference) else {
        return false;
    };
    let segments = reference.segments();
    matches!(segments, [.., collection, id] if collection == "invocations" && id == selector)
}

include!("inspection/tests.rs");
