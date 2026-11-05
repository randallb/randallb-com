use super::*;
use cwr::v2026_09::local::LocalHostIdentity;
use std::{
    fs,
    io::{BufRead, BufReader},
    path::PathBuf,
    process::{Command, Stdio},
    sync::Mutex,
};
static ENVIRONMENT: Mutex<()> = Mutex::new(());
struct Supervisor(std::process::Child);
impl Drop for Supervisor {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn copy(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let target = destination.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}
fn fixture(root: &Path) -> PathBuf {
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../packages/cwr-core/fixtures/contracts/agent-recovery-turn/workspace");
    let workspace = root.join("workspace");
    copy(&source, &workspace);
    let helper = workspace.join("coworkers/helper");
    fs::write(helper.join("integration.toml"),"type='cwp://org.coworkerprotocol/2026-09/schemas/integration'\n[adapter]\nref='cwp://org.coworkerprotocol/2026-09/adapters/loopback'\n").unwrap();
    fs::write(
        helper.join("ROLE.toml"),
        r#"type='cwp://org.coworkerprotocol/2026-09/schemas/role'
name='Fixture cloud helper'
[[adapters]]
ref='cwp://org.coworkerprotocol/2026-09/adapters/loopback'
[adapters.params.host_call]
operation='cwr.send_request'
[adapters.params.host_call.input]
type='cwp://org.coworkerprotocol/2026-09/schemas/gateway_request'
resource_ref='~@/fixture_gateway'
[adapters.params.host_call.input.request]
read='fictional-school'
[[actions]]
verb='read'
object='fixture'
title='Read fixture'
description='Read a fictional authorized gateway'
[actions.input_schema]
path='./input.json'
[actions.output_schema]
path='./output.json'
"#,
    )
    .unwrap();
    fs::write(helper.join("input.json"), "{\"type\":\"object\"}").unwrap();
    fs::write(helper.join("output.json"), "{\"type\":\"object\"}").unwrap();
    let mut definition = fs::read_to_string(helper.join("coworker.toml")).unwrap();
    definition.push_str("\n[integrations.params]\nexecution_environment='workloop_cloud'\n");
    fs::write(helper.join("coworker.toml"), definition).unwrap();
    let gateway = workspace.join("coworkers/gateway");
    fs::create_dir(&gateway).unwrap();
    fs::write(gateway.join("GATEWAY.toml"),"type='cwp://org.coworkerprotocol/2026-09/schemas/gateway'\nrole={ref='~@/behavior'}\nintegration={ref='~@/execution'}\naction_name='handle--request'\n[[imports]]\npath='./ROLE.toml'\nas='behavior'\n[[imports]]\npath='./integration.toml'\nas='execution'\n").unwrap();
    fs::write(
        gateway.join("integration.toml"),
        fs::read_to_string(helper.join("integration.toml")).unwrap(),
    )
    .unwrap();
    fs::write(gateway.join("ROLE.toml"),"type='cwp://org.coworkerprotocol/2026-09/schemas/role'\n[[adapters]]\nref='cwp://org.coworkerprotocol/2026-09/adapters/loopback'\n[[actions]]\nverb='handle'\nobject='request'\ntitle='Read fictional fixture'\ndescription='No external service or customer data'\n[actions.input_schema]\npath='./input.json'\n[actions.output_schema]\npath='./output.json'\n").unwrap();
    fs::write(gateway.join("input.json"), "{\"type\":\"object\"}").unwrap();
    fs::write(gateway.join("output.json"), "{\"type\":\"object\"}").unwrap();
    let manifest = workspace.join("CWR.toml");
    let mut text = fs::read_to_string(&manifest).unwrap();
    text.push_str("\n[[imports]]\npath='./coworkers/gateway/GATEWAY.toml'\nas='fixture_gateway'\n");
    fs::write(&manifest, text).unwrap();
    manifest
}
#[test]
fn normal_cwr_action_delegates_through_native_cloud_service_then_uses_replacement_executor() {
    run_native_fixture(false, false);
}
#[test]
fn real_open_responses_adapter_streams_tools_through_native_cwr_and_credential_gateway() {
    run_native_fixture(true, false);
}
#[test]
fn real_streamed_burst_survives_slow_durable_journal_and_preserves_replacement() {
    run_native_fixture(true, true);
}
#[path = "tests/slow_journal.rs"]
mod slow_journal;
fn run_native_fixture(model: bool, slow: bool) {
    let _environment = ENVIRONMENT
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let root = tempfile::tempdir().unwrap();
    let manifest = fixture(root.path());
    let state = root.path().join("cwpdb.sqlite3");
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let loopback = repository.join("target/debug/cwr-adapter-loopback");
    let proxy = repository.join("target/debug/cwr-adapter-cloud");
    assert!(
        loopback.is_file() && proxy.is_file(),
        "Build native cloud and loopback adapters before this integration check"
    );
    let mut supervisor = Supervisor(
        Command::new("deno")
            .args([
                "run",
                "-A",
                "experimental/workloop-cloud-execution/fixture_host.ts",
            ])
            .current_dir(&repository)
            .env("CWR_FIXTURE_COORDINATOR_URL", &origin)
            .env("CWR_FIXTURE_LOOPBACK_EXECUTABLE", &loopback)
            .env("CWR_FIXTURE_STREAM_DELTAS", if slow { "120" } else { "0" })
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap(),
    );
    let mut line = String::new();
    BufReader::new(supervisor.0.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let transport: Value = serde_json::from_str(&line).unwrap();
    let executor_origin = transport["url"].as_str().unwrap();
    if model {
        configure_model_fixture(&manifest, executor_origin);
    }
    let runtime = LocalRuntime::open_with_state_path_and_runtime_ref(
        &manifest,
        &state,
        "cwp://fixture.test/native-cloud",
        LocalHostIdentity::new(
            cwr::v2026_09::local::load_default_local_acting_actor_ref(&manifest).unwrap(),
            "cwp://fixture.test/native-ingress",
        ),
    )
    .unwrap();
    let config = CloudServiceConfig {
        workspace_ref: "cwp://fixture.test/workspaces/one".into(),
        coordinator_actor_ref: runtime.catalog().unwrap().acting_actor.actor_ref,
        coordinator_token: "coordinator-token-at-least-thirty-two-bytes".into(),
        assignment_signing_key: vec![3; 32],
        executors: vec![
            CloudExecutorEndpoint {
                executor_ref: "cwp://fixture.test/executors/a".into(),
                url: executor_origin.into(),
            },
            CloudExecutorEndpoint {
                executor_ref: "cwp://fixture.test/executors/b".into(),
                url: executor_origin.into(),
            },
        ],
        assignment_lifetime_ms: 60_000,
        model: if model {
            Some(cwr::cloud_execution::CloudModelGrant {
                model: "fixture/model".into(),
                endpoint: format!("{executor_origin}/fake-responses"),
                request_budget: 4,
                max_output_tokens: 64,
            })
        } else {
            None
        },
    };
    let journal = slow_journal::SlowJournal::open(&state, slow);
    let frame_proof = journal.frames.clone();
    let service = Arc::new(CloudHttpService::new(runtime.clone(), journal, config).unwrap());
    let server = start(listener, service).unwrap();
    let variables = [
        ("CWR_CLOUD_COORDINATOR_URL", origin.clone()),
        (
            "CWR_CLOUD_COORDINATOR_TOKEN",
            "coordinator-token-at-least-thirty-two-bytes".into(),
        ),
        (
            "CWR_CLOUD_WORKSPACE_REF",
            "cwp://fixture.test/workspaces/one".into(),
        ),
        ("CWR_CLOUD_ADAPTER_EXECUTABLE", proxy.display().to_string()),
        (
            "CWR_LOOPBACK_ADAPTER_EXECUTABLE",
            loopback.display().to_string(),
        ),
    ];
    let previous: Vec<_> = variables
        .iter()
        .map(|(key, _)| (*key, std::env::var_os(key)))
        .collect();
    for (key, value) in &variables {
        std::env::set_var(key, value);
    }
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let conversation = runtime
            .admit_conversation_text(None, "Fictional school context retained on the coordinator")
            .unwrap();
        let first = runtime
            .admit_action(
                Some(conversation.conversation_ref()),
                "@helper--read--fixture",
                json!({"first":true}),
            )
            .unwrap();
        let person = runtime.catalog().unwrap().acting_actor.actor_ref;
        let proof = runtime
            .authorize_cloud_host_work(
                first.invocation_ref().unwrap(),
                &person,
                conversation.conversation_ref(),
            )
            .unwrap();
        assert_eq!(proof["source_message_ref"], first.record.source_message_ref);
        assert!(runtime
            .authorize_cloud_host_work(
                first.invocation_ref().unwrap(),
                "cwp://forged.test/person",
                conversation.conversation_ref()
            )
            .is_err());
        let turn = runtime.execute_admission(&first, |_| {}).unwrap();
        assert_eq!(
            turn.invocation.as_ref().unwrap().state,
            cwr::v2026_09::invocation::ActionInvocationState::Completed,
            "{:?}",
            turn.invocation
        );
        let second = runtime
            .admit_action(
                Some(conversation.conversation_ref()),
                "@helper--read--fixture",
                json!({"second":true}),
            )
            .unwrap();
        let turn = runtime.execute_admission(&second, |_| {}).unwrap();
        assert_eq!(
            turn.invocation.as_ref().unwrap().state,
            cwr::v2026_09::invocation::ActionInvocationState::Completed,
            "{:?}",
            turn.invocation
        );
        assert_replacement_proof(
            &runtime,
            conversation.conversation_ref(),
            executor_origin,
            (model, slow),
            &frame_proof,
        );
    }));
    for (key, value) in previous {
        if let Some(value) = value {
            std::env::set_var(key, value);
        } else {
            std::env::remove_var(key);
        }
    }
    let _ = supervisor.0.kill();
    let _ = supervisor.0.wait();
    drop(server);
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

fn configure_model_fixture(manifest: &Path, executor_origin: &str) {
    let helper = manifest.parent().unwrap().join("coworkers/helper");
    let role = fs::read_to_string(helper.join("ROLE.toml")).unwrap();
    let role = role.replace(
        "cwp://org.coworkerprotocol/2026-09/adapters/loopback",
        "cwp://org.coworkerprotocol/2026-09/adapters/open-responses",
    );
    let a = role.find("[adapters.params.host_call]").unwrap();
    let b = role.find("[[actions]]").unwrap();
    let role=format!("{}[adapters.params]\nentrypoint='./PROMPT.md'\nmodel='fixture/model'\nmax_output_tokens=64\nmax_turns=4\ntimeout_seconds=30\n{}",&role[..a],&role[b..]);
    fs::write(helper.join("ROLE.toml"), role).unwrap();
    fs::write(helper.join("integration.toml"),format!("type='cwp://org.coworkerprotocol/2026-09/schemas/integration'\n[adapter]\nref='cwp://org.coworkerprotocol/2026-09/adapters/open-responses'\n[params]\nendpoint='{executor_origin}/fake-responses'\n")).unwrap();
}

fn assert_replacement_proof(
    runtime: &LocalRuntime,
    conversation_ref: &str,
    executor_origin: &str,
    flags: (bool, bool),
    frame_proof: &slow_journal::Frames,
) {
    let (model, slow) = flags;
    let inspection: Value = reqwest::blocking::get(format!("{executor_origin}/inspect"))
        .unwrap()
        .json()
        .unwrap();
    assert_eq!(
        inspection["started"],
        json!([
            "cwp://fixture.test/executors/a",
            "cwp://fixture.test/executors/b"
        ])
    );
    assert_eq!(inspection["destroyed"], inspection["started"]);
    assert_eq!(inspection["active"], 0);
    assert_eq!(inspection["modelCalls"], if model { 4 } else { 0 });
    if slow {
        assert_eq!(
            inspection["streamedDeltas"], 240,
            "both actual Responses calls must deliver the declared streaming burst"
        );
        let frames = frame_proof.lock().unwrap();
        assert!(
            frames.len() >= 240,
            "SSE must reach actual durable ModelOutput appends"
        );
        let unique = frames.iter().collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            unique.len(),
            frames.len(),
            "event GET retries must not concurrently re-ingest frames"
        );
    }
    let retained = runtime.conversation(conversation_ref).unwrap().unwrap();
    assert!(retained
        .messages
        .iter()
        .any(|message| message.text.contains("Fictional school context")));
    let records = runtime.host_conversations().unwrap();
    assert!(records.iter().any(|conversation|conversation.activity.iter().any(|activity|matches!(&activity.event,cwr::v2026_09::conversation::ConversationActivityEvent::HostCall {operation,..} if operation=="cwr.send_request"))));
}
