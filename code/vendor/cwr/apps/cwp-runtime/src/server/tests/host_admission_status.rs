use super::*;
use cwr::adapter::ExecutionPhase;
use cwr::v2026_09::{
    host::{measure_turn_status_projection, ConversationTurnState},
    invocation::ActionInvocationState,
    local::{load_default_local_acting_actor_ref, LocalRuntimeOptions},
    upstream::ConversationContextPolicy,
};
use std::{num::NonZeroUsize, os::unix::fs::PermissionsExt, process::Command};

const TEST_NAME: &str =
    "server::tests::host_admission_status::host_created_root_is_published_while_its_creator_is_running";
const CHILD_CALLBACK_TEST_NAME: &str =
    "server::tests::host_admission_status::synchronous_child_callback_publishes_its_parents_independent_admission";
const CHILD_ENV: &str = "CWR_HOST_ADMISSION_STATUS_TEST_CHILD";

#[test]
fn host_created_root_is_published_while_its_creator_is_running() {
    exercise_host_admission("own", TEST_NAME);
}

#[test]
fn synchronous_child_callback_publishes_its_parents_independent_admission() {
    exercise_host_admission("delegate", CHILD_CALLBACK_TEST_NAME);
}

fn exercise_host_admission(mode: &str, test_name: &str) {
    if !run_isolated_test(test_name, CHILD_ENV) {
        return;
    }
    let temporary = tempfile::tempdir().unwrap();
    let runtime = fixture_runtime(temporary.path());
    let person = runtime.catalog().unwrap().acting_actor.actor_ref;
    let admit = |event, input| {
        runtime
            .admit_action_as(
                "cwp://example.test/integrations/fixture",
                event,
                &person,
                None,
                "@helper--run--task",
                input,
            )
            .unwrap()
    };
    let a = admit("creator", serde_json::json!({"mode":mode}));
    let c = admit("unrelated", serde_json::json!({}));
    let scaffold = tempfile::tempdir().unwrap();
    let state = runtime_state(&scaffold);
    lock_state(&state).unwrap().runtime = runtime.clone();
    broadcast_turn_statuses(&state, &runtime).unwrap();
    let c_status =
        lock_state(&state).unwrap().published_turn_statuses[c.conversation_ref()].clone();
    let (client, server) = wire::wire_pair();
    client
        .set_read_timeout(Some(Duration::from_secs(15)))
        .unwrap();
    let writer = client_writer(server).unwrap();
    subscribe(&state, &writer).unwrap();
    let reader = observe_created_root(
        client,
        runtime.clone(),
        a.clone(),
        c.conversation_ref().into(),
        temporary.path().into(),
        mode == "delegate",
    );
    let task = runtime.try_schedule_execution().unwrap().unwrap();
    assert_eq!(task.record().invocation_ref, a.invocation_ref().unwrap());
    let (_, work) = measure_turn_status_projection(|| {
        scheduler::execute(Arc::clone(&state), runtime.clone(), task);
    });
    broadcast_execution_failure(
        &state,
        a.conversation_ref(),
        "host-admission-sentinel",
        "fixture",
        "end fixture",
    )
    .unwrap();
    let (created, events) = reader.join().unwrap();
    disconnect_writer(&writer);
    let created = created.unwrap_or_else(|| {
        panic!(
            "independent pending root was not published before creator completion; host response: {:?}; events: {events:?}",
            fs::read_to_string(temporary.path().join("create-response.json"))
        )
    });
    assert!(temporary.path().join("observed-created").exists());
    assert!(temporary.path().join("observed-followup").exists());
    if mode == "delegate" {
        assert!(temporary.path().join("parent-resumed").exists());
    }
    assert!(events.iter().any(|event| matches!(event,
        RuntimeServiceEvent::ConversationExecutionFinished { response, .. }
            if response.invocation_ref == a.invocation_ref().unwrap() && response.status.as_u16() == 200)));
    let published = &lock_state(&state).unwrap().published_turn_statuses;
    assert_eq!(published[&created][0].state, ConversationTurnState::Pending);
    assert_eq!(published[c.conversation_ref()], c_status);
    let created_calls = work
        .iter()
        .filter(|(reference, _)| reference == &created)
        .count();
    let unrelated_calls = work
        .iter()
        .filter(|(reference, _)| reference == c.conversation_ref())
        .count();
    // A synchronous invocation has two independent-effect publication
    // boundaries: the parent's admission seen by a child callback, then the
    // parent invoke's completed HostResult after the child returns. The child
    // activity callbacks between those boundaries must remain selective.
    let expected_calls = if mode == "delegate" { 2 } else { 1 };
    assert_eq!(
        created_calls, expected_calls,
        "created root projection count did not match effect boundaries: {work:?}"
    );
    assert_eq!(
        unrelated_calls, expected_calls,
        "unrelated root projection count did not match effect boundaries: {work:?}"
    );
    println!("host admission status mode={mode}: independent and unrelated roots each projected {expected_calls} times at required effect boundaries; intervening tool activities remained selective");
    assert_eq!(
        *published,
        runtime.conversation_host().turn_statuses().unwrap()
    );
}

pub(super) fn run_isolated_test(test_name: &str, child_env: &str) -> bool {
    if std::env::var_os(child_env).is_some() {
        return true;
    }
    let output = Command::new(std::env::current_exe().unwrap())
        .args([test_name, "--exact", "--nocapture", "--test-threads=1"])
        .env(child_env, "1")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    println!("{stdout}");
    assert!(output.status.success(), "{stdout}\n{stderr}");
    false
}

fn observe_created_root(
    client: UnixStream,
    runtime: LocalRuntime,
    a: LocalAdmission,
    c: String,
    directory: PathBuf,
    delegated: bool,
) -> thread::JoinHandle<(Option<String>, Vec<RuntimeServiceEvent>)> {
    thread::spawn(move || {
        let mut created = None;
        let mut events = Vec::new();
        let assert_parent_held = || {
            assert_eq!(
                runtime.turn(&a.record).unwrap().invocation.unwrap().state,
                ActionInvocationState::Running
            );
            if delegated {
                assert!(
                    !directory.join("parent-resumed").exists(),
                    "parent returned from the synchronous call before child observation"
                );
            }
        };
        for line in BufReader::new(client).lines() {
            let event: RuntimeServiceEvent = serde_json::from_str(&line.unwrap()).unwrap();
            if let RuntimeServiceEvent::ConversationTurnStatusSnapshot {
                conversation_ref,
                turns,
            } = &event
            {
                if conversation_ref == a.conversation_ref()
                    && turns
                        .iter()
                        .flat_map(|turn| &turn.workers)
                        .any(|worker| worker.phase == ExecutionPhase::RunningCommand)
                {
                    assert_parent_held();
                    fs::write(directory.join("observed-followup"), "observed").unwrap();
                }
                if conversation_ref != a.conversation_ref()
                    && conversation_ref != &c
                    && !turns.is_empty()
                {
                    assert_eq!(turns[0].state, ConversationTurnState::Pending);
                    assert_parent_held();
                    // The creator owns the only execution permit. The created
                    // root has no worker callback to publish its own admission.
                    assert!(runtime.try_schedule_execution().unwrap().is_none());
                    let admission = runtime
                        .admission_record(
                            "cwp://!cwr/integrations/runtime",
                            &format!("{}#create-root", a.invocation_ref().unwrap()),
                        )
                        .unwrap()
                        .unwrap();
                    let record = runtime.turn(&admission).unwrap().invocation.unwrap();
                    assert!(record.parent_invocation_ref.is_none());
                    assert_eq!(record.state, ActionInvocationState::Admitted);
                    created = Some(conversation_ref.clone());
                    fs::write(directory.join("observed-created"), "observed").unwrap();
                }
            }
            let finished = matches!(&event,
                RuntimeServiceEvent::ConversationExecutionFailed { invocation_ref, .. }
                    if invocation_ref == "host-admission-sentinel");
            events.push(event);
            if finished {
                return (created, events);
            }
        }
        panic!("host admission fixture ended without its sentinel")
    })
}

pub(super) fn fixture_runtime(root: &Path) -> LocalRuntime {
    let workspace = root.join("workspace");
    copy_fixture(fixture_workspace().parent().unwrap(), &workspace);
    fs::write(workspace.join("coworkers/helper/integration.toml"),
        "type='cwp://org.coworkerprotocol/2026-09/schemas/integration'\n[adapter]\nref='cwp://org.coworkerprotocol/2026-09/adapters/sh'\n").unwrap();
    fs::write(workspace.join("coworkers/helper/ROLE.toml"),
        "type='cwp://org.coworkerprotocol/2026-09/schemas/role'\n[[adapters]]\nref='cwp://org.coworkerprotocol/2026-09/adapters/sh'\n[[actions]]\nverb='run'\nobject='task'\ntitle='Run fixture'\ndescription='Run the host admission fixture'\n[actions.input_schema]\npath='./input.json'\n[actions.output_schema]\npath='./output.json'\n").unwrap();
    let role = workspace.join("coworkers/helper/ROLE.toml");
    let mut source = fs::read_to_string(&role).unwrap();
    source.push_str("\n[[actions]]\nverb='respond_to'\nobject='message'\ntitle='Respond to fixture'\ndescription='Receive independently created fixture work'\n[actions.input_schema]\npath='./message-input.json'\n[actions.output_schema]\npath='./message-output.json'\n");
    fs::write(role, source).unwrap();
    for schema in ["input.json", "output.json"] {
        fs::write(
            workspace.join("coworkers/helper").join(schema),
            r#"{"type":"object"}"#,
        )
        .unwrap();
    }
    fs::write(workspace.join("coworkers/helper/message-input.json"),
        r#"{"type":"object","required":["source_message_ref"],"properties":{"source_message_ref":{"type":"string"}}}"#).unwrap();
    fs::write(
        workspace.join("coworkers/helper/message-output.json"),
        r#"{"type":"object","required":["text"],"properties":{"text":{"type":"string"}}}"#,
    )
    .unwrap();
    let child = workspace.join("coworkers/child");
    copy_fixture(&workspace.join("coworkers/helper"), &child);
    let child_source = child.join("coworker.toml");
    let source = fs::read_to_string(&child_source)
        .unwrap()
        .replace("name = \"Helper\"", "name = \"Child\"")
        .replace("slug = \"helper\"", "slug = \"child\"");
    fs::write(child_source, source).unwrap();
    let manifest = workspace.join("CWR.toml");
    let mut source = fs::read_to_string(&manifest).unwrap();
    source.push_str("\n[[imports]]\npath='./coworkers/child/coworker.toml'\nas='child'\n");
    fs::write(&manifest, source).unwrap();
    let adapter = root.join("fixture-adapter");
    fs::write(&adapter, include_str!("host_admission_status_adapter.py")).unwrap();
    fs::set_permissions(&adapter, fs::Permissions::from_mode(0o700)).unwrap();
    std::env::set_var("CWR_SH_ADAPTER_EXECUTABLE", &adapter);
    let runtime = LocalRuntime::open_with_state_path_and_runtime_ref_and_options(
        &manifest,
        root.join("state.sqlite"),
        "cwp://example.test/runtime",
        LocalHostIdentity::new(
            load_default_local_acting_actor_ref(&manifest).unwrap(),
            "cwp://example.test/integrations/fixture",
        ),
        LocalRuntimeOptions {
            max_concurrent_conversations: NonZeroUsize::MIN,
            ..Default::default()
        },
    )
    .unwrap();
    let actors = runtime
        .catalog()
        .unwrap()
        .known_actors
        .into_iter()
        .map(|actor| actor.actor_ref)
        .collect();
    runtime
        .configure_conversation_contexts(Some(ConversationContextPolicy {
            action_permissions: vec![],
            suspended: false,
            default_context_ref: "workspace".into(),
            contexts: BTreeMap::from([("workspace".into(), actors)]),
        }))
        .unwrap();
    runtime
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
