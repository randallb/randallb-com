//! Run explicitly through scripts/runtime_publication_replay.py; no performance threshold.
#[path = "publication_replay/support.rs"]
mod support;
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use support::{query, Fixture};
use tungstenite::Message;

const KINDS: [&str; 5] = [
    "progress",
    "tool_start",
    "stream",
    "tool_complete",
    "terminal",
];
fn unix_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos()
        .try_into()
        .unwrap()
}
fn load() -> Value {
    let mut values = [0.0; 3];
    // SAFETY: values holds three f64 entries; getloadavg writes at most that count.
    let count = unsafe { libc::getloadavg(values.as_mut_ptr(), 3) };
    json!({"load_1_5_15": &values[..count.max(0) as usize], "available_parallelism":std::thread::available_parallelism().map(|n|n.get()).ok()})
}
fn check(deadline: Instant, interrupted: &AtomicBool) {
    assert!(
        !interrupted.load(Ordering::Relaxed),
        "replay interrupted; cleaning fixture"
    );
    assert!(
        Instant::now() < deadline,
        "replay deadline exceeded; cleaning fixture"
    );
}
fn seed(fixture: &Fixture, workload: &Value, interrupted: &AtomicBool) {
    let client = fixture.client.as_ref().unwrap();
    let endpoint = fixture.endpoint.as_ref().unwrap();
    for conversation in 0..workload["unrelated_conversations"].as_u64().unwrap() {
        check(Instant::now() + Duration::from_secs(30), interrupted);
        let value = query(client, endpoint, "mutation($text:String!,$id:ID!){createConversation(text:$text,requestId:$id){conversationRef invocation{id}}}",json!({"text":format!("Synthetic retained conversation {conversation}, message 0"),"id":format!("seed-{conversation}-0")}));
        assert!(
            value["createConversation"]["invocation"].is_null(),
            "seeding must not start a worker: {value}"
        );
        let reference = &value["createConversation"]["conversationRef"];
        assert!(reference.is_string(), "{value}");
        for message in 1..workload["unrelated_messages"].as_u64().unwrap() {
            check(Instant::now() + Duration::from_secs(30), interrupted);
            let value = query(client,endpoint,"mutation($text:String!,$id:ID!,$ref:ID!){sendMessage(text:$text,requestId:$id,conversationRef:$ref){messageRef invocation{id}}}",json!({"ref":reference,"text":format!("Synthetic retained message {message}"),"id":format!("seed-{conversation}-{message}")}));
            assert!(value["sendMessage"]["invocation"].is_null(), "{value}");
        }
    }
}
fn admit(fixture: &Fixture, workers: u64, interrupted: &AtomicBool) -> Vec<Value> {
    let deadline = Instant::now() + Duration::from_secs(90);
    let invocations: Vec<Value> = (0..workers).map(|worker| {
        check(deadline,interrupted);
        let value = query(fixture.client.as_ref().unwrap(),fixture.endpoint.as_ref().unwrap(),
            "mutation($url:ID!,$input:JSON!,$id:ID!){invokeAction(url:$url,activation:\"@worker--run--task\",input:$input,requestId:$id){...on ActionInvocation{id conversationRef} ...on ActionError{code message}}}",
            json!({"url":fixture.endpoint.as_ref().unwrap().runtime_ref,"input":{"worker":worker},"id":format!("worker-{worker}")}));
        let invocation = value["invokeAction"].clone();
        assert!(invocation["id"].is_string(), "{value}");
        invocation
    }).collect();
    while !(0..workers).all(|worker| {
        fixture
            .root()
            .join(format!("worker-{worker}/ready"))
            .exists()
    }) {
        check(deadline, interrupted);
        std::thread::sleep(Duration::from_millis(5));
    }
    invocations
}
fn selection(invocations: &[Value]) -> String {
    let fields: String = invocations.iter().enumerate().map(|(worker,invocation)|format!(r#"
        w{worker}:invocation(ref:{reference}){{id state currentExecution{{progress}} outcome{{status summary}} resultMessage{{id text}} activities(last:1){{edges{{node{{id label state}}}}}}}}
        c{worker}:conversation(ref:{conversation}){{timeline(first:10){{edges{{node{{id children(first:10){{edges{{node{{id source{{...on ResolvedSource{{node{{__typename id ...on Message{{text}}}}}}}}}}}}}}}}}}}}}}
    "#, reference=invocation["id"],conversation=invocation["conversationRef"])).collect();
    format!("subscription{{runtime{{{fields}}}}}")
}
fn text_node(value: &Value, text: &str, kind: &str) -> Option<Value> {
    match value {
        Value::Object(object) => {
            if object.get("__typename").is_some_and(|v| v == kind)
                && object.get("text").is_some_and(|v| v == text)
            {
                return Some(value.clone());
            }
            object
                .values()
                .find_map(|value| text_node(value, text, kind))
        }
        Value::Array(values) => values.iter().find_map(|value| text_node(value, text, kind)),
        _ => None,
    }
}
fn observed(runtime: &Value, worker: usize, phase: usize) -> Option<Value> {
    let invocation = &runtime[format!("w{worker}")];
    let conversation = &runtime[format!("c{worker}")];
    let marker = format!(
        "replay-w{worker}-{}",
        match phase {
            0 => "progress",
            1 | 3 => "tool",
            2 => "stream",
            _ => "terminal",
        }
    );
    let activity = &invocation["activities"]["edges"][0]["node"];
    let success = match phase {
        0 => text_node(conversation, &marker, "PublishedMessage").is_some(),
        1 | 3 => {
            activity["label"] == marker
                && activity["state"] == if phase == 1 { "running" } else { "completed" }
        }
        2 => text_node(conversation, &marker, "StreamingMessage").is_some(),
        _ => {
            invocation["state"] == "completed"
                && invocation["outcome"]["status"] == 200
                && invocation["outcome"]["summary"] == marker
                && invocation["resultMessage"]["text"] == marker
        }
    };
    success.then(||json!({"marker":marker,"invocation_state":invocation["state"],"activity":activity,"outcome":invocation["outcome"]}))
}
fn receive(
    socket: &mut tungstenite::WebSocket<std::os::unix::net::UnixStream>,
    deadline: Instant,
    interrupted: &AtomicBool,
) -> Value {
    loop {
        check(deadline, interrupted);
        match socket.read() {
            Ok(Message::Text(text)) => {
                let value: Value = serde_json::from_str(&text).unwrap();
                assert!(value["payload"].get("errors").is_none(), "{value}");
                assert_ne!(value["type"], "error", "{value}");
                if value["type"] == "next" {
                    return value;
                }
            }
            Ok(_) => {
                socket.flush().unwrap();
            }
            Err(tungstenite::Error::Io(error))
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(error) => panic!("subscriber failed: {error}"),
        }
    }
}
fn run(workload: Value, repeat: u64, run_id: usize, output: &Path, interrupted: &AtomicBool) {
    let evidence = output.join(format!("run-{run_id}.json"));
    let machine_load_start = load();
    let mut fixture = Fixture::start(&workload, evidence.clone(), interrupted);
    seed(&fixture, &workload, interrupted);
    let invocations = admit(&fixture, workload["workers"].as_u64().unwrap(), interrupted);
    let mut socket = support::connect(fixture.endpoint.as_ref().unwrap());
    socket
        .send(Message::Text(
            json!({"id":"replay","type":"subscribe","payload":{"query":selection(&invocations)}})
                .to_string()
                .into(),
        ))
        .unwrap();
    let initial = receive(
        &mut socket,
        Instant::now() + Duration::from_secs(30),
        interrupted,
    );
    assert!(
        initial["payload"]["data"]["runtime"].is_object(),
        "{initial}"
    );
    let origin = Instant::now();
    let mut samples = Vec::new();
    for (phase, kind) in KINDS.iter().enumerate() {
        let trigger = Instant::now();
        let trigger_unix_ns = unix_ns();
        fs::write(fixture.root().join(format!("release-{phase}")), b"go").unwrap();
        let mut seen = vec![false; invocations.len()];
        let deadline = Instant::now() + Duration::from_secs(30);
        while !seen.iter().all(|value| *value) {
            let value = receive(&mut socket, deadline, interrupted);
            // Timestamp immediately after a full frame is decoded; includes subscriber selection/JSON decode.
            let subscriber = Instant::now();
            let subscriber_unix_ns = unix_ns();
            let runtime = &value["payload"]["data"]["runtime"];
            for (worker, invocation) in invocations.iter().enumerate() {
                if seen[worker] {
                    continue;
                }
                if let Some(state) = observed(runtime, worker, phase) {
                    seen[worker] = true;
                    samples.push(json!({"event_kind":kind,"worker":worker,"phase":phase,"marker":state["marker"],"invocation_ref":invocation["id"],"conversation_ref":invocation["conversationRef"],"trigger_unix_ns":trigger_unix_ns,"subscriber_unix_ns":subscriber_unix_ns,"trigger_elapsed_ns":trigger.duration_since(origin).as_nanos(),"subscriber_elapsed_ns":subscriber.duration_since(origin).as_nanos(),"latency_ns":subscriber.duration_since(trigger).as_nanos(),"observed_state":state}));
                }
            }
        }
    }
    let _ = socket.close(None);
    let cleanup = fixture.cleanup();
    assert_eq!(cleanup["verified"], true, "{cleanup}");
    let result = json!({"fixture_version":1,"run_id":run_id,"workload":workload,"repeat":repeat,"machine_load_start":machine_load_start,"machine_load_end":load(),"samples":samples,"order_verified":true,"cleanup":cleanup});
    drop(fixture);
    assert!(
        !Path::new(cleanup["fixture_root"].as_str().unwrap()).exists(),
        "fixture store survived cleanup"
    );
    fs::write(evidence, serde_json::to_vec_pretty(&result).unwrap()).unwrap();
    println!("Replay run {run_id}: {workload}; {repeat}");
}
#[test]
#[ignore = "explicit publication replay benchmark; use documented command"]
fn replay_publication() {
    let config: Value = serde_json::from_slice(
        &fs::read(
            std::env::var_os("CWR_REPLAY_CONFIG")
                .expect("use scripts/runtime_publication_replay.py"),
        )
        .unwrap(),
    )
    .unwrap();
    let output = PathBuf::from(config["output"].as_str().unwrap());
    assert!(output.is_absolute() && output.is_dir());
    let interrupted = Arc::new(AtomicBool::new(false));
    for signal in [signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM] {
        signal_hook::flag::register(signal, interrupted.clone()).unwrap();
    }
    let mut run_id = 0;
    for workers in config["workers"].as_array().unwrap() {
        let workers = workers.as_u64().unwrap();
        assert!((1..=16).contains(&workers));
        let base = json!({"workers":workers,"relevant_history":0,"unrelated_conversations":0,"unrelated_messages":config["unrelated_messages"]});
        let mut workloads = vec![base.clone()];
        for (field, values) in [
            ("relevant_history", &config["relevant_history"]),
            (
                "unrelated_conversations",
                &config["unrelated_conversations"],
            ),
        ] {
            for size in values
                .as_array()
                .unwrap()
                .iter()
                .filter(|size| size.as_u64().unwrap() > 0)
            {
                let mut workload = base.clone();
                workload[field] = size.clone();
                workloads.push(workload);
            }
        }
        for workload in workloads {
            for repeat in 0..config["repeats"].as_u64().unwrap() {
                run(workload.clone(), repeat, run_id, &output, &interrupted);
                run_id += 1;
            }
        }
    }
}
