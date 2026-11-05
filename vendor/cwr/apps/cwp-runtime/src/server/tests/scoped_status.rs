use super::*;
use cwr::{
    adapter::{ExecutionActivity, ExecutionPhase, ExecutionPhaseState},
    v2026_09::{
        conversation::ConversationAdmissionStore,
        host::{measure_turn_status_projection, ConversationTurnState},
        local::load_default_local_acting_actor_ref,
    },
};
use std::{os::unix::fs::PermissionsExt, process::Command};

const TEST_NAME: &str =
    "server::tests::scoped_status::execution_callback_skips_unrelated_worker_histories";
const CHILD_ENV: &str = "CWR_SCOPED_STATUS_TEST_CHILD";

#[test]
fn execution_callback_skips_unrelated_worker_histories() {
    // Adapter executable selection is process-wide. Keep the override out of
    // the parallel test runner and exercise only this fixture in the child.
    if std::env::var_os(CHILD_ENV).is_none() {
        let output = Command::new(std::env::current_exe().unwrap())
            .args([TEST_NAME, "--exact", "--nocapture", "--test-threads=1"])
            .env(CHILD_ENV, "1")
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        println!("{stdout}");
        assert!(output.status.success(), "{stdout}\n{stderr}");
        return;
    }

    let small = exercise_execution(4);
    let large = exercise_execution(128);
    assert!(large.full_history_items > small.full_history_items);
    assert_eq!(large.selected_history_items, small.selected_history_items);
    assert_eq!(large.execution_history_items, small.execution_history_items);
    assert_eq!(
        large.execution_projection_calls,
        small.execution_projection_calls
    );
    println!(
        "scoped execution comparison: unrelated activities 4 -> 128; full snapshot history items {} -> {}; selected snapshot history items {} -> {}; actual execution history items {} -> {}; actual execution projector calls {} -> {}; unrelated execution projections 0 -> 0",
        small.full_history_items, large.full_history_items,
        small.selected_history_items, large.selected_history_items,
        small.execution_history_items, large.execution_history_items,
        small.execution_projection_calls, large.execution_projection_calls,
    );
}

struct ProjectionWork {
    full_history_items: usize,
    selected_history_items: usize,
    execution_history_items: usize,
    execution_projection_calls: usize,
}

fn exercise_execution(unrelated_history: usize) -> ProjectionWork {
    let (temporary, runtime, a, b) = execution_fixture(unrelated_history);
    let scaffold = tempfile::tempdir().unwrap();
    let state = runtime_state(&scaffold);
    lock_state(&state).unwrap().runtime = runtime.clone();
    let (before, full_work) =
        measure_turn_status_projection(|| runtime.conversation_host().turn_statuses().unwrap());
    assert_eq!(full_work.len(), 2);
    assert_eq!(
        before[b.conversation_ref()][0].state,
        ConversationTurnState::Running
    );
    let (selected, selected_work) = measure_turn_status_projection(|| {
        runtime
            .conversation_host()
            .turn_statuses_affected_by(
                &BTreeSet::from([a.invocation_ref().unwrap().to_owned()]),
                &BTreeSet::from([a.conversation_ref().to_owned()]),
            )
            .unwrap()
    });
    assert_eq!(selected.len(), 1);
    assert_eq!(selected[a.conversation_ref()], before[a.conversation_ref()]);
    assert_eq!(selected_work.len(), 1);
    assert_eq!(selected_work[0].0, a.conversation_ref());
    broadcast_turn_statuses(&state, &runtime).unwrap();
    let b_status =
        lock_state(&state).unwrap().published_turn_statuses[b.conversation_ref()].clone();

    let (client, server) = wire::wire_pair();
    let writer = client_writer(server).unwrap();
    subscribe(&state, &writer).unwrap();
    let reader = observed_events(
        client,
        a.conversation_ref().to_owned(),
        temporary.path().into(),
    );
    let task = runtime.try_schedule_execution().unwrap().unwrap();
    assert_eq!(task.record().invocation_ref, a.invocation_ref().unwrap());
    let (_, execution_work) = measure_turn_status_projection(|| {
        scheduler::execute(Arc::clone(&state), runtime.clone(), task);
    });
    broadcast_execution_failure(
        &state,
        a.conversation_ref(),
        "scoped-status-sentinel",
        "fixture",
        "end fixture",
    )
    .unwrap();
    let events = reader.join().unwrap();
    disconnect_writer(&writer);

    assert!(
        temporary.path().join("observed-tool").exists(),
        "tool status was not delivered: {events:?}"
    );
    assert!(
        temporary.path().join("observed-progress").exists(),
        "progress status was not delivered: {events:?}"
    );
    assert!(events.iter().any(|event| matches!(event,
        RuntimeServiceEvent::ConversationExecutionFinished { response, .. }
            if response.invocation_ref == a.invocation_ref().unwrap() && response.status.as_u16() == 200)));
    assert!(events.iter().any(|event| matches!(event,
        RuntimeServiceEvent::ConversationTurnStatusSnapshot { conversation_ref, turns }
            if conversation_ref == a.conversation_ref() && turns.is_empty())));
    assert!(!execution_work.is_empty());
    assert!(
        execution_work
            .iter()
            .all(|(reference, _)| reference == a.conversation_ref()),
        "unrelated worker history projected: {execution_work:?}"
    );
    let published = &lock_state(&state).unwrap().published_turn_statuses;
    assert!(!published.contains_key(a.conversation_ref()));
    assert_eq!(published[b.conversation_ref()], b_status);
    assert_eq!(
        *published,
        runtime.conversation_host().turn_statuses().unwrap()
    );
    ProjectionWork {
        full_history_items: full_work.iter().map(|(_, items)| items).sum(),
        selected_history_items: selected_work.iter().map(|(_, items)| items).sum(),
        execution_history_items: execution_work.iter().map(|(_, items)| items).sum(),
        execution_projection_calls: execution_work.len(),
    }
}

fn copy_fixture(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        if entry.file_name() == ".cwr" {
            continue;
        }
        let target = destination.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_fixture(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn execution_fixture(
    unrelated_history: usize,
) -> (
    tempfile::TempDir,
    LocalRuntime,
    LocalAdmission,
    LocalAdmission,
) {
    let temporary = tempfile::tempdir().unwrap();
    let workspace = temporary.path().join("workspace");
    copy_fixture(fixture_workspace().parent().unwrap(), &workspace);
    for path in [
        "coworkers/helper/integration.toml",
        "coworkers/helper/ROLE.toml",
    ] {
        let path = workspace.join(path);
        let source = fs::read_to_string(&path).unwrap();
        fs::write(path, source.replace("adapters/codex", "adapters/sh")).unwrap();
    }
    fs::write(workspace.join("coworkers/helper/ROLE.toml"),
        "type='cwp://org.coworkerprotocol/2026-09/schemas/role'\n[[adapters]]\nref='cwp://org.coworkerprotocol/2026-09/adapters/sh'\n[[actions]]\nverb='run'\nobject='task'\ntitle='Run fixture'\ndescription='Run the status fixture'\n[actions.input_schema]\npath='./input.json'\n[actions.output_schema]\npath='./output.json'\n").unwrap();
    for schema in ["input.json", "output.json"] {
        fs::write(
            workspace.join("coworkers/helper").join(schema),
            r#"{"type":"object"}"#,
        )
        .unwrap();
    }
    let adapter = temporary.path().join("fixture-adapter");
    fs::write(&adapter, include_str!("scoped_status_adapter.py")).unwrap();
    fs::set_permissions(&adapter, fs::Permissions::from_mode(0o700)).unwrap();
    std::env::set_var("CWR_SH_ADAPTER_EXECUTABLE", &adapter);

    let manifest = workspace.join("CWR.toml");
    let state_path = temporary.path().join("conversation-admission.json");
    let open_runtime = || {
        LocalRuntime::open_with_state_path_and_runtime_ref_and_options(
            &manifest,
            &state_path,
            "cwp://example.test/runtime",
            LocalHostIdentity::new(
                load_default_local_acting_actor_ref(&manifest).unwrap(),
                "cwp://example.test/integrations/runtime",
            ),
            cwr::v2026_09::local::LocalRuntimeOptions {
                conversation_timeline_rollout:
                    cwr::v2026_09::conversation::ConversationTimelineRollout::Disabled,
                ..Default::default()
            },
        )
        .unwrap()
    };
    let runtime = open_runtime();
    let person = load_default_local_acting_actor_ref(&manifest).unwrap();
    let a = runtime
        .admit_action_as(
            "cwp://example.test/integrations/fixture",
            "a",
            &person,
            None,
            "@helper--run--task",
            serde_json::json!({}),
        )
        .unwrap();
    let b = runtime
        .admit_action_as(
            "cwp://example.test/integrations/fixture",
            "b",
            &person,
            None,
            "@helper--run--task",
            serde_json::json!({}),
        )
        .unwrap();
    drop(runtime);
    {
        let mut store =
            ConversationAdmissionStore::open(&state_path, "cwp://example.test/runtime").unwrap();
        store
            .claim_invocation(b.invocation_ref().unwrap(), 10)
            .unwrap();
        for _ in 0..unrelated_history {
            store
                .append_execution_activity(
                    b.invocation_ref().unwrap(),
                    ConversationActivityEvent::ExecutionActivity {
                        activity: ExecutionActivity {
                            phase: ExecutionPhase::UsingTool,
                            state: ExecutionPhaseState::Started,
                            completed_parts: None,
                            total_parts: None,
                        },
                    },
                )
                .unwrap();
        }
    }
    let runtime = open_runtime();
    (temporary, runtime, a, b)
}

fn observed_events(
    client: UnixStream,
    a_ref: String,
    marker_directory: PathBuf,
) -> thread::JoinHandle<Vec<RuntimeServiceEvent>> {
    thread::spawn(move || {
        let mut events = Vec::new();
        for line in BufReader::new(client).lines() {
            let event: RuntimeServiceEvent = serde_json::from_str(&line.unwrap()).unwrap();
            mark_observed_status(&event, &a_ref, &marker_directory);
            let finished = matches!(&event,
                RuntimeServiceEvent::ConversationExecutionFailed { invocation_ref, .. }
                    if invocation_ref == "scoped-status-sentinel");
            events.push(event);
            if finished {
                break;
            }
        }
        events
    })
}

fn mark_observed_status(event: &RuntimeServiceEvent, a_ref: &str, directory: &Path) {
    let RuntimeServiceEvent::ConversationTurnStatusSnapshot {
        conversation_ref,
        turns,
    } = event
    else {
        return;
    };
    if conversation_ref != a_ref {
        return;
    }
    for worker in turns.iter().flat_map(|turn| &turn.workers) {
        let marker = match worker.phase {
            ExecutionPhase::UsingTool => Some("observed-tool"),
            ExecutionPhase::Responding => Some("observed-progress"),
            _ => None,
        };
        if let Some(marker) = marker {
            fs::write(directory.join(marker), "observed").unwrap();
        }
    }
}
