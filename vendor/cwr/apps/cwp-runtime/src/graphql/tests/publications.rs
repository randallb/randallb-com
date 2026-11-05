use super::*;
use cwr::v2026_09::{conversation::ConversationTimelineRollout, local::LocalRuntimeOptions};

fn canonical(directory: &Path) -> LocalRuntime {
    mutations::fixture_with_options(
        directory,
        LocalRuntimeOptions {
            conversation_timeline_rollout: ConversationTimelineRollout::NewConversations,
            ..Default::default()
        },
    )
}
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
const PAGE: &str = r#"
query($ref:ID!,$after:Cursor) {
  runtime {
    conversation(ref:$ref) {
      timeline(first:1,after:$after) {
        connectionRef revision totalCount
        pageInfo { hasNextPage endCursor }
        edges {
          cursor
          node {
            id publicationRef presentation { text }
            source {
              __typename
              ...on ResolvedSource { node { __typename id ...on Message { text } } }
            }
          }
        }
      }
    }
  }
}"#;

#[test]
fn canonical_pages_resolve_placement_and_source_identities_and_scope_cursors() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = canonical(directory.path());
    let first = runtime
        .admit_action(None, "@worker--run--task", json!({"value":"first"}))
        .unwrap();
    let conversation = first.conversation_ref().to_owned();
    let server = serve(directory.path(), runtime.clone());
    let client = client(&server.endpoint);
    let endpoint = session_endpoint(&client, &server.endpoint, None);
    let first_page = query_session(&client, &endpoint, PAGE, json!({"ref":conversation}));
    assert!(first_page.get("errors").is_none(), "{first_page}");
    let timeline = &first_page["data"]["runtime"]["conversation"]["timeline"];
    assert_eq!(timeline["totalCount"], 2);
    assert_eq!(timeline["pageInfo"]["hasNextPage"], true);
    let item = &timeline["edges"][0]["node"];
    assert_eq!(item["source"]["__typename"], "ResolvedSource");
    assert_eq!(item["source"]["node"]["__typename"], "PublishedMessage");
    assert_ne!(item["id"], item["source"]["node"]["id"]);
    assert_ne!(item["id"], item["publicationRef"]);
    let lookup = query_session(
        &client,
        &endpoint,
        "query($item:ID!,$source:ID!){runtime{timelineItem(ref:$item){id publicationRef} placement:node(ref:$item){__typename id} source:node(ref:$source){__typename id}}}",
        json!({"item":item["id"],"source":item["source"]["node"]["id"]}),
    );
    assert!(lookup.get("errors").is_none(), "{lookup}");
    assert_eq!(
        lookup["data"]["runtime"]["placement"]["__typename"],
        "TimelineItem"
    );
    assert_eq!(
        lookup["data"]["runtime"]["source"]["__typename"],
        "PublishedMessage"
    );
    let cursor = &timeline["pageInfo"]["endCursor"];
    let other_session = session_endpoint(&client, &server.endpoint, None);
    let denied_cursor = query_session(
        &client,
        &other_session,
        PAGE,
        json!({"ref":conversation,"after":cursor}),
    );
    assert!(denied_cursor.get("errors").is_some(), "{denied_cursor}");

    let second = query_session(
        &client,
        &endpoint,
        PAGE,
        json!({"ref":conversation,"after":cursor}),
    );
    assert!(second.get("errors").is_none(), "{second}");
    let invocation_item =
        &second["data"]["runtime"]["conversation"]["timeline"]["edges"][0]["node"];
    assert_eq!(
        invocation_item["source"]["node"]["id"],
        first.invocation_ref().unwrap()
    );
    let nested = query_session(
        &client,
        &endpoint,
        "query($ref:ID!){runtime{timelineItem(ref:$ref){children(first:1){totalCount edges{node{id}}}}}}",
        json!({"ref":invocation_item["id"]}),
    );
    assert!(nested.get("errors").is_none(), "{nested}");
    assert_eq!(
        nested["data"]["runtime"]["timelineItem"]["children"]["totalCount"],
        0
    );
    let other = runtime
        .admit_action(None, "@worker--run--task", json!({"value":"other"}))
        .unwrap();
    let invalid = query_session(
        &client,
        &endpoint,
        PAGE,
        json!({"ref":other.conversation_ref(),"after":cursor}),
    );
    assert!(invalid.get("errors").is_some(), "{invalid}");
    let backwards = query_session(
        &client,
        &endpoint,
        "query($ref:ID!){runtime{conversation(ref:$ref){timeline(last:1){pageInfo{hasPreviousPage} edges{node{source{...on ResolvedSource{node{id}}}}}}}}}",
        json!({"ref":conversation}),
    );
    assert!(backwards.get("errors").is_none(), "{backwards}");
    assert_eq!(
        backwards["data"]["runtime"]["conversation"]["timeline"]["pageInfo"]["hasPreviousPage"],
        true
    );
}

#[test]
fn canonical_lookup_hides_nonparticipants_and_does_not_invent_legacy_placements() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = canonical(directory.path());
    let actor = runtime
        .catalog()
        .unwrap()
        .actor_by_handle("@worker")
        .unwrap()
        .actor_ref
        .clone();
    let admission = runtime
        .submit_text_as(
            "cwp://example.test/integrations/graphql",
            "private",
            &actor,
            None,
            "Private worker message",
        )
        .unwrap();
    let server = serve(directory.path(), runtime.clone());
    let client = client(&server.endpoint);
    let worker = session_endpoint(&client, &server.endpoint, Some(&actor));
    let own = query_session(
        &client,
        &worker,
        PAGE,
        json!({"ref":admission.conversation_ref()}),
    );
    assert!(own.get("errors").is_none(), "{own}");
    let item = &own["data"]["runtime"]["conversation"]["timeline"]["edges"][0]["node"];
    let endpoint = session_endpoint(&client, &server.endpoint, None);
    let hidden = query_session(
        &client,
        &endpoint,
        "query($conversation:ID!,$item:ID!,$source:ID!){runtime{conversation(ref:$conversation){id} timelineItem(ref:$item){id} node(ref:$source){id}}}",
        json!({"conversation":admission.conversation_ref(),"item":item["id"],"source":item["source"]["node"]["id"]}),
    );
    assert!(hidden.get("errors").is_none(), "{hidden}");
    assert!(hidden["data"]["runtime"]["conversation"].is_null());
    assert!(hidden["data"]["runtime"]["timelineItem"].is_null());
    assert!(hidden["data"]["runtime"]["node"].is_null());

    let legacy_directory = tempfile::tempdir().unwrap();
    let legacy = mutations::fixture_with_options(
        legacy_directory.path(),
        LocalRuntimeOptions {
            conversation_timeline_rollout: ConversationTimelineRollout::Disabled,
            ..Default::default()
        },
    );
    let admission = legacy
        .admit_action(None, "@worker--run--task", json!({"value":"legacy"}))
        .unwrap();
    let legacy_server = serve(legacy_directory.path(), legacy);
    let result = query(
        &super::client(&legacy_server.endpoint),
        &legacy_server.endpoint,
        PAGE,
        json!({"ref":admission.conversation_ref()}),
    );
    assert!(result.get("errors").is_some(), "{result}");
}

#[test]
fn subscription_publications_open_a_fresh_canonical_snapshot() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = canonical(directory.path());
    let actor = runtime.catalog().unwrap().acting_actor.actor_ref;
    let admission = runtime
        .submit_text_as(
            "cwp://example.test/integrations/graphql",
            "first",
            &actor,
            None,
            "First publication",
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
        json!({"id":"publications","type":"subscribe","payload":{
            "query":"subscription($ref:ID!){runtime{conversation(ref:$ref){timeline(first:10){totalCount edges{node{id presentation{text}}}}}}}",
            "variables":{"ref":admission.conversation_ref()}
        }}),
    );
    let first = subscriptions::read(&mut socket);
    assert!(first["payload"].get("errors").is_none(), "{first}");
    assert_eq!(
        first["payload"]["data"]["runtime"]["conversation"]["timeline"]["totalCount"],
        1
    );
    runtime
        .submit_text_as(
            "cwp://example.test/integrations/graphql",
            "second",
            &actor,
            Some(admission.conversation_ref()),
            "Second publication",
        )
        .unwrap();
    changes.send_replace(());
    let next = subscriptions::read(&mut socket);
    assert!(next["payload"].get("errors").is_none(), "{next}");
    assert_eq!(
        next["payload"]["data"]["runtime"]["conversation"]["timeline"]["totalCount"],
        2
    );
    assert_eq!(
        next["payload"]["data"]["runtime"]["conversation"]["timeline"]["edges"][1]["node"]
            ["presentation"]["text"],
        "Second publication"
    );
    socket.close(None).unwrap();
}

#[test]
fn filtered_siblings_normalize_cursor_scope_and_count_before_slicing() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = canonical(directory.path());
    let first = runtime
        .admit_action(None, "@worker--run--task", json!({"value":"first"}))
        .unwrap();
    let conversation = first.conversation_ref().to_owned();
    runtime
        .admit_action(
            Some(&conversation),
            "@worker--run--task",
            json!({"value":"second"}),
        )
        .unwrap();
    let server = serve(directory.path(), runtime);
    let client = client(&server.endpoint);
    let endpoint = session_endpoint(&client, &server.endpoint, None);
    let document = r#"
      query($ref:ID!,$filter:TimelineFilter,$after:Cursor,$last:Int,$first:Int) {
        runtime {
          conversation(ref:$ref) {
            timeline(filter:$filter,first:$first,last:$last,after:$after) {
              totalCount
              pageInfo { hasNextPage hasPreviousPage endCursor }
              edges { node { id source { ...on ResolvedSource { node { __typename } } } } }
            }
          }
        }
      }
    "#;
    let run = |filter: Value, after: Value, first: Value, last: Value| {
        query_session(
            &client,
            &endpoint,
            document,
            json!({"ref":conversation,"filter":filter,"after":after,"first":first,"last":last}),
        )
    };
    let page = run(
        json!({"sourceTypes":["MESSAGE","MESSAGE"]}),
        Value::Null,
        json!(1),
        Value::Null,
    );
    assert!(page.get("errors").is_none(), "{page}");
    let timeline = &page["data"]["runtime"]["conversation"]["timeline"];
    assert_eq!(timeline["totalCount"], 2);
    assert_eq!(timeline["pageInfo"]["hasNextPage"], true);
    let cursor = timeline["pageInfo"]["endCursor"].clone();
    let next = run(
        json!({"sourceTypes":["MESSAGE"],"search":"  "}),
        cursor.clone(),
        json!(1),
        Value::Null,
    );
    assert!(next.get("errors").is_none(), "{next}");
    assert_eq!(
        next["data"]["runtime"]["conversation"]["timeline"]["totalCount"],
        2
    );
    let wrong = run(
        json!({"sourceTypes":["ACTION_INVOCATION"]}),
        cursor,
        json!(1),
        Value::Null,
    );
    assert!(wrong["errors"].is_array(), "{wrong}");
    let search = run(
        json!({"search":"WORK ADMITTED"}),
        Value::Null,
        Value::Null,
        json!(1),
    );
    assert!(search.get("errors").is_none(), "{search}");
    assert_eq!(
        search["data"]["runtime"]["conversation"]["timeline"]["totalCount"],
        2
    );
    assert_eq!(
        search["data"]["runtime"]["conversation"]["timeline"]["pageInfo"]["hasPreviousPage"],
        true
    );
    let empty = run(
        json!({"search":"no match exists"}),
        Value::Null,
        json!(1),
        Value::Null,
    );
    assert_eq!(
        empty["data"]["runtime"]["conversation"]["timeline"]["totalCount"],
        0
    );
}

#[test]
fn child_role_filter_pages_matches_and_scopes_cursors() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = canonical(directory.path());
    let caller = runtime.catalog().unwrap().acting_actor.actor_ref;
    let conversation = "cwp://example.test/child-role/source";
    let source = format!("{conversation}/messages/source");
    let first_reply = format!("{conversation}/messages/reply-1");
    let last_reply = format!("{conversation}/messages/reply-2");
    let invocation = format!("{conversation}/work/answer");
    let root = format!("{conversation}/timeline");
    let children = format!("{conversation}/children");
    let item = |connection: &str, sequence: u64, message: &str| {
        let publication = format!("{connection}/publications/{sequence}");
        json!({"item_ref":format!("{publication}/item"),"publication_ref":publication,
            "connection_ref":connection,"sibling_sequence":sequence,
            "content":{"kind":"message","message_ref":message}})
    };
    let invocation_publication = format!("{root}/publications/work");
    let export = json!({
        "version":"cwr.conversation-export/3","exported_at_unix_ms":1,
        "conversation":{"conversation_ref":conversation,"participant_actor_refs":[caller],
            "messages":[
                {"message_ref":source,"author_actor_ref":caller,"addressed_actor_refs":[],"text":"Question"},
                {"message_ref":first_reply,"author_actor_ref":caller,"addressed_actor_refs":[],"reply_to_message_ref":source,"text":"First reply"},
                {"message_ref":last_reply,"author_actor_ref":caller,"addressed_actor_refs":[],"reply_to_message_ref":source,"text":"Last reply"}
            ],
            "activity":[
                {"activity_ref":format!("{conversation}/activity/1"),"sequence":1,"occurred_at_unix_ms":1,
                    "event":{"kind":"message_recorded","message_ref":source}},
                {"activity_ref":format!("{conversation}/activity/2"),"sequence":2,"occurred_at_unix_ms":2,
                    "event":{"kind":"message_recorded","message_ref":first_reply}},
                {"activity_ref":format!("{conversation}/activity/3"),"sequence":3,"occurred_at_unix_ms":3,
                    "event":{"kind":"message_recorded","message_ref":last_reply}}
            ]},
        "timeline":{"work":[{"invocation_ref":invocation,"requester_actor_ref":caller,
            "target_actor_ref":caller,"action_name":"retained--work","source_message_ref":source,
            "reply_message_ref":null,"parent_invocation_ref":null,"admission_sequence":1,
            "admitted_at_unix_ms":1,"started_at_unix_ms":1,"finished_at_unix_ms":2,
            "status":cwr::v2026_09::invocation::ActionResponseStatus::COMPLETED,
            "summary":"Done","activities":[]}],
            "timeline":{"root_connection_ref":root,"connections":[
                {"connection_ref":root,"owner":{"kind":"conversation","conversation_ref":conversation},
                    "revision":2,"next_sequence":3,"items":[item(&root,1,&source),
                        {"item_ref":format!("{invocation_publication}/item"),"publication_ref":invocation_publication,
                         "connection_ref":root,"sibling_sequence":2,
                         "content":{"kind":"invocation","invocation_ref":invocation},"child_connection_ref":children}]},
                {"connection_ref":children,"owner":{"kind":"timeline_item","item_ref":format!("{invocation_publication}/item")},
                    "revision":3,"next_sequence":4,"items":[item(&children,1,&first_reply),item(&children,2,&last_reply),item(&children,3,&source)]}
            ]}}
    });
    let path = directory.path().join("child-role-export.json");
    fs::write(&path, serde_json::to_vec(&export).unwrap()).unwrap();
    let imported = runtime
        .import_conversation_as(&caller, path.to_str().unwrap(), Some("child-role"))
        .unwrap();
    let cwr::intrinsic_cwr::IntrinsicCwrResponse::ConversationImported { conversation_ref } =
        imported
    else {
        panic!("unexpected import response");
    };

    let server = serve(directory.path(), runtime);
    let client = client(&server.endpoint);
    let endpoint = session_endpoint(&client, &server.endpoint, None);
    let document = r#"
      query($ref:ID!,$filter:TimelineFilter,$before:Cursor) {
        runtime { conversation(ref:$ref) { timeline(last:1) { edges { node {
          children(filter:$filter,last:1,before:$before) {
            totalCount pageInfo { hasPreviousPage startCursor }
            edges { node { presentation { text } } }
          }
        } } } } }
      }
    "#;
    let run = |filter: Value, before: Value| {
        query_session(
            &client,
            &endpoint,
            document,
            json!({"ref":conversation_ref,"filter":filter,"before":before}),
        )
    };
    let latest = run(json!({"childRole":"FINAL_REPLY"}), Value::Null);
    assert!(latest.get("errors").is_none(), "{latest}");
    let children =
        &latest["data"]["runtime"]["conversation"]["timeline"]["edges"][0]["node"]["children"];
    assert_eq!(children["totalCount"], 2);
    assert_eq!(children["pageInfo"]["hasPreviousPage"], true);
    assert_eq!(
        children["edges"][0]["node"]["presentation"]["text"],
        "Last reply"
    );
    let cursor = children["pageInfo"]["startCursor"].clone();
    let earlier = run(json!({"childRole":"FINAL_REPLY"}), cursor.clone());
    assert!(earlier.get("errors").is_none(), "{earlier}");
    assert_eq!(
        earlier["data"]["runtime"]["conversation"]["timeline"]["edges"][0]["node"]["children"]
            ["edges"][0]["node"]["presentation"]["text"],
        "First reply"
    );
    let wrong_filter = run(json!({"childRole":"COMMENTARY"}), cursor);
    assert!(wrong_filter["errors"].is_array(), "{wrong_filter}");
}

#[test]
fn message_metadata_keeps_authorship_and_exposes_selected_action_details() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = canonical(directory.path());
    let role = directory.path().join("workspace/ROLE.toml");
    fs::write(
        &role,
        fs::read_to_string(&role)
            .unwrap()
            .replace("title='Run task'", "title='Run the configured task'"),
    )
    .unwrap();
    runtime.reload_graph().unwrap();
    let admitted = runtime
        .admit_action(None, "@worker--run--task", json!({"value":"private input"}))
        .unwrap();
    let server = serve(directory.path(), runtime);
    let client = client(&server.endpoint);
    let endpoint = session_endpoint(&client, &server.endpoint, None);
    let result = query_session(
        &client,
        &endpoint,
        r#"
      query($ref:ID!) {
        runtime {
          conversation(ref:$ref) {
            timeline {
              edges { node { source { ...on ResolvedSource { node { ...on Message {
                id text authorActorRef author { id name __typename }
                ...on PublishedMessage { visualizations { markerIndex title mode error artifactRef } }
                occurredAt addressingMode progress { sourceActivityRef }
                action { __typename title ...on MessageActionCall { selector input invocation { id actionPresentation { title selector request } requesterActorRef targetActorRef requester { id name } target { id name } } } }
                replyTo { id }
              } } } } } }
            }
          }
        }
      }
    "#,
        json!({"ref":admitted.conversation_ref()}),
    );
    assert!(result.get("errors").is_none(), "{result}");
    let message = &result["data"]["runtime"]["conversation"]["timeline"]["edges"][0]["node"]
        ["source"]["node"];
    assert_eq!(message["author"]["id"], message["authorActorRef"]);
    assert_eq!(message["author"]["__typename"], "Coworker");
    assert!(message["occurredAt"].as_str().unwrap().ends_with('Z'));
    assert_eq!(message["action"]["input"], json!({"value":"private input"}));
    assert_eq!(message["visualizations"], json!([]));
    assert_eq!(message["addressingMode"], "EXPLICIT");
    let invocation = &message["action"]["invocation"];
    assert_eq!(
        invocation["requester"]["id"],
        invocation["requesterActorRef"]
    );
    assert_eq!(invocation["target"]["id"], invocation["targetActorRef"]);
    assert_eq!(
        invocation["actionPresentation"]["selector"],
        "@worker--run--task"
    );
    assert_eq!(
        invocation["actionPresentation"]["title"],
        "Run the configured task"
    );
    assert_eq!(invocation["actionPresentation"]["request"], message["text"]);
    let direct_invocation = query_session(
        &client,
        &endpoint,
        "query($ref:ID!){runtime{invocation(ref:$ref){requester{id name} target{id name}} node(ref:$ref){...on ActionInvocation{requester{id name} target{id name}}}}}",
        json!({"ref":invocation["id"]}),
    );
    assert!(
        direct_invocation.get("errors").is_none(),
        "{direct_invocation}"
    );
    for field in ["requester", "target"] {
        assert_eq!(
            direct_invocation["data"]["runtime"]["invocation"][field],
            invocation[field]
        );
        assert_eq!(
            direct_invocation["data"]["runtime"]["node"][field],
            invocation[field]
        );
    }
    let direct = query_session(
        &client,
        &endpoint,
        "query($ref:ID!){runtime{node(ref:$ref){id __typename ...on Actor{name}}}}",
        json!({"ref":message["authorActorRef"]}),
    );
    assert_eq!(direct["data"]["runtime"]["node"], message["author"]);
}

#[test]
fn stop_control_links_retained_target_without_parsing_a_message() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = canonical(directory.path());
    let work = runtime
        .admit_action(None, "@worker--run--task", json!({"value":"queued"}))
        .unwrap();
    let control = runtime
        .admit_action(
            Some(work.conversation_ref()),
            "@cwr--stop--invocation",
            json!({"invocation_ref":work.invocation_ref().unwrap()}),
        )
        .unwrap();
    runtime.execute_admission(&control, |_| {}).unwrap();
    let server = serve(directory.path(), runtime.clone());
    let client = client(&server.endpoint);
    let document = "query($ref:ID!){runtime{invocation(ref:$ref){id stopControl{disposition target{id state stopSettlement finishedAt}}}}}";
    for _ in 0..2 {
        let result = query(
            &client,
            &server.endpoint,
            document,
            json!({"ref":control.invocation_ref().unwrap()}),
        );
        assert!(result.get("errors").is_none(), "{result}");
        let receipt = &result["data"]["runtime"]["invocation"]["stopControl"];
        assert_eq!(receipt["disposition"], "ACCEPTED");
        assert_eq!(receipt["target"]["id"], work.invocation_ref().unwrap());
        assert_eq!(receipt["target"]["state"], "stopped");
        assert_eq!(receipt["target"]["stopSettlement"], "CONFIRMED");
        assert!(receipt["target"]["finishedAt"].is_string());
    }
}
