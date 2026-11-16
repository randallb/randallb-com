use super::*;
use std::{collections::BTreeSet, path::PathBuf};

use cwr::intrinsic_cwr::{IntrinsicCwrCommand, IntrinsicCwrResponse};
use cwr::v2026_09::{
    conversation::{
        ConversationActivity, ConversationActivityEvent, ConversationSummaryPageRequest,
    },
    local::{LocalHostIdentity, LocalRuntimeEvent},
    service::{
        CommunicationDestination, GatewayInvocationKind, RuntimeServiceCommand, RuntimeServiceEvent,
    },
};

mod archive_controls;
mod control;
mod delivery;
mod execution;
mod gateway;
mod host_admission_status;
mod intrinsic;
mod intrinsic_stop;
mod presentation;
mod reads;
mod retirement;
mod schedule_status;
mod scoped_control_status;
mod scoped_status;
mod wire;
mod wire_bootstrap;

fn fixture_workspace() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../packages/cwr-core/fixtures/contracts/agent-recovery-turn/workspace/CWR.toml")
}

#[test]
fn runtime_service_owns_submission_and_snapshot() {
    let temporary = tempfile::tempdir().expect("create runtime state");
    let runtime_ref = "cwp://example.test/runtime";
    let runtime = LocalRuntime::open_with_state_path_and_runtime_ref(
        fixture_workspace(),
        temporary.path().join("conversation-admission.json"),
        runtime_ref,
        LocalHostIdentity::new(
            crate::test_refs::actor_ref("person").as_str(),
            "cwp://example.test/integrations/runtime",
        ),
    )
    .expect("open runtime owner");
    let state = Arc::new(RuntimeServerShared {
        state: Mutex::new(RuntimeServerState {
            generation_restart: None,
            committed_retirement: None,
            armed_retirement: None,
            retirement_allowed: false,
            controls_finalizing: false,
            conversation_controls: BTreeMap::new(),
            runtime_ref: runtime_ref.into(),
            runtime,
            subscribers: Vec::new(),
            host_actors: BTreeMap::new(),
            communication_adapters: BTreeMap::new(),
            gateways: BTreeMap::new(),
            attachments: BTreeMap::new(),
            pending_default_conversation_refs: BTreeSet::new(),
            expected_communication_adapters: 0,
            attachment_state_path: temporary.path().join("conversation-attachments.json"),
            published_turn_statuses: BTreeMap::new(),
            published_stopped_invocations: std::collections::BTreeSet::new(),
            live_statuses: LiveStatusRegistry::default(),
            live_outputs: LiveOutputRegistry::default(),
        }),
        broadcast_lock: Mutex::new(()),
        graphql_changes: tokio::sync::watch::channel(()).0,
        scheduler: scheduler::ExecutionScheduler::default(),
        activity: crate::idle::Activity::default(),
    });
    let (client, server) = UnixStream::pair().expect("create runtime wire pair");
    let writer = client_writer(server).unwrap();

    execute_command(
        &state,
        &writer,
        RuntimeServiceCommand::SubmitText {
            version: RUNTIME_SERVICE_VERSION,
            request_id: "desktop-1".into(),
            integration_ref: "cwp://example.test/integrations/desktop".into(),
            author_actor_ref: crate::test_refs::actor_ref("person").as_str().into(),
            conversation_ref: None,
            text: "retain this note".into(),
        },
    )
    .expect("submit through runtime service");

    let mut lines = BufReader::new(client).lines();
    let event: RuntimeServiceEvent = serde_json::from_str(
        &lines
            .next()
            .expect("runtime response")
            .expect("read runtime response"),
    )
    .expect("decode runtime response");
    let RuntimeServiceEvent::CommandSucceeded {
        request_id,
        admission: Some(admission),
        ..
    } = event
    else {
        panic!("submission must return its retained admission")
    };
    assert_eq!(request_id, "desktop-1");
    assert_eq!(admission.conversation.messages[0].text, "retain this note");
    assert_eq!(
        lock_state(&state)
            .expect("inspect runtime state")
            .runtime
            .conversations()
            .expect("read conversations")
            .len(),
        1
    );
}

#[test]
fn runtime_snapshot_replays_pending_completed_output_before_clearing_it() {
    let temporary = tempfile::tempdir().expect("create runtime state");
    let state = runtime_state(&temporary);
    let runtime = lock_state(&state).expect("inspect runtime").runtime.clone();
    let admission = runtime
        .submit_text_as(
            "cwp://example.test/integrations/desktop",
            "live-output-root",
            crate::test_refs::actor_ref("person").as_str(),
            None,
            "retain this note",
        )
        .expect("create conversation");
    let invocation_ref = "cwp://example.test/runtime/invocations/live";
    observe_live_output(
        &state,
        admission.conversation_ref(),
        &LocalRuntimeEvent::InvocationStarted {
            invocation_ref: invocation_ref.into(),
            generation: 1,
            target_actor_ref: crate::test_refs::actor_ref("helper").as_str().into(),
        },
    )
    .expect("register live invocation");
    observe_live_output(
        &state,
        admission.conversation_ref(),
        &LocalRuntimeEvent::Activity {
            invocation_ref: invocation_ref.into(),
            target_actor_ref: crate::test_refs::actor_ref("helper").as_str().into(),
            activity: Box::new(ConversationActivity {
                activity_ref: "cwp://example.test/runtime/activities/live".into(),
                sequence: 1,
                occurred_at_unix_ms: 1,
                event: ConversationActivityEvent::CommentaryProgress {
                    model_run_ref: "cwp://example.test/runtime/model_runs/live".into(),
                    provider_integration_ref: "~@/integrations/codex".into(),
                    provider: "codex".into(),
                    thread_id: Some("thread-live".into()),
                    response_or_turn_id: "turn-live".into(),
                    item_id: "item-live".into(),
                    text: "Checking the workspace".into(),
                },
            }),
        },
    )
    .expect("publish completed commentary");

    let mut live_outputs = Vec::new();
    for event in snapshot_events(&state, "desktop-reconnect") {
        if let RuntimeServiceEvent::ConversationLiveOutput {
            conversation_ref,
            output,
        } = event
        {
            live_outputs.push((conversation_ref, output));
        }
    }
    assert_eq!(live_outputs.len(), 2);
    assert_eq!(live_outputs[0].0, admission.conversation_ref());
    let output = live_outputs[0]
        .1
        .as_ref()
        .expect("snapshot includes the exact completed output");
    assert_eq!(output.invocation_ref, invocation_ref);
    assert_eq!(output.text, "Checking the workspace");
    assert_eq!(
        live_outputs[1],
        (admission.conversation_ref().to_owned(), None)
    );
}

#[test]
fn late_adapter_registration_backfills_a_new_default_conversation() {
    let temporary = tempfile::tempdir().expect("create runtime state");
    let state = runtime_state_with_expected_adapters(&temporary, 1);
    let (submit_client, submit_server) = UnixStream::pair().expect("create submit wire pair");
    execute_command(
        &state,
        &client_writer(submit_server).unwrap(),
        RuntimeServiceCommand::SubmitText {
            version: RUNTIME_SERVICE_VERSION,
            request_id: "desktop-before-slack".into(),
            integration_ref: "cwp://example.test/integrations/desktop".into(),
            author_actor_ref: crate::test_refs::actor_ref("person").as_str().into(),
            conversation_ref: None,
            text: "created while Slack connects".into(),
        },
    )
    .expect("submit before adapter registration");
    drop(submit_client);
    assert_eq!(
        lock_state(&state)
            .expect("inspect pending defaults")
            .pending_default_conversation_refs
            .len(),
        1
    );

    let (adapter_client, adapter_server) = UnixStream::pair().expect("create adapter wire pair");
    execute_command(
        &state,
        &client_writer(adapter_server).unwrap(),
        RuntimeServiceCommand::RegisterCommunicationAdapter {
            version: RUNTIME_SERVICE_VERSION,
            request_id: "register-slack".into(),
            adapter_session_ref: "cwp://example.test/sessions/slack".into(),
            integration_ref: SLACK_INTEGRATION_REF.into(),
            destinations: vec![CommunicationDestination {
                destination_ref: SLACK_DESTINATION_REF.into(),
                display_name: "Slack".into(),
                default_for_new_conversations: true,
            }],
        },
    )
    .expect("register adapter");

    let events = BufReader::new(adapter_client)
        .lines()
        .take(2)
        .map(|line| {
            serde_json::from_str::<RuntimeServiceEvent>(&line.expect("read adapter event"))
                .expect("decode adapter event")
        })
        .collect::<Vec<_>>();
    assert!(matches!(
        events[0],
        RuntimeServiceEvent::CommandSucceeded { .. }
    ));
    assert!(matches!(
        events[1],
        RuntimeServiceEvent::CreateAttachment { .. }
    ));
    let state = lock_state(&state).expect("inspect attachment backfill");
    assert_eq!(state.attachments.len(), 1);
    assert!(state.pending_default_conversation_refs.is_empty());
}

#[test]
fn runtime_service_executes_intrinsics_against_the_central_catalog() {
    let temporary = tempfile::tempdir().expect("create runtime state");
    let state = runtime_state(&temporary);
    let (client, server) = UnixStream::pair().expect("create intrinsic wire pair");
    execute_command(
        &state,
        &client_writer(server).unwrap(),
        RuntimeServiceCommand::ExecuteIntrinsic {
            version: RUNTIME_SERVICE_VERSION,
            request_id: "intrinsic-list".into(),
            operation_id: None,
            command: IntrinsicCwrCommand::ListCoworkers,
            requesting_actor_ref: crate::test_refs::actor_ref("person").as_str().into(),
            conversation_ref: None,
        },
    )
    .expect("execute intrinsic");
    let event: RuntimeServiceEvent = serde_json::from_str(
        &BufReader::new(client)
            .lines()
            .next()
            .expect("intrinsic response")
            .expect("read intrinsic response"),
    )
    .expect("decode intrinsic response");
    let RuntimeServiceEvent::CommandSucceeded {
        intrinsic_response: Some(response),
        ..
    } = event
    else {
        panic!("intrinsic command must return its response")
    };
    assert!(matches!(*response, IntrinsicCwrResponse::Coworkers { .. }));
}

#[test]
fn slack_ingress_binds_the_existing_desktop_attachment() {
    let temporary = tempfile::tempdir().expect("create runtime state");
    let state = runtime_state(&temporary);
    let runtime = lock_state(&state)
        .expect("inspect runtime state")
        .runtime
        .clone();
    let desktop = runtime
        .submit_text_as(
            "cwp://example.test/integrations/desktop",
            "desktop-root",
            crate::test_refs::actor_ref("person").as_str(),
            None,
            "started on desktop",
        )
        .expect("admit Desktop root");
    let attachment_ref = "cwp://example.test/runtime/attachments/desktop";
    seed_attachment(
        &state,
        attachment_ref,
        &desktop,
        None,
        &desktop.record.source_message_ref,
    );
    let slack = runtime
        .submit_text_as(
            SLACK_INTEGRATION_REF,
            "slack-reply",
            crate::test_refs::actor_ref("person").as_str(),
            Some(desktop.conversation_ref()),
            "continued in Slack",
        )
        .expect("admit Slack reply");

    let retained_ref =
        attachments::retain_ingress(&state, &slack, slack_attachment(SLACK_BINDING_KEY))
            .expect("retain Slack ingress attachment");

    assert_eq!(retained_ref, attachment_ref);
    let state = lock_state(&state).expect("inspect reconciled attachments");
    assert_eq!(state.attachments.len(), 1);
    let retained = &state.attachments[attachment_ref];
    assert_eq!(retained.binding_key.as_deref(), Some(SLACK_BINDING_KEY));
    assert!(retained
        .source_message_refs
        .contains(&desktop.record.source_message_ref));
    assert!(retained
        .source_message_refs
        .contains(&slack.record.source_message_ref));
}

#[test]
fn slack_ingress_merges_a_legacy_duplicate_attachment() {
    let temporary = tempfile::tempdir().expect("create runtime state");
    let state = runtime_state(&temporary);
    let runtime = lock_state(&state)
        .expect("inspect runtime state")
        .runtime
        .clone();
    let desktop = runtime
        .submit_text_as(
            "cwp://example.test/integrations/desktop",
            "desktop-root",
            crate::test_refs::actor_ref("person").as_str(),
            None,
            "started on desktop",
        )
        .expect("admit Desktop root");
    let first_slack = runtime
        .submit_text_as(
            SLACK_INTEGRATION_REF,
            "slack-first",
            crate::test_refs::actor_ref("person").as_str(),
            Some(desktop.conversation_ref()),
            "first Slack reply",
        )
        .expect("admit first Slack reply");
    seed_attachment(
        &state,
        "cwp://example.test/runtime/attachments/desktop",
        &desktop,
        None,
        &desktop.record.source_message_ref,
    );
    let bound_ref = "cwp://example.test/runtime/attachments/slack";
    seed_attachment(
        &state,
        bound_ref,
        &desktop,
        Some(SLACK_BINDING_KEY),
        &first_slack.record.source_message_ref,
    );
    let next_slack = runtime
        .submit_text_as(
            SLACK_INTEGRATION_REF,
            "slack-next",
            crate::test_refs::actor_ref("person").as_str(),
            Some(desktop.conversation_ref()),
            "next Slack reply",
        )
        .expect("admit next Slack reply");

    let retained_ref =
        attachments::retain_ingress(&state, &next_slack, slack_attachment(SLACK_BINDING_KEY))
            .expect("reconcile duplicate Slack attachments");

    assert_eq!(retained_ref, bound_ref);
    let state = lock_state(&state).expect("inspect reconciled attachments");
    assert_eq!(state.attachments.len(), 1);
    let retained = &state.attachments[bound_ref];
    for source_message_ref in [
        &desktop.record.source_message_ref,
        &first_slack.record.source_message_ref,
        &next_slack.record.source_message_ref,
    ] {
        assert!(retained.source_message_refs.contains(source_message_ref));
    }
}

const SLACK_INTEGRATION_REF: &str = "~@/integrations/slack";
const SLACK_DESTINATION_REF: &str = "~@/integrations/slack/destinations/channel";
const SLACK_BINDING_KEY: &str = "team:channel:thread";

pub(super) fn runtime_state(temporary: &tempfile::TempDir) -> SharedState {
    runtime_state_with_expected_adapters(temporary, 0)
}

fn snapshot_events(state: &SharedState, request_id: &str) -> Vec<RuntimeServiceEvent> {
    let (client, server) = UnixStream::pair().expect("create snapshot wire pair");
    let timeout = Some(std::time::Duration::from_secs(2));
    client.set_read_timeout(timeout).unwrap();
    server.set_write_timeout(timeout).unwrap();

    // Drain the socket while the server writes; snapshots need not fit its buffer.
    std::thread::scope(|scope| {
        let reader = scope.spawn(|| {
            let mut events = Vec::new();
            for line in BufReader::new(client).lines() {
                let event: RuntimeServiceEvent =
                    serde_json::from_str(&line.expect("read snapshot event"))
                        .expect("decode snapshot event");
                let complete = matches!(
                    &event,
                    RuntimeServiceEvent::CommandSucceeded { request_id: completed_id, .. }
                        if completed_id == request_id
                );
                events.push(event);
                if complete {
                    return events;
                }
            }
            panic!("snapshot ended without command completion");
        });
        execute_command(
            state,
            &client_writer(server).unwrap(),
            RuntimeServiceCommand::Snapshot {
                version: RUNTIME_SERVICE_VERSION,
                request_id: request_id.into(),
                metadata_only: false,
            },
        )
        .expect("request runtime snapshot");
        reader.join().expect("snapshot reader completed")
    })
}

fn runtime_state_with_expected_adapters(
    temporary: &tempfile::TempDir,
    expected_communication_adapters: usize,
) -> SharedState {
    runtime_state_with_options(
        temporary,
        expected_communication_adapters,
        Default::default(),
    )
}

fn runtime_state_with_options(
    temporary: &tempfile::TempDir,
    expected_communication_adapters: usize,
    options: cwr::v2026_09::local::LocalRuntimeOptions,
) -> SharedState {
    let runtime_ref = "cwp://example.test/runtime";
    let runtime = LocalRuntime::open_with_state_path_and_runtime_ref_and_options(
        fixture_workspace(),
        temporary.path().join("conversation-admission.json"),
        runtime_ref,
        LocalHostIdentity::new(
            crate::test_refs::actor_ref("person").as_str(),
            "cwp://example.test/integrations/runtime",
        ),
        options,
    )
    .expect("open runtime owner");
    Arc::new(RuntimeServerShared {
        state: Mutex::new(RuntimeServerState {
            generation_restart: None,
            committed_retirement: None,
            armed_retirement: None,
            retirement_allowed: false,
            controls_finalizing: false,
            conversation_controls: BTreeMap::new(),
            runtime_ref: runtime_ref.into(),
            runtime,
            subscribers: Vec::new(),
            host_actors: BTreeMap::new(),
            communication_adapters: BTreeMap::new(),
            gateways: BTreeMap::new(),
            attachments: BTreeMap::new(),
            pending_default_conversation_refs: BTreeSet::new(),
            expected_communication_adapters,
            attachment_state_path: temporary.path().join("conversation-attachments.json"),
            published_turn_statuses: BTreeMap::new(),
            published_stopped_invocations: std::collections::BTreeSet::new(),
            live_statuses: LiveStatusRegistry::default(),
            live_outputs: LiveOutputRegistry::default(),
        }),
        broadcast_lock: Mutex::new(()),
        graphql_changes: tokio::sync::watch::channel(()).0,
        scheduler: scheduler::ExecutionScheduler::default(),
        activity: crate::idle::Activity::default(),
    })
}

fn seed_attachment(
    state: &SharedState,
    attachment_ref: &str,
    admission: &LocalAdmission,
    binding_key: Option<&str>,
    source_message_ref: &str,
) {
    lock_state(state)
        .expect("seed runtime attachment")
        .attachments
        .insert(
            attachment_ref.into(),
            RuntimeAttachment {
                conversation_ref: admission.record.conversation_ref.clone(),
                integration_ref: SLACK_INTEGRATION_REF.into(),
                destination_ref: SLACK_DESTINATION_REF.into(),
                binding_key: binding_key.map(str::to_owned),
                source_message_refs: BTreeSet::from([source_message_ref.into()]),
                attached: true,
                attachment_request_in_flight: false,
                requested_delivery_refs: BTreeSet::new(),
                terminal_delivery_refs: BTreeSet::new(),
            },
        );
}

fn slack_attachment(binding_key: &str) -> cwr::v2026_09::service::RuntimeIngressAttachment {
    cwr::v2026_09::service::RuntimeIngressAttachment {
        integration_ref: SLACK_INTEGRATION_REF.into(),
        destination_ref: SLACK_DESTINATION_REF.into(),
        binding_key: binding_key.into(),
    }
}
