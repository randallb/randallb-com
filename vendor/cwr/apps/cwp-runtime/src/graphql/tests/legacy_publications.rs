//! Rootless layout selection, cursor authority and live retained responses.
use super::*;
use cwr::v2026_09::{conversation::ConversationTimelineRollout, local::LocalRuntimeOptions};

fn serve(directory: &Path, runtime: LocalRuntime) -> GraphqlServer {
    start(
        runtime,
        "cwp://example.test/runtime".into(),
        directory,
        crate::idle::Activity::default(),
        Arc::new(|_| Ok(())),
        tokio::sync::watch::channel(()).1,
    )
    .unwrap()
}

#[test]
fn selective_legacy_history_uses_position_cursors_without_creating_a_root() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = mutations::fixture_with_options(
        directory.path(),
        LocalRuntimeOptions {
            conversation_timeline_rollout: ConversationTimelineRollout::Disabled,
            ..Default::default()
        },
    );
    let actor = runtime.catalog().unwrap().acting_actor.actor_ref;
    let admission = runtime
        .submit_text_as(
            "cwp://example.test/integrations/graphql",
            "legacy-first",
            &actor,
            None,
            "First legacy message",
        )
        .unwrap();
    let reference = admission.conversation_ref().to_owned();
    runtime
        .submit_text_as(
            "cwp://example.test/integrations/graphql",
            "legacy-second",
            &actor,
            Some(&reference),
            "Second legacy message",
        )
        .unwrap();
    let server = serve(directory.path(), runtime.clone());
    let client = super::client(&server.endpoint);
    let endpoint = session_endpoint(&client, &server.endpoint, None);
    let selection = r#"query($ref:ID!,$before:Cursor){runtime{conversation(ref:$ref){
        canonicalTimeline(last:1,before:$before){edges{node{id}}}
        legacyHistory(last:1,before:$before){revision pageInfo{hasPreviousPage startCursor}
            edges{cursor node{id ...on Message{text authorActorRef}}}}
    }}}"#;
    let latest = query_session(&client, &endpoint, selection, json!({"ref":reference}));
    assert!(latest.get("errors").is_none(), "{latest}");
    let conversation = &latest["data"]["runtime"]["conversation"];
    assert!(conversation["canonicalTimeline"].is_null());
    let page = &conversation["legacyHistory"];
    assert_eq!(page["edges"].as_array().unwrap().len(), 1);
    assert_eq!(page["pageInfo"]["hasPreviousPage"], true);
    let older = query_session(
        &client,
        &endpoint,
        selection,
        json!({"ref":reference,"before":page["pageInfo"]["startCursor"]}),
    );
    assert!(older.get("errors").is_none(), "{older}");
    assert_ne!(
        older["data"]["runtime"]["conversation"]["legacyHistory"]["edges"][0]["cursor"],
        page["edges"][0]["cursor"]
    );
    let another_session = session_endpoint(&client, &server.endpoint, None);
    let denied = query_session(
        &client,
        &another_session,
        selection,
        json!({"ref":reference,"before":page["pageInfo"]["startCursor"]}),
    );
    assert!(denied.get("errors").is_some(), "{denied}");
    let snapshot = runtime
        .bundled_conversation_module_as("cwp://example.test/integrations/graphql", &actor)
        .unwrap()
        .publication_snapshot_in_context(
            None,
            std::time::Instant::now() + Duration::from_secs(5),
            || false,
            |_| {},
        )
        .unwrap();
    assert!(snapshot.timeline_layout(&reference).unwrap().is_none());
}

#[test]
fn legacy_subscription_refreshes_retained_final_response_without_migration() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = mutations::fixture_with_options(
        directory.path(),
        LocalRuntimeOptions {
            conversation_timeline_rollout: ConversationTimelineRollout::Disabled,
            ..Default::default()
        },
    );
    let actor = runtime.catalog().unwrap().acting_actor.actor_ref;
    let admission = runtime
        .submit_text_as(
            "cwp://example.test/integrations/graphql",
            "legacy-final",
            &actor,
            None,
            "@cwr--list--coworkers",
        )
        .unwrap();
    let (changes, observe) = tokio::sync::watch::channel(());
    let server = start(
        runtime.clone(),
        "cwp://example.test/runtime".into(),
        directory.path(),
        crate::idle::Activity::default(),
        Arc::new(|_| Ok(())),
        observe,
    )
    .unwrap();
    let endpoint = session_endpoint(&client(&server.endpoint), &server.endpoint, None);
    let mut socket = subscriptions::connect_session(&endpoint);
    subscriptions::send(
        &mut socket,
        json!({"id":"legacy-final","type":"subscribe","payload":{
            "query":"subscription($ref:ID!){runtime{conversation(ref:$ref){canonicalTimeline(last:10){edges{node{id}}} legacyHistory(last:10){edges{node{id ...on Message{text authorActorRef replyToMessageRef} ...on ActionInvocation{state}}}}}}}",
            "variables":{"ref":admission.conversation_ref()}
        }}),
    );
    let first = subscriptions::read(&mut socket);
    assert!(first["payload"].get("errors").is_none(), "{first}");
    let settled = runtime.execute_admission(&admission, |_| {}).unwrap();
    let final_message = settled
        .conversation
        .messages
        .iter()
        .find(|message| {
            message.reply_to_message_ref.as_deref()
                == Some(admission.record.source_message_ref.as_str())
        })
        .expect("retained final response linked to its original request");
    changes.send_replace(());
    let next = subscriptions::read(&mut socket);
    assert!(next["payload"].get("errors").is_none(), "{next}");
    let history = &next["payload"]["data"]["runtime"]["conversation"];
    assert!(history["canonicalTimeline"].is_null());
    let edges = history["legacyHistory"]["edges"].as_array().unwrap();
    assert!(
        edges
            .iter()
            .any(|edge| edge["node"]["state"] == "completed"),
        "{next}"
    );
    let published = edges
        .iter()
        .find(|edge| edge["node"]["id"] == final_message.message_ref)
        .expect("selected final response is a message, not an invocation summary");
    assert_eq!(published["node"]["text"], final_message.text);
    assert_eq!(
        published["node"]["authorActorRef"],
        final_message.author_actor_ref
    );
    assert_eq!(
        published["node"]["replyToMessageRef"],
        admission.record.source_message_ref
    );
    assert!(
        edges.len()
            > first["payload"]["data"]["runtime"]["conversation"]["legacyHistory"]["edges"]
                .as_array()
                .unwrap()
                .len()
    );
}
