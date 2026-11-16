//! Final public replies preserve identity independently of action presentation.
use super::*;

#[test]
fn invocation_result_message_exposes_the_published_chat_reply_without_action_presentation() {
    use cwr::v2026_09::upstream::{UpstreamCoworker, UpstreamEvent};
    let directory = tempfile::tempdir().unwrap();
    let runtime = mutations::fixture(directory.path());
    let person = runtime.catalog().unwrap().acting_actor.actor_ref;
    let actor = UpstreamCoworker {
        actor_ref: "cwp://example.test/actors/chat".into(),
        coworker_ref: "cwp://example.test/coworkers/chat".into(),
        handle: "@chat".into(),
        name: "Chat".into(),
        actions: vec![],
    };
    let (events, receiver) = std::sync::mpsc::channel();
    let (stops, _stop_events) = std::sync::mpsc::sync_channel(1);
    let session = runtime
        .register_upstream_provider(
            "owner",
            vec![actor.clone()],
            Arc::new(move |event| {
                events.send(event)?;
                Ok(())
            }),
            stops,
        )
        .unwrap();
    runtime
        .enable_upstream_coworker("owner", &session, &actor.actor_ref, &person, true)
        .unwrap();
    let admitted = runtime
        .submit_text_in_upstream_scope(
            "cwp://example.test/integrations/graphql",
            "chat-result",
            &person,
            None,
            "@chat Reply to this fictional request",
            Some("owner"),
        )
        .unwrap();
    let executing = runtime.clone();
    let task = admitted.clone();
    let worker = std::thread::spawn(move || executing.execute_admission(&task, |_| {}).unwrap());
    let UpstreamEvent::WorkRequested { work } =
        receiver.recv_timeout(Duration::from_secs(2)).unwrap()
    else {
        panic!("expected chat work");
    };
    runtime
        .complete_upstream_work(
            "owner",
            &work.invocation_ref,
            cwr::adapter::AdapterTerminal {
                invocation_ref: work.invocation_ref.clone(),
                sequence: 1,
                status: 200,
                summary: "Fictional reply".into(),
                result: Some(json!({"text":"Fictional reply"})),
                error: None,
                continuation: None,
            },
        )
        .unwrap();
    worker.join().unwrap();
    let server = start(
        runtime,
        "cwp://example.test/runtime".into(),
        directory.path(),
        crate::idle::Activity::default(),
        Arc::new(|_| Ok(())),
        tokio::sync::watch::channel(()).1,
    )
    .unwrap();
    let value = query(&client(&server.endpoint), &server.endpoint,
        "query($ref:ID!){runtime{invocation(ref:$ref){id state outcome{status summary error{code message}} sourceMessageRef actionPresentation{selector} resultMessage{id text authorActorRef replyToMessageRef}}}}",
        json!({"ref":admitted.invocation_ref().unwrap()}),
    );
    assert!(value.get("errors").is_none(), "{value}");
    let invocation = &value["data"]["runtime"]["invocation"];
    assert_eq!(invocation["state"], "completed", "{value}");
    assert!(invocation["actionPresentation"].is_null());
    assert!(invocation["resultMessage"]["id"].is_string(), "{value}");
    assert_eq!(
        invocation["resultMessage"]["replyToMessageRef"],
        invocation["sourceMessageRef"]
    );
    assert!(invocation["resultMessage"]["text"]
        .as_str()
        .unwrap()
        .contains("Fictional reply"));
}
