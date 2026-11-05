#[path = "support/refs.rs"]
mod test_refs;
use std::{
    fs,
    io::Write,
    os::unix::fs::PermissionsExt,
    process::{ChildStdin, Command, Stdio},
    sync::mpsc::Receiver,
    thread,
    time::{Duration, Instant},
};

use cwr::v2026_09::{
    conversation::{ConversationAdmissionStore, ConversationSummaryPageRequest},
    host::{HostCommand, HostEvent, HOST_COMMAND_VERSION},
    invocation::ActionInvocationState,
};

const WORKSPACE_ID: &str = "5f0dc30e-41bd-4f99-a8e4-0fb66caef76f";
const RUNTIME_REF: &str =
    "cwp://org.coworkerprotocol.local/runtimes/5f0dc30e-41bd-4f99-a8e4-0fb66caef76f";
const SOURCE_REF: &str = "cwp://org.coworkerprotocol.local/runtimes/5f0dc30e-41bd-4f99-a8e4-0fb66caef76f/sources/conversations";

#[path = "host_stdio/configured.rs"]
mod configured;
#[path = "host_stdio/configured_retirement.rs"]
mod configured_retirement;
#[path = "host_stdio/configured_runtime.rs"]
mod configured_runtime;
#[path = "host_stdio/framing.rs"]
mod framing;
#[path = "host_stdio/generation.rs"]
mod generation;
#[path = "host_stdio/history.rs"]
mod history;
#[path = "host_stdio/ordinary_graphql.rs"]
mod ordinary_graphql;
#[path = "host_stdio/resident.rs"]
mod resident;
#[path = "host_stdio/resident_storage.rs"]
mod resident_storage;
#[path = "host_stdio/support.rs"]
mod support;
#[path = "host_stdio/workspace.rs"]
mod workspace;

use support::{
    runtime_directory, runtime_home, runtime_state_directory, start_runtime, start_runtime_for,
    start_runtime_with_codex, stop_resident,
};

#[test]
fn host_stdio_does_not_start_the_resident_before_actor_selection() {
    let temporary = tempfile::tempdir().expect("temporary runtime configuration");
    let (mut child, mut input, output) = start_runtime(temporary.path());
    let runtime = runtime_directory(temporary.path());

    let inspected = send(
        &mut input,
        &output,
        HostCommand::InspectWorkspace {
            version: HOST_COMMAND_VERSION,
            request_id: "inspect-before-start".into(),
        },
    );
    assert!(matches!(inspected, HostEvent::WorkspaceInspected { .. }));
    assert!(!runtime.join("resident-runtime.pid").exists());
    assert!(!runtime.join("host.sock").exists());

    let selected = send(
        &mut input,
        &output,
        HostCommand::SelectActor {
            version: HOST_COMMAND_VERSION,
            request_id: "select-and-start".into(),
            actor_ref: crate::test_refs::actor_ref("person").as_str().into(),
        },
    );
    assert!(matches!(
        selected,
        HostEvent::ActorSelected { actor, .. } if actor.actor_ref == crate::test_refs::actor_ref("person").as_str()
    ));
    assert!(runtime.join("resident-runtime.pid").exists());

    drop(input);
    child.0.kill().unwrap();
    child.0.wait().unwrap();
    stop_resident(temporary.path());
}

#[test]
fn host_stdio_reads_root_conversations_without_replaying_full_history() {
    let temporary = tempfile::tempdir().expect("temporary runtime configuration");
    let (mut child, mut input, output) = start_runtime(temporary.path());
    write(
        &mut input,
        &HostCommand::SelectActor {
            version: HOST_COMMAND_VERSION,
            request_id: "select-for-page".into(),
            actor_ref: crate::test_refs::actor_ref("person").as_str().into(),
        },
    );
    let mut selected = false;
    let mut ready = false;
    let mut snapshot = false;
    while !(selected && ready && snapshot) {
        match receive(&output) {
            HostEvent::ActorSelected { request_id, .. } if request_id == "select-for-page" => {
                selected = true;
            }
            HostEvent::Ready { .. } => ready = true,
            HostEvent::ConversationSnapshot { .. } => snapshot = true,
            _ => {}
        }
    }
    send(
        &mut input,
        &output,
        HostCommand::CreateConversation {
            version: HOST_COMMAND_VERSION,
            request_id: "create-for-page".into(),
            source_ref: SOURCE_REF.into(),
            author_actor_ref: crate::test_refs::actor_ref("person").as_str().into(),
            text: "show help".into(),
        },
    );

    write(
        &mut input,
        &HostCommand::ReadRootConversations {
            version: HOST_COMMAND_VERSION,
            request_id: "read-page".into(),
            source_ref: SOURCE_REF.into(),
            page: ConversationSummaryPageRequest {
                first: Some(1),
                ..Default::default()
            },
            timeout_ms: 1_000,
        },
    );
    drop(input);
    let event = loop {
        let event = receive(&output);
        match &event {
            HostEvent::ConversationSnapshot { .. } => {
                panic!("a page read must not replay the full conversation snapshot")
            }
            HostEvent::RootConversationsPage { request_id, .. } if request_id == "read-page" => {
                break event
            }
            _ => {}
        }
    };
    let HostEvent::RootConversationsPage {
        request_id,
        source_ref,
        page,
    } = event
    else {
        panic!("read command must return a root conversation page")
    };
    assert_eq!(request_id, "read-page");
    assert_eq!(source_ref, SOURCE_REF);
    assert_eq!(page.summaries.len(), 1);
    assert_eq!(
        page.summaries[0].last_message_preview.as_deref(),
        Some("show help")
    );

    assert!(child
        .wait_for_exit("wait for paged read to drain after stdin closes")
        .success());
    stop_resident(temporary.path());
}

#[test]
fn host_stdio_inspects_selects_and_opens_the_conversation_session() {
    let temporary = tempfile::tempdir().expect("temporary runtime configuration");
    let (mut child, mut input, output) = start_runtime(temporary.path());

    let inspected = send(
        &mut input,
        &output,
        HostCommand::InspectWorkspace {
            version: HOST_COMMAND_VERSION,
            request_id: "inspect".into(),
        },
    );
    let HostEvent::WorkspaceInspected { workspace, .. } = inspected else {
        panic!("inspect command must return a workspace description")
    };
    assert!(workspace
        .workspace_ref
        .starts_with("cwp://org.coworkerprotocol.local/resources/"));
    let actors = workspace
        .actors
        .iter()
        .map(|actor| actor.handle.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        actors,
        std::collections::BTreeSet::from(["@helper", "@person", "@cwr"])
    );

    let rejected = send(
        &mut input,
        &output,
        HostCommand::SelectActor {
            version: HOST_COMMAND_VERSION,
            request_id: "select-missing".into(),
            actor_ref: "~@/actors/missing".into(),
        },
    );
    assert!(
        matches!(rejected, HostEvent::CommandFailed { request_id, error }
        if request_id == "select-missing" && error.contains("not in the resolved graph"))
    );

    let selected = send(
        &mut input,
        &output,
        HostCommand::SelectActor {
            version: HOST_COMMAND_VERSION,
            request_id: "select-person".into(),
            actor_ref: crate::test_refs::actor_ref("person").as_str().into(),
        },
    );
    let HostEvent::ActorSelected { actor, catalog, .. } = selected else {
        panic!("valid selection must open the actor session")
    };
    assert_eq!(
        actor.actor_ref,
        crate::test_refs::actor_ref("person").as_str()
    );
    assert_eq!(catalog.actors.len(), 2);
    assert_eq!(
        catalog.actors[0].actor_ref,
        crate::test_refs::actor_ref("helper").as_str()
    );
    assert_eq!(
        catalog.actors[1].actor_ref,
        cwr::action_contract::INTRINSIC_CWR_ACTOR_REF
    );

    let mut saw_ready = false;
    let mut saw_snapshot = false;
    while !(saw_ready && saw_snapshot) {
        match receive(&output) {
            HostEvent::Ready { sources } => {
                saw_ready = sources.iter().any(|source| source.source_ref == SOURCE_REF);
            }
            HostEvent::ConversationSnapshot { source_ref, .. } => {
                saw_snapshot = source_ref == SOURCE_REF;
            }
            _ => {}
        }
    }

    let (mut contender, mut contender_input, contender_output) = start_runtime(temporary.path());
    let selected = send(
        &mut contender_input,
        &contender_output,
        HostCommand::SelectActor {
            version: HOST_COMMAND_VERSION,
            request_id: "select-owned-runtime".into(),
            actor_ref: crate::test_refs::actor_ref("person").as_str().into(),
        },
    );
    assert!(
        matches!(selected, HostEvent::ActorSelected { request_id, actor, .. }
        if request_id == "select-owned-runtime" && actor.actor_ref == crate::test_refs::actor_ref("person").as_str())
    );
    drop(contender_input);
    assert!(contender
        .wait_for_exit("wait for second resident client")
        .success());

    let rejected = send(
        &mut input,
        &output,
        HostCommand::CreateConversation {
            version: HOST_COMMAND_VERSION + 1,
            request_id: "unsupported-version".into(),
            source_ref: SOURCE_REF.into(),
            author_actor_ref: crate::test_refs::actor_ref("person").as_str().into(),
            text: "Keep this note".into(),
        },
    );
    assert!(
        matches!(rejected, HostEvent::CommandFailed { request_id, error }
        if request_id == "unsupported-version" && error.contains("unsupported host command version"))
    );

    let rejected = send(
        &mut input,
        &output,
        HostCommand::CreateConversation {
            version: HOST_COMMAND_VERSION,
            request_id: "unknown-source".into(),
            source_ref: "cwp://example.test/runtime/sources/missing".into(),
            author_actor_ref: crate::test_refs::actor_ref("person").as_str().into(),
            text: "Keep this note".into(),
        },
    );
    assert!(
        matches!(rejected, HostEvent::CommandFailed { request_id, error }
        if request_id == "unknown-source" && error.contains("unknown conversation source"))
    );

    let rejected = send(
        &mut input,
        &output,
        HostCommand::CreateConversation {
            version: HOST_COMMAND_VERSION,
            request_id: "impersonate-helper".into(),
            source_ref: SOURCE_REF.into(),
            author_actor_ref: crate::test_refs::actor_ref("helper").as_str().into(),
            text: "Keep this note".into(),
        },
    );
    assert!(
        matches!(rejected, HostEvent::CommandFailed { request_id, error }
        if request_id == "impersonate-helper"
            && error.contains("cannot submit as"))
    );

    let result = send(
        &mut input,
        &output,
        HostCommand::CreateConversation {
            version: HOST_COMMAND_VERSION,
            request_id: "list-coworkers".into(),
            source_ref: SOURCE_REF.into(),
            author_actor_ref: crate::test_refs::actor_ref("person").as_str().into(),
            text: "@cwr list coworkers".into(),
        },
    );
    assert!(
        matches!(result, HostEvent::CommandSucceeded { request_id, receipt }
        if request_id == "list-coworkers" && receipt.invocation_ref.is_some())
    );

    write(
        &mut input,
        &HostCommand::ReloadGraph {
            version: HOST_COMMAND_VERSION,
            request_id: "reload".into(),
            source_ref: Some(SOURCE_REF.into()),
        },
    );
    let mut catalog_updated = false;
    loop {
        match receive(&output) {
            HostEvent::CatalogUpdated { actor, catalog } => {
                catalog_updated = actor.actor_ref == crate::test_refs::actor_ref("person").as_str()
                    && catalog.actors.iter().any(|actor| {
                        actor.actor_ref == crate::test_refs::actor_ref("helper").as_str()
                    });
            }
            HostEvent::CommandSucceeded { request_id, .. } if request_id == "reload" => break,
            _ => {}
        }
    }
    assert!(
        catalog_updated,
        "reload must publish the authoritative catalog"
    );

    drop(input);
    assert!(child
        .wait_for_exit("wait for host runtime after stdin closes")
        .success());
    assert!(runtime_directory(temporary.path())
        .join("runtime.sock")
        .exists());
    stop_resident(temporary.path());
}

#[test]
fn host_stdio_drains_pipelined_commands_before_eof_shutdown() {
    let temporary = tempfile::tempdir().expect("temporary managed runtime");
    let (mut child, mut input, output) = start_runtime(temporary.path());
    let selected = send(
        &mut input,
        &output,
        HostCommand::SelectActor {
            version: HOST_COMMAND_VERSION,
            request_id: "select-for-eof-drain".into(),
            actor_ref: crate::test_refs::actor_ref("person").as_str().into(),
        },
    );
    assert!(matches!(selected, HostEvent::ActorSelected { actor, .. }
        if actor.actor_ref == crate::test_refs::actor_ref("person").as_str()));
    while !matches!(receive(&output), HostEvent::ConversationSnapshot { .. }) {}

    write_batch(
        &mut input,
        &[
            HostCommand::CreateConversation {
                version: HOST_COMMAND_VERSION,
                request_id: "first-before-eof".into(),
                source_ref: SOURCE_REF.into(),
                author_actor_ref: crate::test_refs::actor_ref("person").as_str().into(),
                text: "Retain the first EOF note".into(),
            },
            HostCommand::CreateConversation {
                version: HOST_COMMAND_VERSION,
                request_id: "second-before-eof".into(),
                source_ref: SOURCE_REF.into(),
                author_actor_ref: crate::test_refs::actor_ref("person").as_str().into(),
                text: "Retain the second EOF note".into(),
            },
        ],
    );
    drop(input);

    assert!(child.wait_for_exit("wait for EOF command drain").success());
    let mut completed = [false; 2];
    while !completed.iter().all(|completed| *completed) {
        if let HostEvent::CommandSucceeded {
            request_id,
            receipt,
        } = receive(&output)
        {
            let index = match request_id.as_str() {
                "first-before-eof" => Some(0),
                "second-before-eof" => Some(1),
                _ => None,
            };
            if let Some(index) = index {
                assert!(receipt.message_ref.is_some());
                completed[index] = true;
            }
        }
    }
    stop_resident(temporary.path());
}

#[test]
fn host_stdio_preserves_a_command_pipelined_after_actor_selection() {
    let temporary = tempfile::tempdir().expect("temporary managed runtime");
    let (mut child, mut input, output) = start_runtime(temporary.path());
    write_batch(
        &mut input,
        &[
            HostCommand::SelectActor {
                version: HOST_COMMAND_VERSION,
                request_id: "select-pipelined".into(),
                actor_ref: crate::test_refs::actor_ref("person").as_str().into(),
            },
            HostCommand::CreateConversation {
                version: HOST_COMMAND_VERSION,
                request_id: "pipelined-conversation".into(),
                source_ref: SOURCE_REF.into(),
                author_actor_ref: crate::test_refs::actor_ref("person").as_str().into(),
                text: "Retain this pipelined note".into(),
            },
        ],
    );

    let mut selected = false;
    let mut admitted = false;
    while !(selected && admitted) {
        match receive(&output) {
            HostEvent::ActorSelected { request_id, .. } => {
                selected = request_id == "select-pipelined";
            }
            HostEvent::CommandSucceeded {
                request_id,
                receipt,
            } => {
                admitted = request_id == "pipelined-conversation" && receipt.message_ref.is_some();
            }
            _ => {}
        }
    }

    drop(input);
    assert!(child
        .wait_for_exit("wait for pipelined host runtime")
        .success());
    stop_resident(temporary.path());
}

fn send(input: &mut ChildStdin, output: &Receiver<String>, command: HostCommand) -> HostEvent {
    write(input, &command);
    loop {
        let event = receive(output);
        let request_id = match &event {
            HostEvent::WorkspaceInspected { request_id, .. }
            | HostEvent::ActorSelected { request_id, .. }
            | HostEvent::RootConversationsPage { request_id, .. }
            | HostEvent::IntrinsicResponse { request_id, .. }
            | HostEvent::CommandSucceeded { request_id, .. }
            | HostEvent::CommandFailed { request_id, .. } => request_id,
            _ => continue,
        };
        if request_id == command.request_id() {
            return event;
        }
    }
}

fn write(input: &mut ChildStdin, command: &HostCommand) {
    serde_json::to_writer(&mut *input, command).expect("encode host command");
    input.write_all(b"\n").expect("terminate host command");
    input.flush().expect("flush host command");
}

fn write_batch(input: &mut ChildStdin, commands: &[HostCommand]) {
    let mut frames = Vec::new();
    for command in commands {
        serde_json::to_writer(&mut frames, command).expect("encode pipelined host command");
        frames.push(b'\n');
    }
    input
        .write_all(&frames)
        .expect("write pipelined host commands");
    input.flush().expect("flush pipelined host commands");
}

fn receive(output: &Receiver<String>) -> HostEvent {
    let line = output
        .recv_timeout(Duration::from_secs(30))
        .expect("bounded runtime response");
    let event: HostEvent = serde_json::from_str(&line).expect("typed host event");
    event.validate().expect("valid host event");
    event
}

fn resident_diagnostic_text(path: impl AsRef<std::path::Path>) -> cwr::CwrResult<String> {
    let mut db = cwr::cwpdb::NativeStore::open_read_only(path)?;
    let page = db.diagnostic_records(None, 100)?;
    if page.truncated {
        return Err("diagnostic fixture exceeds page bound".into());
    }
    Ok(page
        .records
        .into_iter()
        .map(|record| record.value.to_string())
        .collect::<Vec<_>>()
        .join("\n"))
}
