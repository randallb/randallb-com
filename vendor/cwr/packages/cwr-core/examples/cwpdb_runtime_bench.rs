//! Runs real admission, claim, progress, publication and read/reopen APIs without a provider.
use cwr::{
    v2026_09::{conversation::*, graph::resolve_graph, invocation::ActionResponseStatus},
    CwrResult,
};
use serde_json::{json, Value};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

const ROOT: &str = "cwp://org.coworkerprotocol.local/runtimes/7fa11111-2222-4333-8444-555555555555";

fn main() -> CwrResult<()> {
    let mut args = std::env::args().skip(1);
    let directory = PathBuf::from(
        args.next()
            .ok_or("usage: cwpdb_runtime_bench NEW_DIRECTORY [turns]")?,
    );
    let count: usize = args.next().unwrap_or_else(|| "1000".into()).parse()?;
    if count == 0 {
        return Err("turns must be positive".into());
    }
    std::fs::create_dir(&directory)?;
    let path = directory.join("cwpdb.sqlite3");
    let graph = resolve_graph(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures/contracts/agent-recovery-turn/workspace/CWR.toml"),
    )?;
    let actor = |slug: &str| {
        graph
            .actors
            .iter()
            .find(|a| a.coworker_slug == slug)
            .expect("fixture actor")
    };
    let person = actor("person");
    let worker = actor("helper");
    let binding = worker.default_actor_integration_ref.as_deref();
    let mut store = ConversationAdmissionStore::open(&path, ROOT)?;
    let mut admission = Vec::new();
    let mut claim = Vec::new();
    let mut progress = Vec::new();
    let mut finish = Vec::new();
    let text = "x".repeat(1024);
    let started = Instant::now();
    let mut last_conversation = String::new();
    for index in 0..count {
        let ingress = ConversationIngress {
            owner_context_ref: None,
            file_refs: Vec::new(),
            integration_ref: format!("{ROOT}/integrations/benchmark"),
            provider_event_ref: format!("event-{index}"),
            author_actor_ref: person.actor_ref.clone(),
            conversation: ConversationIngressConversation::Create,
            addressing: ConversationIngressAddressing::Addressed {
                actor_refs: vec![worker.actor_ref.clone()],
            },
            text: text.clone(),
        };
        let t = Instant::now();
        let ConversationAdmissionOutcome::Started(turn) = store.admit(&graph, &ingress, binding)?
        else {
            return Err("expected new admission".into());
        };
        admission.push(t.elapsed().as_secs_f64() * 1000.0);
        let invocation = turn.record.invocation_ref.as_deref().ok_or("invocation")?;
        last_conversation = turn.record.conversation_ref.clone();
        let t = Instant::now();
        store.claim_invocation(invocation, 11)?;
        claim.push(t.elapsed().as_secs_f64() * 1000.0);
        let t = Instant::now();
        store.record_invocation_progress_with_activity(invocation, 102, "Working")?;
        progress.push(t.elapsed().as_secs_f64() * 1000.0);
        let t = Instant::now();
        store.publish_root_message_completed(
            invocation,
            ActionResponseStatus::COMPLETED,
            json!({"text":text}),
            "Completed".into(),
            30,
        )?;
        finish.push(t.elapsed().as_secs_f64() * 1000.0);
        assert!(matches!(
            store.admit(&graph, &ingress, binding)?,
            ConversationAdmissionOutcome::Existing(_)
        ));
    }
    let elapsed = started.elapsed().as_secs_f64();
    drop(store);
    let t = Instant::now();
    let reopened = ConversationAdmissionStore::open(&path, ROOT)?;
    let reopen_ms = t.elapsed().as_secs_f64() * 1000.0;
    assert_eq!(reopened.admissions().count(), count);
    assert_eq!(reopened.invocation_records().count(), count);
    assert!(reopened.runnable_invocation_refs().is_empty());
    assert_eq!(
        reopened
            .conversation(&last_conversation)
            .unwrap()
            .messages
            .len(),
        2
    );
    let reads = measure_reads(&path, &last_conversation, count)?;
    drop(reopened);
    let physical = cwpdb_store::benchmark::checkpoint_runtime_database(&path)?;
    let report = json!({"turns":count,"message_body_bytes":1024,"sqlite":physical.sqlite_version,"durability":"WAL/FULL",
        "admit_ms":distribution(admission),"claim_ms":distribution(claim),"progress_ms":distribution(progress),"finish_ms":distribution(finish),
        "total_seconds":elapsed,"reopen_ms":reopen_ms,"database_bytes":physical.database_bytes,"reads":reads,
        "scope":"Real runtime APIs and graph preparation; excludes provider processes and scheduling."});
    std::fs::write(
        directory.join("benchmark.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

fn measure_reads(path: &std::path::Path, conversation: &str, count: usize) -> CwrResult<Value> {
    let mut reader = ConversationReadStore::open(path, ROOT)?;
    let mut summaries = Vec::new();
    let mut timelines = Vec::new();
    for _ in 0..25 {
        let operation = reader.begin_operation(Duration::from_secs(10))?;
        let t = Instant::now();
        let page = operation.root_conversations(ConversationSummaryWindow {
            first: Some(50),
            ..Default::default()
        })?;
        summaries.push(t.elapsed().as_secs_f64() * 1000.0);
        assert_eq!(page.total_count, count);
        let t = Instant::now();
        let page = operation.conversation_timeline(
            conversation,
            ConversationRecordWindow {
                first: Some(50),
                ..Default::default()
            },
        )?;
        timelines.push(t.elapsed().as_secs_f64() * 1000.0);
        assert_eq!(page.messages.len(), 2);
    }
    Ok(json!({"summary_50_ms":distribution(summaries),"completed_turn_ms":distribution(timelines)}))
}

fn distribution(mut samples: Vec<f64>) -> Value {
    samples.sort_by(f64::total_cmp);
    json!({"median":samples[samples.len()/2],"p95":samples[(samples.len()*95/100).min(samples.len()-1)],"samples":samples.len()})
}
