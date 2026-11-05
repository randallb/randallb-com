use super::*;
use cwr::v2026_09::local::LocalHostIdentity;
use reqwest::blocking::Client;
use serde_json::{json, Value};

fn runtime(directory: &Path) -> LocalRuntime {
    LocalRuntime::open_with_state_path_and_runtime_ref(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(
            "../../packages/cwr-core/fixtures/contracts/agent-recovery-turn/workspace/CWR.toml",
        ),
        directory.join("cwpdb.sqlite3"),
        "cwp://example.test/runtime",
        LocalHostIdentity::new(
            crate::test_refs::actor_ref("person").as_str(),
            "cwp://example.test/integrations/graphql",
        ),
    )
    .unwrap()
}
fn client(endpoint: &LocalGraphqlEndpoint) -> Client {
    Client::builder()
        .unix_socket(endpoint.socket_path.clone())
        .no_proxy()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap()
}
fn session_endpoint(
    client: &Client,
    endpoint: &LocalGraphqlEndpoint,
    actor: Option<&str>,
) -> LocalGraphqlEndpoint {
    let opened: Value = client
        .post(format!("{}/session", endpoint.url()))
        .bearer_auth(&endpoint.token)
        .json(&json!({"actorRef":actor}))
        .send()
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .unwrap();
    let mut endpoint = endpoint.clone();
    endpoint.token = opened["token"].as_str().unwrap().into();
    endpoint.actor_ref = opened["actorRef"].as_str().unwrap().into();
    endpoint
}
fn query(client: &Client, endpoint: &LocalGraphqlEndpoint, text: &str, variables: Value) -> Value {
    let endpoint = session_endpoint(client, endpoint, None);
    let result = query_session(client, &endpoint, text, variables);
    client
        .delete(format!("{}/session", endpoint.url()))
        .bearer_auth(&endpoint.token)
        .send()
        .unwrap();
    result
}
fn query_session(
    client: &Client,
    endpoint: &LocalGraphqlEndpoint,
    text: &str,
    variables: Value,
) -> Value {
    client
        .post(endpoint.url())
        .bearer_auth(&endpoint.token)
        .json(&json!({"query":text,"variables":variables}))
        .send()
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .unwrap()
}

#[test]
fn authenticated_reads_paginate_and_preserve_message_identity() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = runtime(directory.path());
    let actor = crate::test_refs::actor_ref("person");
    let first = runtime
        .submit_text_as(
            "cwp://example.test/integrations/graphql",
            "one",
            actor.as_str(),
            None,
            "First message",
        )
        .unwrap();
    runtime
        .submit_text_as(
            "cwp://example.test/integrations/graphql",
            "two",
            actor.as_str(),
            None,
            "Second message",
        )
        .unwrap();
    let server = start(
        runtime,
        "cwp://example.test/runtime".into(),
        directory.path(),
        crate::idle::Activity::default(),
        Arc::new(|_| Ok(())),
        tokio::sync::watch::channel(()).1,
    )
    .unwrap();
    let endpoint = LocalGraphqlEndpoint::read(&directory.path().join("graphql.json")).unwrap();
    let client = client(&server.endpoint);
    let document = "query($after:String){runtime{id actorRef defaultMessageTarget { id handle name } messageTargets { id handle name } conversations(first:1,after:$after){totalCount edges{cursor node{id summary{lastMessagePreview completedWorkRefs completedWork{invocationRef sequence} revision}}} pageInfo{hasNextPage}}}}";
    let page = query(&client, &endpoint, document, json!({}));
    assert!(page.get("errors").is_none(), "{page}");
    let targets = page["data"]["runtime"]["messageTargets"]
        .as_array()
        .unwrap();
    assert!(page["data"]["runtime"]["defaultMessageTarget"].is_null());
    assert!(!targets.is_empty());
    assert!(targets.iter().all(|target| target["id"] != actor.as_str()));
    assert!(targets.iter().any(|target| target["handle"] == "@helper"));
    assert_eq!(page["data"]["runtime"]["conversations"]["totalCount"], 2);
    assert_eq!(
        page["data"]["runtime"]["conversations"]["pageInfo"]["hasNextPage"],
        true
    );
    assert_eq!(
        page["data"]["runtime"]["conversations"]["edges"][0]["node"]["summary"]
            ["lastMessagePreview"],
        "Second message"
    );
    let cursor = &page["data"]["runtime"]["conversations"]["edges"][0]["cursor"];
    let second = query(&client, &endpoint, document, json!({"after":cursor}));
    assert!(second.get("errors").is_none(), "{second}");
    assert_ne!(
        page["data"]["runtime"]["conversations"]["edges"][0]["node"]["id"],
        second["data"]["runtime"]["conversations"]["edges"][0]["node"]["id"]
    );
    let timeline = query(
        &client,
        &endpoint,
        "query($id:ID!){runtime{conversation(id:$id){id summary{lastMessagePreview} legacyTimeline(first:10){edges{node{__typename ...on Message{id authorActorRef text replyToMessageRef}}} revision}}}}",
        json!({"id":first.conversation_ref()}),
    );
    assert!(timeline.get("errors").is_none(), "{timeline}");
    assert_eq!(
        timeline["data"]["runtime"]["conversation"]["summary"]["lastMessagePreview"],
        "First message"
    );
    let item = &timeline["data"]["runtime"]["conversation"]["legacyTimeline"]["edges"][0]["node"];
    assert_eq!(item["__typename"], "PublishedMessage");
    assert_eq!(item["text"], "First message");
    assert_eq!(item["authorActorRef"], actor.as_str());
    drop(server);
    assert!(!directory.path().join("graphql.json").exists());
}

#[test]
fn http_boundary_rejects_missing_credentials_browser_origins_and_spoofed_hosts() {
    let directory = tempfile::tempdir().unwrap();
    let server = start(
        runtime(directory.path()),
        "cwp://example.test/runtime".into(),
        directory.path(),
        crate::idle::Activity::default(),
        Arc::new(|_| Ok(())),
        tokio::sync::watch::channel(()).1,
    )
    .unwrap();
    let endpoint = &server.endpoint;
    let client = client(&server.endpoint);
    let body = json!({"query":"{runtime{id}}"});
    assert_eq!(
        client
            .post(endpoint.url())
            .json(&body)
            .send()
            .unwrap()
            .status(),
        401
    );
    assert_eq!(
        client
            .post(endpoint.url())
            .bearer_auth("wrong")
            .json(&body)
            .send()
            .unwrap()
            .status(),
        401
    );
    assert_eq!(
        client
            .post(endpoint.url())
            .bearer_auth(&endpoint.token)
            .header("Origin", "http://untrusted.test")
            .json(&body)
            .send()
            .unwrap()
            .status(),
        403
    );
    assert_eq!(
        client
            .post(endpoint.url())
            .bearer_auth(&endpoint.token)
            .header("Host", "untrusted.test")
            .json(&body)
            .send()
            .unwrap()
            .status(),
        403
    );
    let invalid_page = query(
        &client,
        endpoint,
        "{runtime{conversations(first:101){totalCount}}}",
        json!({}),
    );
    assert!(invalid_page.get("errors").is_some());
}

#[test]
fn actor_scope_hides_other_participants_and_startup_rotates_credentials() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = runtime(directory.path());
    let helper = crate::test_refs::actor_ref("helper");
    let hidden = runtime
        .submit_text_as(
            "cwp://example.test/integrations/graphql",
            "hidden",
            helper.as_str(),
            None,
            "Private helper message",
        )
        .unwrap();
    let server = start(
        runtime.clone(),
        "cwp://example.test/runtime".into(),
        directory.path(),
        crate::idle::Activity::default(),
        Arc::new(|_| Ok(())),
        tokio::sync::watch::channel(()).1,
    )
    .unwrap();
    let old = server.endpoint.clone();
    let client = client(&server.endpoint);
    let result = query(
        &client,
        &old,
        "query($id:ID!){runtime{conversations{totalCount} conversation(id:$id){id summary{lastMessagePreview}}}}",
        json!({"id":hidden.conversation_ref()}),
    );
    assert!(result.get("errors").is_none(), "{result}");
    assert_eq!(result["data"]["runtime"]["conversations"]["totalCount"], 0);
    assert!(result["data"]["runtime"]["conversation"].is_null());
    drop(server);
    let replacement = start(
        runtime,
        "cwp://example.test/runtime".into(),
        directory.path(),
        crate::idle::Activity::default(),
        Arc::new(|_| Ok(())),
        tokio::sync::watch::channel(()).1,
    )
    .unwrap();
    assert!(replacement.endpoint.token != old.token);
    assert_eq!(
        client
            .post(replacement.endpoint.url())
            .bearer_auth(&old.token)
            .json(&json!({"query":"{runtime{id}}"}))
            .send()
            .unwrap()
            .status(),
        401
    );
}

#[test]
fn exported_schema_matches_the_checked_in_contract() {
    assert_eq!(
        sdl().trim_end(),
        include_str!("../../schema.graphql").trim_end()
    );
}

mod mutations;

mod subscriptions;

#[test]
fn incomplete_unauthenticated_http_body_cannot_block_shutdown() {
    use std::{io::Write, os::unix::net::UnixStream, time::Instant};
    let directory = tempfile::tempdir().unwrap();
    let (_changes, observe) = tokio::sync::watch::channel(());
    let server = start(
        runtime(directory.path()),
        "cwp://example.test/runtime".into(),
        directory.path(),
        crate::idle::Activity::default(),
        Arc::new(|_| Ok(())),
        observe,
    )
    .unwrap();
    let mut socket = UnixStream::connect(&server.endpoint.socket_path).unwrap();
    write!(socket, "POST /graphql HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: 999\r\n\r\n{{").unwrap();
    std::thread::sleep(Duration::from_millis(50));
    let start = Instant::now();
    drop(server);
    assert!(start.elapsed() < Duration::from_secs(4));
}

#[test]
fn timeline_invocation_node_has_the_direct_lookup_identity() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = mutations::fixture(directory.path());
    let admission = runtime
        .admit_action(None, "@worker--run--task", json!({"value":"typed"}))
        .unwrap();
    let id = admission.invocation_ref().unwrap();
    let conversation = admission.conversation_ref();
    let (_changes, observe) = tokio::sync::watch::channel(());
    let server = start(
        runtime,
        "cwp://example.test/runtime".into(),
        directory.path(),
        crate::idle::Activity::default(),
        Arc::new(|_| Ok(())),
        observe,
    )
    .unwrap();
    let nodes = query(
        &client(&server.endpoint),
        &server.endpoint,
        "query($id:ID!){runtime{conversation(id:$id){legacyTimeline{edges{node{__typename ...on Message{id} ...on ActionInvocation{id state}}}}}}}",
        json!({"id":conversation}),
    );
    assert!(nodes.get("errors").is_none(), "{nodes}");
    let edges = nodes["data"]["runtime"]["conversation"]["legacyTimeline"]["edges"]
        .as_array()
        .unwrap();
    assert!(
        edges
            .iter()
            .any(|edge| edge["node"]["__typename"] == "AdmittedInvocation"
                && edge["node"]["id"] == id)
    );
    let direct = query(
        &client(&server.endpoint),
        &server.endpoint,
        "query($id:ID!){runtime{invocation(id:$id){__typename id}}}",
        json!({"id":id}),
    );
    assert_eq!(
        direct["data"]["runtime"]["invocation"]["__typename"],
        "AdmittedInvocation"
    );
    assert_eq!(direct["data"]["runtime"]["invocation"]["id"], id);
}

mod creation;
mod sessions;

mod budgets;

#[test]
fn endpoint_owner_excludes_competing_publication_and_releases_after_cleanup() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = runtime(directory.path());
    let serve = || {
        start(
            runtime.clone(),
            "cwp://example.test/runtime".into(),
            directory.path(),
            crate::idle::Activity::default(),
            Arc::new(|_| Ok(())),
            tokio::sync::watch::channel(()).1,
        )
    };
    let first = serve().unwrap();
    let discovery = fs::read(directory.path().join("graphql.json")).unwrap();
    assert!(serve().is_err());
    assert_eq!(
        fs::read(directory.path().join("graphql.json")).unwrap(),
        discovery
    );
    assert!(query(
        &client(&first.endpoint),
        &first.endpoint,
        "{runtime{id}}",
        json!({})
    )
    .get("errors")
    .is_none());
    drop(first);
    assert!(!directory.path().join("graphql.sock").exists());
    assert!(directory.path().join("graphql.sock.lock").exists());
    let replacement = serve().unwrap();
    assert!(query(
        &client(&replacement.endpoint),
        &replacement.endpoint,
        "{runtime{id}}",
        json!({})
    )
    .get("errors")
    .is_none());
}

mod publications;

mod stopping;

mod coworkers;

#[test]
fn configured_default_message_target_is_visible() {
    fn copy_tree(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).unwrap();
        for entry in std::fs::read_dir(from).unwrap() {
            let entry = entry.unwrap();
            if entry.path().is_dir() {
                copy_tree(&entry.path(), &to.join(entry.file_name()));
            } else {
                std::fs::copy(entry.path(), to.join(entry.file_name())).unwrap();
            }
        }
    }
    let directory = tempfile::tempdir().unwrap();
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../packages/cwr-core/fixtures/contracts/agent-recovery-turn/workspace");
    let workspace = directory.path().join("workspace");
    copy_tree(&source, &workspace);
    let person = workspace.join("coworkers/person/COWORKER.toml");
    let text = std::fs::read_to_string(&person).unwrap();
    std::fs::write(
        person,
        format!("{text}\ndefault_conversation_target = true\n"),
    )
    .unwrap();
    let runtime = LocalRuntime::open_with_state_path_and_runtime_ref(
        workspace.join("CWR.toml"),
        directory.path().join("default.sqlite3"),
        "cwp://example.test/runtime",
        LocalHostIdentity::new(
            cwr::v2026_09::graph::resolve_graph(workspace.join("CWR.toml"))
                .unwrap()
                .actors
                .into_iter()
                .find(|actor| actor.coworker_slug == "person")
                .unwrap()
                .actor_ref
                .as_str(),
            "cwp://example.test/integrations/graphql",
        ),
    )
    .unwrap();
    let server = start(
        runtime,
        "cwp://example.test/runtime".into(),
        directory.path(),
        crate::idle::Activity::default(),
        Arc::new(|_| Ok(())),
        tokio::sync::watch::channel(()).1,
    )
    .unwrap();
    let endpoint = LocalGraphqlEndpoint::read(&directory.path().join("graphql.json")).unwrap();
    let page = query(
        &client(&server.endpoint),
        &endpoint,
        "{runtime{defaultMessageTarget{id handle}}}",
        json!({}),
    );
    assert!(page.get("errors").is_none(), "{page}");
    assert_eq!(
        page["data"]["runtime"]["defaultMessageTarget"]["handle"],
        "@helper"
    );
}

#[test]
fn conversation_page_summaries_reuse_authorized_rows_within_the_read_budget() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = runtime(directory.path());
    let actor = crate::test_refs::actor_ref("person");
    for index in 0..55 {
        runtime
            .submit_text_as(
                "cwp://example.test/integrations/graphql",
                format!("row-{index}"),
                actor.as_str(),
                None,
                format!("Page message {index}"),
            )
            .unwrap();
    }
    let server = start(
        runtime,
        "cwp://example.test/runtime".into(),
        directory.path(),
        crate::idle::Activity::default(),
        Arc::new(|_| Ok(())),
        tokio::sync::watch::channel(()).1,
    )
    .unwrap();
    let result = query(
        &client(&server.endpoint),
        &server.endpoint,
        "{runtime{conversations(first:50){totalCount edges{node{id title archived summary{lastMessageRef lastMessagePreview completedWorkRefs completedWork{invocationRef sequence} revision}}} pageInfo{hasNextPage}}}}",
        json!({}),
    );
    assert!(result.get("errors").is_none(), "{result}");
    let page = &result["data"]["runtime"]["conversations"];
    assert_eq!(page["totalCount"], 55);
    assert_eq!(page["edges"].as_array().unwrap().len(), 50);
    assert_eq!(page["pageInfo"]["hasNextPage"], true);
    assert!(page["edges"].as_array().unwrap().iter().all(|edge| {
        edge["node"]["summary"]["lastMessagePreview"]
            .as_str()
            .is_some_and(|text| text.starts_with("Page message "))
    }));
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

mod action_context;
mod legacy_publications;
mod native_query_cost;
mod results;
mod schedules;
