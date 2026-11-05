use std::{
    collections::BTreeMap,
    error::Error,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    sync::mpsc::{self, Receiver, RecvTimeoutError},
    thread,
    time::Duration,
};

use cwr::v2026_09::{
    conversation::{
        work_invocation_belongs_to_root, ConversationActivityEvent, RetainedConversation,
    },
    host::{
        HostActionCatalog, HostActor, HostCommand, HostCommandReceipt, HostEvent,
        HOST_COMMAND_VERSION,
    },
    invocation::{ActionResponse, ActionResponseDetail, ActionResponseStatus},
};
use serde_json::Value;

const CLI_INGRESS_INTEGRATION_REF: &str = "cwp://org.coworkerprotocol.local/integrations/cli";

pub(super) struct RemoteClient {
    writer: ChildStdin,
    bridge: Child,
    incoming: Receiver<RemoteIncoming>,
    actor_ref: String,
    source_ref: String,
    actor: HostActor,
    catalog: HostActionCatalog,
    conversations: BTreeMap<String, RetainedConversation>,
    request_session_id: String,
    next_request: u64,
}

enum RemoteIncoming {
    Event(Box<HostEvent>),
    Closed(String),
}

const REMOTE_POLL: Duration = Duration::from_millis(25);

pub(super) enum RemoteSubmission {
    Conversation(HostCommandReceipt),
    Intrinsic(cwr::intrinsic_cwr::IntrinsicCwrResponse),
}

pub(super) struct RemoteTurn {
    pub conversation: RetainedConversation,
    pub status: ActionResponseStatus,
    pub actor_ref: String,
    pub summary: Option<String>,
    pub response: Option<ActionResponse>,
}

impl RemoteClient {
    pub(super) fn open(
        cwr_toml: &Path,
        requested_actor_ref: Option<String>,
    ) -> Result<Self, Box<dyn Error + Send + Sync>> {
        Self::open_selected(cwr_toml, None, requested_actor_ref)
    }

    pub(super) fn open_selected(
        cwr_toml: &Path,
        configured_runtime: Option<&str>,
        requested_actor_ref: Option<String>,
    ) -> Result<Self, Box<dyn Error + Send + Sync>> {
        let opening = crate::startup_timing::span("client_open");
        let spawn = crate::startup_timing::span("bridge_spawn");
        let executable = std::env::var_os("CWR_DEV_RUNTIME_EXECUTABLE")
            .map(PathBuf::from)
            .unwrap_or(
                std::env::current_exe()?
                    .with_file_name(format!("cwp-runtime{}", std::env::consts::EXE_SUFFIX)),
            );
        if !executable.is_file() {
            return Err("cwp-runtime is not installed beside cwr".into());
        }
        let mut command = Command::new(executable);
        command.arg(cwr_toml);
        if let Some(name) = configured_runtime {
            command.args(["--runtime", name]);
        }
        command.args([
            std::ffi::OsStr::new("--host-stdio"),
            std::ffi::OsStr::new("--ingress"),
            std::ffi::OsStr::new(CLI_INGRESS_INTEGRATION_REF),
        ]);
        let mut bridge = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()?;
        spawn.finish();
        let writer = bridge
            .stdin
            .take()
            .ok_or("cwp-runtime stdin is unavailable")?;
        let reader = bridge
            .stdout
            .take()
            .ok_or("cwp-runtime stdout is unavailable")?;
        let incoming = start_event_reader(reader)?;
        let mut client = Self {
            writer,
            bridge,
            incoming,
            actor_ref: String::new(),
            source_ref: String::new(),
            actor: HostActor {
                actor_ref: String::new(),
                coworker_ref: String::new(),
                handle: String::new(),
                name: String::new(),
                category: None,
                provider_model: None,
            },
            catalog: HostActionCatalog {
                retained_actors: Vec::new(),
                known_actors: Vec::new(),
                default_conversation_target_ref: None,
                actors: Vec::new(),
                actions: Vec::new(),
            },
            conversations: BTreeMap::new(),
            request_session_id: uuid::Uuid::new_v4().simple().to_string(),
            next_request: 1,
        };
        let inspection = crate::startup_timing::span("workspace_inspection");
        let inspect = client.request_id("inspect");
        client.send(&HostCommand::InspectWorkspace {
            version: HOST_COMMAND_VERSION,
            request_id: inspect.clone(),
        })?;
        match client.wait_for_request(&inspect)? {
            HostEvent::WorkspaceInspected {
                default_actor_ref, ..
            } => {
                client.actor_ref = requested_actor_ref.or(default_actor_ref).ok_or(
                    "local host requires an explicit acting actor; use --actor or CWR_ACTOR_REF",
                )?;
            }
            event => return Err(unexpected("workspace inspection", &event)),
        }
        inspection.finish();
        let selection = crate::startup_timing::span("actor_selection");
        let select = client.request_id("select");
        client.send(&HostCommand::SelectActor {
            version: HOST_COMMAND_VERSION,
            request_id: select.clone(),
            actor_ref: client.actor_ref.clone(),
        })?;
        match client.wait_for_request(&select)? {
            HostEvent::ActorSelected { actor, catalog, .. } => {
                client.actor = actor;
                client.catalog = catalog;
            }
            event => return Err(unexpected("actor selection", &event)),
        }
        selection.finish();
        let snapshot = crate::startup_timing::span("initial_snapshot");
        let mut saw_snapshot = false;
        while client.source_ref.is_empty() || !saw_snapshot {
            let event = client.read_event()?;
            saw_snapshot |= matches!(event, HostEvent::ConversationSnapshot { .. });
            client.apply(event);
        }
        snapshot.finish();
        opening.finish();
        Ok(client)
    }

    pub(super) fn catalog(&self) -> &HostActionCatalog {
        &self.catalog
    }

    pub(super) fn conversation(&self, conversation_ref: &str) -> Option<&RetainedConversation> {
        self.conversations.get(conversation_ref)
    }

    pub(super) fn submit_text(
        &mut self,
        conversation_ref: Option<String>,
        text: String,
    ) -> Result<RemoteSubmission, Box<dyn Error + Send + Sync>> {
        let request_id = self.request_id("submit");
        let command = match conversation_ref {
            None => HostCommand::CreateConversation {
                version: HOST_COMMAND_VERSION,
                request_id: request_id.clone(),
                source_ref: self.source_ref.clone(),
                author_actor_ref: self.actor_ref.clone(),
                text,
            },
            Some(conversation_ref) => HostCommand::ContinueConversation {
                version: HOST_COMMAND_VERSION,
                request_id: request_id.clone(),
                source_ref: self.source_ref.clone(),
                conversation_ref,
                author_actor_ref: self.actor_ref.clone(),
                text,
                recovery_only: false,
            },
        };
        let submission = crate::startup_timing::span("submission_to_admission");
        self.send(&command)?;
        let result = self.submission_response(&request_id)?;
        submission.finish();
        Ok(result)
    }

    pub(super) fn invoke_action(
        &mut self,
        activation: String,
        input: Value,
    ) -> Result<RemoteSubmission, Box<dyn Error + Send + Sync>> {
        let submission = crate::startup_timing::span("submission_to_admission");
        let request_id = self.request_id("action");
        self.send(&HostCommand::InvokeAction {
            version: HOST_COMMAND_VERSION,
            request_id: request_id.clone(),
            source_ref: Some(self.source_ref.clone()),
            author_actor_ref: self.actor_ref.clone(),
            conversation_ref: None,
            activation,
            input,
        })?;
        let result = self.submission_response(&request_id)?;
        submission.finish();
        Ok(result)
    }

    pub(super) fn wait_for_turn_interruptible(
        &mut self,
        receipt: &HostCommandReceipt,
        mut observe: impl FnMut(&HostEvent) -> Result<(), Box<dyn Error + Send + Sync>>,
        mut interrupted: impl FnMut() -> bool,
    ) -> Result<Option<RemoteTurn>, Box<dyn Error + Send + Sync>> {
        let Some(invocation_ref) = receipt.invocation_ref.as_deref() else {
            return Ok(None);
        };
        let mut stop_requested = false;
        loop {
            if !stop_requested && interrupted() {
                let request_id = self.request_id("interrupt");
                self.send(&HostCommand::StopConversation {
                    version: HOST_COMMAND_VERSION,
                    request_id,
                    source_ref: self.source_ref.clone(),
                    conversation_ref: receipt.conversation_ref.clone(),
                    requesting_actor_ref: self.actor_ref.clone(),
                    invocation_ref: Some(invocation_ref.into()),
                })?;
                stop_requested = true;
            }
            let event = match self.incoming.recv_timeout(REMOTE_POLL) {
                Ok(RemoteIncoming::Event(event)) => *event,
                Ok(RemoteIncoming::Closed(error)) => return Err(error.into()),
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => {
                    return Err("resident CWR runtime event reader stopped".into())
                }
            };
            observe(&event)?;
            let terminal = terminal_update(&event, &receipt.conversation_ref, invocation_ref);
            self.apply(event);
            if let Some((status, actor_ref, summary, response)) = terminal {
                let conversation = self
                    .conversations
                    .get(&receipt.conversation_ref)
                    .cloned()
                    .ok_or("resident runtime completed work without retaining its conversation")?;
                return Ok(Some(RemoteTurn {
                    conversation,
                    status,
                    actor_ref,
                    summary,
                    response,
                }));
            }
        }
    }

    fn submission_response(
        &mut self,
        request_id: &str,
    ) -> Result<RemoteSubmission, Box<dyn Error + Send + Sync>> {
        match self.wait_for_request(request_id)? {
            HostEvent::CommandSucceeded { receipt, .. } => {
                Ok(RemoteSubmission::Conversation(receipt))
            }
            HostEvent::IntrinsicResponse { response, .. } => {
                Ok(RemoteSubmission::Intrinsic(*response))
            }
            HostEvent::CommandFailed { error, .. } => Err(error.into()),
            event => Err(unexpected("command response", &event)),
        }
    }

    fn request_id(&mut self, label: &str) -> String {
        let request_id = format!(
            "cli-{}-{label}-{}",
            self.request_session_id, self.next_request
        );
        self.next_request += 1;
        request_id
    }

    fn send(&mut self, command: &HostCommand) -> Result<(), Box<dyn Error + Send + Sync>> {
        command.validate()?;
        serde_json::to_writer(&mut self.writer, command)?;
        self.writer.write_all(b"\n")?;
        self.writer.flush()?;
        Ok(())
    }

    fn wait_for_request(
        &mut self,
        request_id: &str,
    ) -> Result<HostEvent, Box<dyn Error + Send + Sync>> {
        loop {
            let event = self.read_event()?;
            if event_request_id(&event) == Some(request_id) {
                return Ok(event);
            }
            self.apply(event);
        }
    }

    fn read_event(&mut self) -> Result<HostEvent, Box<dyn Error + Send + Sync>> {
        match self.incoming.recv()? {
            RemoteIncoming::Event(event) => Ok(*event),
            RemoteIncoming::Closed(error) => Err(error.into()),
        }
    }

    fn apply(&mut self, event: HostEvent) {
        match event {
            HostEvent::Ready { sources } => {
                if let Some(source) = sources.into_iter().next() {
                    self.source_ref = source.source_ref;
                }
            }
            HostEvent::ConversationSnapshot { conversations, .. } => {
                self.conversations = conversations
                    .into_iter()
                    .map(|conversation| (conversation.conversation_ref.clone(), conversation))
                    .collect();
            }
            HostEvent::ConversationUpsert { conversation, .. } => {
                self.conversations
                    .insert(conversation.conversation_ref.clone(), conversation);
            }
            HostEvent::ConversationRemoved {
                conversation_ref, ..
            } => {
                self.conversations.remove(&conversation_ref);
            }
            HostEvent::CatalogUpdated { actor, catalog } => {
                self.actor = actor;
                self.catalog = catalog;
            }
            HostEvent::RuntimeRestarting { .. } => {
                eprintln!("Development runtime rebuilt; reconnect with the next command. Unacknowledged commands were not resent.");
            }
            HostEvent::UpstreamRegistered { .. }
            | HostEvent::RootConversationsPage { .. }
            | HostEvent::ConversationRecordsPage { .. }
            | HostEvent::VisualizationRead { .. }
            | HostEvent::Upstream { .. }
            | HostEvent::WorkspaceInspected { .. }
            | HostEvent::ActorSelected { .. }
            | HostEvent::ConversationLiveOutput { .. }
            | HostEvent::ConversationLiveStatusSnapshot { .. }
            | HostEvent::ConversationTurnStatusSnapshot { .. }
            | HostEvent::ConversationExecutionFinished { .. }
            | HostEvent::ConversationExecutionFailed { .. }
            | HostEvent::CommandQueued { .. }
            | HostEvent::CommandSucceeded { .. }
            | HostEvent::IntrinsicResponse { .. }
            | HostEvent::CommandFailed { .. } => {}
        }
    }
}

pub(super) fn present_turn_progress(
    event: &HostEvent,
    receipt: &HostCommandReceipt,
    observed_activity: &mut std::collections::BTreeSet<String>,
) {
    for line in turn_progress_lines(event, receipt, observed_activity) {
        eprintln!("{line}");
    }
}

fn turn_progress_lines(
    event: &HostEvent,
    receipt: &HostCommandReceipt,
    observed_activity: &mut std::collections::BTreeSet<String>,
) -> Vec<String> {
    let Some(invocation_ref) = receipt.invocation_ref.as_deref() else {
        return Vec::new();
    };
    let HostEvent::ConversationUpsert { conversation, .. } = event else {
        return Vec::new();
    };
    if conversation.conversation_ref != receipt.conversation_ref {
        return Vec::new();
    }
    let mut lines = Vec::new();
    for activity in &conversation.activity {
        let ConversationActivityEvent::WorkUpdate {
            invocation_ref: update_ref,
            status,
            text,
            activity: work_activity,
            ..
        } = &activity.event
        else {
            continue;
        };
        let presentation_ref = work_activity
            .as_deref()
            .map_or(activity.activity_ref.as_str(), |work| {
                work.activity_ref.as_str()
            });
        if !work_invocation_belongs_to_root(conversation, update_ref, invocation_ref)
            || !observed_activity.insert(presentation_ref.into())
            || !status.is_progress()
        {
            continue;
        }
        if let Some(text) = text {
            lines.push(text.clone());
        } else if let Some(work_activity) = work_activity {
            lines.push(work_activity.label.clone());
        }
    }
    lines
}

pub(super) fn event_updates_turn(
    event: &HostEvent,
    receipt: &HostCommandReceipt,
    observed_activity: &mut std::collections::BTreeSet<String>,
) -> bool {
    let Some(invocation_ref) = receipt.invocation_ref.as_deref() else {
        return false;
    };
    match event {
        HostEvent::ConversationUpsert { conversation, .. }
            if conversation.conversation_ref == receipt.conversation_ref =>
        {
            let mut updated = false;
            for activity in &conversation.activity {
                if matches!(
                    &activity.event,
                    ConversationActivityEvent::WorkUpdate {
                        invocation_ref: update_ref,
                        ..
                    } if work_invocation_belongs_to_root(
                        conversation,
                        update_ref,
                        invocation_ref,
                    )
                ) {
                    updated |= observed_activity.insert(activity.activity_ref.clone());
                }
            }
            updated
        }
        HostEvent::ConversationLiveOutput {
            conversation_ref,
            output: Some(output),
            ..
        } => {
            conversation_ref == &receipt.conversation_ref && output.invocation_ref == invocation_ref
        }
        HostEvent::ConversationRemoved {
            conversation_ref, ..
        } => conversation_ref == &receipt.conversation_ref,
        HostEvent::ConversationExecutionFinished {
            conversation_ref,
            response,
            ..
        } => {
            conversation_ref == &receipt.conversation_ref
                && response.invocation_ref == invocation_ref
        }
        HostEvent::ConversationExecutionFailed {
            conversation_ref,
            invocation_ref: failed_ref,
            ..
        } => conversation_ref == &receipt.conversation_ref && failed_ref == invocation_ref,
        _ => false,
    }
}

impl Drop for RemoteClient {
    fn drop(&mut self) {
        let _ = self.bridge.kill();
        let _ = self.bridge.wait();
    }
}

pub(super) fn action_response_json(
    turn: &RemoteTurn,
    receipt: &HostCommandReceipt,
) -> serde_json::Value {
    if let Some(response) = &turn.response {
        return serde_json::to_value(response).expect("action responses serialize");
    }
    let action_result = receipt
        .message_ref
        .as_deref()
        .and_then(|source_message_ref| {
            turn.conversation.messages.iter().rev().find_map(|message| {
                if message.reply_to_message_ref.as_deref() != Some(source_message_ref) {
                    return None;
                }
                match &message.action {
                    Some(cwr::v2026_09::conversation::ConversationActionEvent::Result {
                        result,
                        outcome,
                        ..
                    }) => Some((result.clone(), *outcome)),
                    _ => None,
                }
            })
        });
    if let Some(response) = action_result
        .as_ref()
        .filter(|(_, outcome)| {
            matches!(
                outcome,
                Some(cwr::v2026_09::conversation::ConversationActionOutcome::Failed)
            )
        })
        .and_then(|(result, _)| serde_json::from_value::<ActionResponse>(result.clone()).ok())
        .filter(|response| {
            response.invocation_ref == receipt.invocation_ref.as_deref().unwrap_or_default()
        })
    {
        return serde_json::to_value(response).expect("action responses serialize");
    }
    let summary = turn
        .summary
        .clone()
        .unwrap_or_else(|| format!("Action returned status {}.", turn.status.as_u16()));
    let (result, error, cause) = if turn.status.is_completed() {
        (
            Some(
                action_result
                    .map(|(result, _)| result)
                    .unwrap_or(serde_json::Value::Null),
            ),
            None,
            None,
        )
    } else if turn.status.is_stopped() {
        (
            None,
            None,
            Some(ActionResponseDetail::new("resident_execution_stopped").with_message(&summary)),
        )
    } else {
        (
            None,
            Some(ActionResponseDetail::new("resident_execution_failed").with_message(&summary)),
            None,
        )
    };
    serde_json::to_value(ActionResponse {
        progress: None,
        invocation_ref: receipt.invocation_ref.clone().unwrap_or_default(),
        sequence: 1,
        status: turn.status,
        summary,
        result,
        error,
        cause,
    })
    .expect("action responses serialize")
}

include!("remote/reader.rs");

include!("remote/events.rs");

#[cfg(test)]
include!("remote/tests.rs");
