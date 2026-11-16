//! Local terminal hosting for the 2026-09 graph, conversation, and invocation model.

use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet, VecDeque},
    fs::{self, File, OpenOptions},
    io,
    num::NonZeroUsize,
    os::unix::{
        fs::{OpenOptionsExt, PermissionsExt},
        io::AsRawFd,
    },
    path::{Path, PathBuf},
    rc::Rc,
    sync::{Arc, Condvar, Mutex, MutexGuard, TryLockError},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Number, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{
    action_contract::{
        invoke_action_input_schema, invoke_action_output_schema, stop_invocation_input_schema,
        stop_invocation_output_schema, ExactActionTarget, InvokeActionInput, StartActionInput,
        StartActionResult, StopInvocationInput, StopInvocationResult, INTRINSIC_CWR_ACTOR_REF,
        INTRINSIC_CWR_ADAPTER_REF, INTRINSIC_CWR_COWORKER_REF, INTRINSIC_CWR_HOST_INTEGRATION_REF,
        INTRINSIC_CWR_INTEGRATION_REF, INVOKE_ACTION_HOST_OPERATION,
        RESPOND_TO_MESSAGE_ACTION_NAME, START_ACTION_HOST_OPERATION,
        STOP_INVOCATION_HOST_OPERATION,
    },
    adapter::{
        AdapterActivityEvent, AdapterCall, AdapterError, AdapterHostCallOutcome,
        AdapterHostOperation, AdapterInvocation, AdapterModelOutputMailbox, AdapterReadiness,
        AdapterTerminal, ProcessAdapterOutcome, ProcessAdapterStopControl,
    },
    intrinsic_cwr::{
        default_conversation_record_page, default_conversation_summary_page,
        execute_intrinsic_cwr_command, intrinsic_cwr_conversation_archived,
        intrinsic_cwr_conversation_renamed, intrinsic_cwr_invocation_debug_failed,
        intrinsic_cwr_invocation_debug_succeeded, intrinsic_cwr_invocation_stop_failed,
        intrinsic_cwr_invocation_stop_succeeded, intrinsic_cwr_invocation_stop_too_late,
        intrinsic_cwr_reload_failed, intrinsic_cwr_reload_succeeded, parse_intrinsic_cwr_command,
        render_intrinsic_cwr_response, ConversationListResponse, ConversationReadRecords,
        ConversationReadResponse, IntrinsicCwrCatalog, IntrinsicCwrCatalogCoworker,
        IntrinsicCwrCommand, IntrinsicCwrDispatch, IntrinsicCwrHostAction,
        IntrinsicCwrHostCapabilities, IntrinsicCwrResponse,
    },
    v2026_09::{
        conversation::{
            ConversationActivity, ConversationActivityEvent, ConversationAdmissionOutcome,
            ConversationAdmissionRecord, ConversationAdmissionStore, ConversationArchiveMutation,
            ConversationContextError, ConversationHostActionCall, ConversationIngress,
            ConversationIngressAddressing, ConversationIngressConversation,
            ConversationIntrinsicMutation, ConversationInvocationClaimOutcome, ConversationMessage,
            ConversationReadCancellation, ConversationReadStore, ConversationRecordPage,
            ConversationRecordPageRequest, ConversationRecordReadRequest,
            ConversationRenameMutation, ConversationSummaryPage, ConversationSummaryPageRequest,
            ConversationTimelinePage, ConversationTimelineRollout,
            ConversationVisualizationArtifact, InvocationStopSelection,
            PublishInvocationStopOutcome, PublishInvocationTerminalOutcome, RetainedConversation,
        },
        graph::{
            ResolvedCwrActionRoute, ResolvedCwrActor, ResolvedCwrActorIntegration,
            ResolvedCwrGraph, ResolvedCwrResource, ResolvedCwrVisibility,
        },
        invocation::{
            ActionInvocationRecord, ActionInvocationState, ActionResponse, ActionResponseDetail,
            ActionResponseStatus, InvocationOrigin, InvocationStopRequest, InvocationTerminal,
            StartActionInvocation, StartInvocationOutcome,
        },
    },
    CwrRef, CwrResult,
};

pub use super::execution::route_action_target;
use super::execution::{retain_native_tool_definitions, selected_process_route};

const MAX_ACTION_DEPTH: usize = 16;

pub mod connections;
pub mod experimental_features;
pub mod imported_volumes;

/// Resolve an ordinary local workspace with its saved inputs and folder grants.
pub fn resolve_local_graph(path: &Path) -> CwrResult<ResolvedCwrGraph> {
    connections::resolve_local_graph(path)
}

/// Resolve saved input selections and imported folder grants for local checks.
pub fn local_graph_options(path: &Path) -> CwrResult<crate::v2026_09::graph::GraphLoadOptions> {
    connections::graph_options(path)
}

/// Reject workspaces that still contain the retired shared-folder marker.
///
/// The marker is left in place so opening an obsolete copy fails visibly. It
/// must be removed explicitly after the workspace has been moved back to its
/// authoritative runtime database.
pub fn reject_retired_shared_folder(workspace: &Path) -> CwrResult<()> {
    let workspace = fs::canonicalize(workspace)?;
    let workspace_directory = if workspace.is_dir() {
        workspace.as_path()
    } else {
        workspace
            .parent()
            .ok_or_else(|| invalid_error("CWR workspace manifest has no containing directory"))?
    };
    let marker = workspace_directory.join(".cwr/shared-folder.json");
    if marker.try_exists()? {
        return invalid(
            "shared-folder workspaces are no longer supported; remove .cwr/shared-folder.json only after restoring this workspace to its authoritative runtime database",
        );
    }
    Ok(())
}

include!("local/model.rs");
include!("local/upstream.rs");
include!("local/gateway_host.rs");
include!("local/delegation.rs");
include!("local/workspace.rs");
include!("local/resident.rs");
include!("local/runtime.rs");
include!("local/gateway_inspection.rs");
include!("local/cloud_execution.rs");
include!("local/admission.rs");
include!("local/owned_action.rs");
include!("local/schedules.rs");
include!("local/ingress_admission.rs");
include!("local/text_admission.rs");
include!("local/action_only.rs");
include!("local/execution.rs");
include!("local/scheduling.rs");
include!("local/execution_activity.rs");
include!("local/execution_metrics.rs");
include!("local/stop.rs");
include!("local/archive.rs");
include!("local/archive_order.rs");
include!("local/shutdown.rs");
include!("local/stop_request.rs");
include!("local/lifecycle.rs");
include!("local/files.rs");
include!("local/lifecycle_stop.rs");
include!("local/messaging.rs");
include!("local/conversation_creation.rs");
include!("local/context_discovery.rs");
include!("local/gateway.rs");
include!("local/participation.rs");
include!("local/transfer.rs");
include!("local/terminal.rs");
include!("local/catalog.rs");
include!("local/intrinsic.rs");
include!("local/arguments.rs");
include!("local/routes.rs");
include!("local/retained.rs");
include!("local/read.rs");
include!("local/conversation_module.rs");
include!("local/bindings.rs");
include!("local/state.rs");

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v2026_09::conversation::{ConversationAddressingMode, ConversationAdmissionState};
    use crate::v2026_09::host::ConversationWorkerPhase;

    pub(super) static SH_ADAPTER_ENV_LOCK: Mutex<()> = Mutex::new(());

    pub(super) fn fixture_workspace() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures/contracts/agent-recovery-turn/workspace/CWR.toml")
    }

    pub(super) fn fixture_host_identity() -> LocalHostIdentity {
        LocalHostIdentity::new(
            crate::v2026_09::test_fixture::actor_ref("person"),
            "cwp://org.coworkerprotocol.local/integrations/test",
        )
    }

    #[test]
    fn retired_shared_folder_configuration_fails_clearly() {
        let temporary = tempfile::tempdir().unwrap();
        let cwr_toml = temporary.path().join("CWR.toml");
        fs::write(&cwr_toml, "").unwrap();
        fs::create_dir(temporary.path().join(".cwr")).unwrap();
        fs::write(temporary.path().join(".cwr/shared-folder.json"), "{}").unwrap();

        let error = reject_retired_shared_folder(&cwr_toml).unwrap_err();

        assert!(error
            .to_string()
            .contains("shared-folder workspaces are no longer supported"));
    }

    #[test]
    fn retired_shared_folder_metadata_errors_are_returned() {
        let temporary = tempfile::tempdir().unwrap();
        let cwr_toml = temporary.path().join("CWR.toml");
        fs::write(&cwr_toml, "").unwrap();
        fs::create_dir(temporary.path().join(".cwr")).unwrap();
        std::os::unix::fs::symlink(
            "shared-folder.json",
            temporary.path().join(".cwr/shared-folder.json"),
        )
        .unwrap();

        reject_retired_shared_folder(&cwr_toml)
            .expect_err("an unstatable retired marker must not be ignored");
    }

    #[test]
    fn retired_shared_folder_check_accepts_directory_and_symlink_paths() {
        let temporary = tempfile::tempdir().unwrap();
        let workspace = temporary.path().join("workspace");
        fs::create_dir(&workspace).unwrap();
        fs::write(workspace.join("CWR.toml"), "").unwrap();
        fs::create_dir(workspace.join(".cwr")).unwrap();
        fs::write(workspace.join(".cwr/shared-folder.json"), "{}").unwrap();
        let linked_manifest = temporary.path().join("linked-CWR.toml");
        std::os::unix::fs::symlink(workspace.join("CWR.toml"), &linked_manifest).unwrap();

        for candidate in [&workspace, &linked_manifest] {
            let error = reject_retired_shared_folder(candidate).unwrap_err();
            assert!(error
                .to_string()
                .contains("shared-folder workspaces are no longer supported"));
        }
    }

    fn detached_parent_and_child(runtime: &LocalRuntime) -> (String, String) {
        let admission = runtime.admit_text(None, "@helper coordinate").unwrap();
        let parent_ref = admission.invocation_ref().unwrap().to_owned();
        let child_ref = {
            let mut state = runtime.lock().unwrap();
            let parent = match state.admission.claim_invocation(&parent_ref, 10).unwrap() {
                ConversationInvocationClaimOutcome::Claimed(claimed) => claimed.claim.invocation,
                ConversationInvocationClaimOutcome::NotRunnable { .. } => {
                    panic!("new invocation was not runnable")
                }
            };
            let child_ref = new_invocation_ref(state.admission.runtime_root_ref());
            let call_message = state
                .admission
                .append_host_action_call(ConversationHostActionCall {
                    conversation_ref: admission.conversation_ref(),
                    caller_invocation_ref: &parent_ref,
                    host_call_id: "start-detached-child",
                    nested_invocation_ref: &child_ref,
                    caller_actor_ref: &parent.target_actor_ref,
                    target_actor_ref: &parent.target_actor_ref,
                    activation: "@helper--draft--brief",
                    input: json!({}),
                    text: "@helper--draft--brief".into(),
                })
                .unwrap();
            state
                .admission
                .start_child_invocation(
                    StartActionInvocation {
                        invocation_ref: child_ref.clone(),
                        origin: InvocationOrigin::HostCall {
                            caller_invocation_ref: parent_ref.clone(),
                            call_id: "start-detached-child".into(),
                        },
                        requester_actor_ref: parent.target_actor_ref.clone(),
                        target_actor_ref: parent.target_actor_ref.clone(),
                        conversation_ref: Some(admission.conversation_ref().into()),
                        actor_integration_ref: parent.actor_integration_ref.clone(),
                        action_name: "draft--brief".into(),
                        input: json!({}),
                        parent_invocation_ref: Some(parent_ref.clone()),
                        admitted_at_unix_ms: 20,
                    },
                    &call_message.message_ref,
                    AdapterInvocation::new(
                        child_ref.clone(),
                        "draft--brief",
                        json!({}),
                        json!({"type": "object"}),
                    ),
                )
                .unwrap();
            child_ref
        };
        (parent_ref, child_ref)
    }

    include!("local/tests/person.rs");
    include!("local/tests/admission.rs");
    include!("local/tests/deadline.rs");
    include!("local/tests/participation.rs");
    include!("local/tests/conversation_creation.rs");
    include!("local/tests/context_discovery.rs");
    include!("local/tests/conversation_reads.rs");
    include!("local/tests/conversation_module.rs");
    include!("local/tests/conversation_module_operations.rs");
    include!("local/tests/timeline_rollout.rs");
    include!("local/tests/transfer.rs");
    include!("local/tests/recovery.rs");
    include!("local/tests/file_replay.rs");
    include!("local/tests/queue.rs");
    include!("local/tests/host_calls.rs");
    include!("local/tests/gateway.rs");
    include!("local/tests/gateway_host.rs");
    include!("local/tests/gateway_host_reload.rs");
    include!("local/tests/gateway_host_admission.rs");
    include!("local/tests/gateway_configuration.rs");
    include!("local/tests/gateway_continuity.rs");
    include!("local/tests/gateway_process.rs");
    include!("local/tests/gateway_mcp.rs");
    include!("local/tests/gateway_inspection.rs");
    include!("local/tests/native_activity.rs");
    include!("local/tests/scoped_turn_status.rs");
    include!("local/tests/execution_status_publication.rs");
    include!("local/tests/stop.rs");
    include!("local/tests/stop_readiness.rs");
    include!("local/tests/stop_request.rs");
    include!("local/tests/archive.rs");
    include!("local/tests/archive_recovery.rs");
    include!("local/tests/archive_lifecycle.rs");
    include!("local/tests/archive_process.rs");

    include!("local/tests/catalog.rs");
    include!("local/tests/delegation.rs");
    include!("local/tests/read_budget.rs");
    include!("local/tests/route_binding.rs");
    include!("local/tests/scheduling.rs");
    include!("local/tests/detached_scheduling.rs");
    include!("local/tests/metrics.rs");
    include!("local/tests/shutdown.rs");
    include!("local/tests/generation_restart.rs");
    include!("local/tests/conversation_ownership.rs");
    include!("local/tests/schedules_spike.rs");
    include!("local/tests/historical_identity.rs");
    include!("local/tests/context_revocation.rs");
    include!("local/tests/availability_boundary.rs");
    include!("local/tests/context_projection.rs");
    include!("local/tests/cloud_delivery.rs");
}

include!("local/person.rs");

include!("local/provider_question.rs");

mod graphql_endpoint;
pub use graphql_endpoint::LocalGraphqlEndpoint;

include!("local/conversation_contexts.rs");
