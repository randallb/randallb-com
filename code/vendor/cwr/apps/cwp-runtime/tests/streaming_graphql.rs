//! Real resident + process adapter + GraphQL transport, without a provider account.
use cwr::v2026_09::local::LocalGraphqlEndpoint;
use serde_json::{json, Value};
use std::{
    fs,
    io::Write,
    os::unix::{fs::PermissionsExt, net::UnixStream},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
use tungstenite::{client::IntoClientRequest, Message};
struct Resident(Child);
impl Drop for Resident {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn query(
    client: &reqwest::blocking::Client,
    endpoint: &LocalGraphqlEndpoint,
    query: &str,
    variables: Value,
) -> Value {
    let value: Value = client
        .post(endpoint.url())
        .bearer_auth(&endpoint.token)
        .json(&json!({"query":query,"variables":variables}))
        .send()
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .unwrap();
    assert!(value.get("errors").is_none(), "{value}");
    value
}
fn source(value: &Value, kind: &str) -> Option<Value> {
    match value {
        Value::Object(object) => {
            if object.get("__typename").is_some_and(|v| v == kind) {
                return Some(value.clone());
            }
            object.values().find_map(|v| source(v, kind))
        }
        Value::Array(array) => array.iter().find_map(|v| source(v, kind)),
        _ => None,
    }
}
fn connect_socket(endpoint: &LocalGraphqlEndpoint) -> tungstenite::WebSocket<UnixStream> {
    let until = Instant::now() + Duration::from_secs(2);
    loop {
        let mut request = endpoint
            .url()
            .replacen("http:", "ws:", 1)
            .into_client_request()
            .unwrap();
        request.headers_mut().insert(
            "authorization",
            format!("Bearer {}", endpoint.token).parse().unwrap(),
        );
        request.headers_mut().insert(
            "sec-websocket-protocol",
            "graphql-transport-ws".parse().unwrap(),
        );
        let stream = UnixStream::connect(&endpoint.socket_path).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let (mut socket, _) = match tungstenite::client(request, stream) {
            Ok(value) => value,
            Err(tungstenite::HandshakeError::Failure(tungstenite::Error::Http(response)))
                if response.status().as_u16() == 409 && Instant::now() < until =>
            {
                std::thread::sleep(Duration::from_millis(10));
                continue;
            }
            Err(error) => panic!("{error}"),
        };
        socket
            .send(Message::Text(
                json!({"type":"connection_init"}).to_string().into(),
            ))
            .unwrap();
        socket.read().unwrap();
        return socket;
    }
}
fn workspace(root: &Path) -> PathBuf {
    let directory = root.join("workspace");
    fs::create_dir_all(&directory).unwrap();
    for (file, text) in [
        ("CWR.toml", "type='cwp://org.coworkerprotocol/2026-09/schemas'\n[[imports]]\npath='./person.toml'\nas='person'\n[[imports]]\npath='./worker.toml'\nas='worker'\n"),
        ("person.toml", "type='cwp://org.coworkerprotocol/2026-09/schemas/coworker'\nname='Person'\nslug='person'\n"),
        ("worker.toml", "type='cwp://org.coworkerprotocol/2026-09/schemas/coworker'\nname='Worker'\nslug='worker'\n[[roles]]\npath='./ROLE.toml'\n[[integrations]]\npath='./integration.toml'\nas='local'\n"),
        ("ROLE.toml", "type='cwp://org.coworkerprotocol/2026-09/schemas/role'\n[[adapters]]\nref='cwp://org.coworkerprotocol/2026-09/adapters/codex'\n[adapters.params]\nentrypoint='./PROMPT.md'\n[[actions]]\nverb='run'\nobject='task'\ntitle='Run task'\ndescription='Stream a fixture'\n[actions.input_schema]\npath='./input.json'\n[actions.output_schema]\npath='./output.json'\n"),
        ("integration.toml", "type='cwp://org.coworkerprotocol/2026-09/schemas/integration'\n[adapter]\nref='cwp://org.coworkerprotocol/2026-09/adapters/codex'\n"),
        ("input.json", "{\"type\":\"object\"}"), ("output.json", "{\"type\":\"object\"}")
    ] { fs::write(directory.join(file), text).unwrap(); }
    fs::write(directory.join("PROMPT.md"), "Produce the fixture result.").unwrap();
    directory.join("CWR.toml")
}
fn check_initial_inspector(
    client: &reqwest::blocking::Client,
    endpoint: &LocalGraphqlEndpoint,
    invocation: &Value,
    operator_token: &str,
) -> (&'static str, Value, String) {
    let inspector_document = r#"query($ref:ID!,$after:Cursor){runtime{invocation(ref:$ref){
        activities(first:1,after:$after){coverage pageInfo{hasNextPage endCursor} edges{cursor node{id state detailsAvailable output{text truncated}}}}
        calls(first:1){edges{node{callId}}} waits(first:1){edges{node{callId state}}}
    }}}"#;
    let inspector_initial = query(
        client,
        endpoint,
        inspector_document,
        json!({"ref":invocation}),
    );
    let activities = &inspector_initial["data"]["runtime"]["invocation"]["activities"];
    assert_eq!(activities["coverage"], "PARTIAL");
    assert_eq!(activities["pageInfo"]["hasNextPage"], true);
    assert_eq!(activities["edges"][0]["node"]["state"], "running");
    let activity_id = activities["edges"][0]["node"]["id"].clone();
    let activity_cursor = activities["pageInfo"]["endCursor"].clone();
    let next = query(
        client,
        endpoint,
        inspector_document,
        json!({"ref":invocation,"after":activity_cursor}),
    );
    assert_ne!(
        next["data"]["runtime"]["invocation"]["activities"]["edges"][0]["node"]["id"],
        activity_id
    );
    let other: Value = client
        .post(format!("{}/session", endpoint.url()))
        .bearer_auth(operator_token)
        .json(&json!({}))
        .send()
        .unwrap()
        .json()
        .unwrap();
    let foreign:Value=client.post(endpoint.url()).bearer_auth(other["token"].as_str().unwrap())
        .json(&json!({"query":inspector_document,"variables":{"ref":invocation,"after":activity_cursor}}))
        .send().unwrap().json().unwrap();
    assert!(foreign.get("errors").is_some(), "{foreign}");
    client
        .delete(format!("{}/session", endpoint.url()))
        .bearer_auth(other["token"].as_str().unwrap())
        .send()
        .unwrap();
    // Scalar aliases cannot bypass the complete inspector response limit.
    let aliases = (0..300)
        .map(|i| format!("l{i}:label"))
        .collect::<Vec<_>>()
        .join(" ");
    let oversized=format!("query($ref:ID!){{runtime{{invocation(ref:$ref){{activities(first:2){{edges{{node{{{aliases}}}}}}}}}}}}}");
    let capped: Value = client
        .post(endpoint.url())
        .bearer_auth(&endpoint.token)
        .json(&json!({"query":oversized,"variables":{"ref":invocation}}))
        .send()
        .unwrap()
        .json()
        .unwrap();
    assert!(
        capped["errors"][0]["message"]
            .as_str()
            .is_some_and(|s| s.contains("512 KiB")),
        "{capped}"
    );
    (inspector_document, activity_id, oversized)
}
fn check_attachments(
    client: &reqwest::blocking::Client,
    endpoint: &LocalGraphqlEndpoint,
    message: &Value,
) {
    let attachments = query(
        client,
        endpoint,
        r#"query($ref:ID!){runtime{message(ref:$ref){attachments(filter:{search:" RESULT ",mediaTypes:["text/html"]},first:1){totalCount edges{node{id title mediaType ...on HtmlVisualization{html}}}}}}}"#,
        json!({"ref":message}),
    );
    let collection = &attachments["data"]["runtime"]["message"]["attachments"];
    assert_eq!(collection["totalCount"], 1, "{attachments}");
    let artifact = &collection["edges"][0]["node"];
    assert_eq!(artifact["title"], "Result chart");
    assert_eq!(artifact["html"], "<p>Streaming result chart</p>");
    let direct=query(client,endpoint,"query($ref:ID!){runtime{node(ref:$ref){id ...on HtmlVisualization{title mediaType html}}}}",json!({"ref":artifact["id"]}));
    assert_eq!(direct["data"]["runtime"]["node"], *artifact);
}
fn resident_fixture() -> (
    tempfile::TempDir,
    Resident,
    LocalGraphqlEndpoint,
    reqwest::blocking::Client,
    String,
) {
    resident_fixture_with_configuration("{}")
}
fn resident_fixture_with_configuration(
    configuration: &str,
) -> (
    tempfile::TempDir,
    Resident,
    LocalGraphqlEndpoint,
    reqwest::blocking::Client,
    String,
) {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let manifest = workspace(root);
    let actor = cwr::v2026_09::local::load_default_local_acting_actor_ref(&manifest).unwrap();
    let runtime_directory = root.join("runtime");
    let config = root.join("runtime.toml");
    fs::write(&config, format!("version=1\nworkspace={:?}\nstate_directory={:?}\nruntime_directory={:?}\nruntime_root_ref='cwp://example.test/runtime'\nacting_actor_ref={actor:?}\n", manifest.to_str().unwrap(), root.join("state").to_str().unwrap(), runtime_directory.to_str().unwrap())).unwrap();
    let adapter = root.join("adapter");
    fs::write(&adapter, include_str!("streaming_adapter.py")).unwrap();
    fs::set_permissions(&adapter, fs::Permissions::from_mode(0o755)).unwrap();
    let provider = root.join("codex-fixture");
    fs::write(&provider, "#!/bin/sh\nexit 0\n").unwrap();
    fs::set_permissions(&provider, fs::Permissions::from_mode(0o755)).unwrap();
    let log = fs::File::create(root.join("resident.log")).unwrap();
    let mut resident = Resident(
        Command::new(env!("CARGO_BIN_EXE_cwp-runtime"))
            .arg(&config)
            .env("WORKLOOP_CONFIGURATION", configuration)
            .env("CWR_CODEX_ADAPTER_EXECUTABLE", &adapter)
            .env("CWR_CODEX_EXECUTABLE", &provider)
            .env("CWR_ADAPTER_DATA_ROOT", root.join("adapter-data"))
            .env("CWR_CODEX_SOURCE_HOME", root.join("empty-source"))
            .env_remove("CWR_DEV_RUNTIME_GENERATION")
            .env_remove("CWR_DEV_RUNTIME_GENERATION_FILE")
            .stdout(Stdio::null())
            .stderr(log)
            .spawn()
            .unwrap(),
    );
    let until = Instant::now() + Duration::from_secs(20);
    let mut endpoint = loop {
        if let Ok(value) = LocalGraphqlEndpoint::read(&runtime_directory.join("graphql.json")) {
            break value;
        }
        assert!(
            resident.0.try_wait().unwrap().is_none(),
            "{}",
            fs::read_to_string(root.join("resident.log")).unwrap()
        );
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(10));
    };
    let operator_token = endpoint.token.clone();
    let client = reqwest::blocking::Client::builder()
        .unix_socket(endpoint.socket_path.clone())
        .no_proxy()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    let session: Value = client
        .post(format!("{}/session", endpoint.url()))
        .bearer_auth(&endpoint.token)
        .json(&json!({}))
        .send()
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .unwrap();
    endpoint.token = session["token"].as_str().unwrap().into();
    (directory, resident, endpoint, client, operator_token)
}
#[test]
fn resident_streaming_message_finalizes_with_the_same_ref_over_http_and_subscription() {
    let (directory, _resident, mut endpoint, client, operator_token) = resident_fixture();
    let root = directory.path();
    let until = Instant::now() + Duration::from_secs(20);
    let admitted = query(&client, &endpoint, "mutation($url:ID!){invokeAction(url:$url,activation:\"@worker--run--task\",input:{},requestId:\"stream\"){...on ActionInvocation{conversationRef id} ...on ActionError{code message}}}", json!({"url":endpoint.runtime_ref}));
    let conversation = admitted["data"]["invokeAction"]["conversationRef"].clone();
    assert!(conversation.is_string(), "{admitted}");
    let invocation = admitted["data"]["invokeAction"]["id"].clone();
    let selection = r#"
      runtime {
        conversation(ref:$ref) {
          timeline(first:10) {
            edges {
              node {
                id
                children(first:10) {
                  edges {
                    node {
                      id
                      source { ...on ResolvedSource { node { __typename id ...on Message { text addressingMode } } } }
                    }
                  }
                }
              }
            }
          }
        }
      }
    "#;
    let inspector_selection=format!("inspector:invocation(ref:{}){{activities(first:1){{edges{{node{{id state output{{text truncated}}}}}}}}}}",serde_json::to_string(invocation.as_str().unwrap()).unwrap());
    let selection =
        selection.replacen("runtime {", &format!("runtime {{ {inspector_selection}"), 1);
    let document = format!("query($ref:ID!){{{selection}}}");
    let streaming = loop {
        let value = query(&client, &endpoint, &document, json!({"ref":conversation}));
        if let Some(value) = source(&value, "StreamingMessage") {
            break value;
        }
        assert!(
            Instant::now() < until,
            "no stream: {value}; {}",
            fs::read_to_string(root.join("resident.log")).unwrap()
        );
        std::thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(streaming["text"], "Partial answer");
    assert_eq!(streaming["addressingMode"], "IMPLICIT");
    let direct = query(&client, &endpoint, "query($ref:ID!){runtime{node(ref:$ref){__typename id ...on Message{text addressingMode}} message(ref:$ref){__typename id text addressingMode}}}", json!({"ref":streaming["id"]}));
    assert_eq!(
        direct["data"]["runtime"]["node"],
        direct["data"]["runtime"]["message"]
    );
    let (inspector_document, activity_id, oversized) =
        check_initial_inspector(&client, &endpoint, &invocation, &operator_token);
    let mut socket = connect_socket(&endpoint);
    socket.send(Message::Text(json!({"id":"large","type":"subscribe","payload":{"query":oversized.replacen("query(","subscription(",1),"variables":{"ref":invocation}}}).to_string().into())).unwrap();
    let capped: Value = serde_json::from_str(socket.read().unwrap().to_text().unwrap()).unwrap();
    assert!(
        capped["payload"]["errors"][0]["message"]
            .as_str()
            .is_some_and(|s| s.contains("512 KiB")),
        "{capped}"
    );
    socket
        .send(Message::Text(
            json!({"id":"large","type":"complete"}).to_string().into(),
        ))
        .unwrap();
    let completion: Value =
        serde_json::from_str(socket.read().unwrap().to_text().unwrap()).unwrap();
    assert_eq!(completion, json!({"id":"large","type":"complete"}));
    socket.send(Message::Text(json!({"id":"stream","type":"subscribe","payload":{"query":format!("subscription($ref:ID!){{{selection}}}"),"variables":{"ref":conversation}}}).to_string().into())).unwrap();
    let initial: Value = serde_json::from_str(socket.read().unwrap().to_text().unwrap()).unwrap();
    assert!(source(&initial, "StreamingMessage").is_some(), "{initial}");
    let filtered = selection.replace(
        "children(first:10)",
        "children(first:10,filter:{search:\"partial\"})",
    );
    socket.send(Message::Text(json!({"id":"filtered","type":"subscribe","payload":{"query":format!("subscription($ref:ID!){{{filtered}}}"),"variables":{"ref":conversation}}}).to_string().into())).unwrap();
    let initial: Value = serde_json::from_str(socket.read().unwrap().to_text().unwrap()).unwrap();
    assert!(source(&initial, "StreamingMessage").is_some(), "{initial}");
    fs::File::create(root.join("finish"))
        .unwrap()
        .write_all(b"go")
        .unwrap();
    let mut removed = false;
    let mut published = None;
    let mut activity_completed = false;
    let final_node = loop {
        let frame = socket.read().unwrap_or_else(|error| {
            panic!(
                "{error}; {}",
                fs::read_to_string(root.join("resident.log")).unwrap()
            )
        });
        let Message::Text(text) = frame else {
            socket.flush().unwrap();
            continue;
        };
        let value: Value = serde_json::from_str(text.as_str()).unwrap();
        if value["id"] == "filtered" && source(&value, "StreamingMessage").is_none() {
            assert!(source(&value, "PublishedMessage").is_none(), "{value}");
            removed = true;
        }
        let work =
            &value["payload"]["data"]["runtime"]["inspector"]["activities"]["edges"][0]["node"];
        if work["id"] == activity_id && work["state"] == "completed" {
            assert_eq!(work["output"]["truncated"], true);
            activity_completed = true;
        }
        if let Some(node) = source(&value, "PublishedMessage") {
            if node["id"] == streaming["id"] {
                published = Some(node);
            }
        }
        if let (true, true, Some(node)) = (removed, activity_completed, published.as_ref()) {
            break node.clone();
        }
        assert!(Instant::now() < until, "{value}");
    };
    assert_ne!(final_node["text"], streaming["text"]);
    assert_eq!(final_node["addressingMode"], "IMPLICIT");
    let inspector_final = query(
        &client,
        &endpoint,
        inspector_document,
        json!({"ref":invocation}),
    );
    let completed =
        &inspector_final["data"]["runtime"]["invocation"]["activities"]["edges"][0]["node"];
    assert_eq!(completed["id"], activity_id);
    assert_eq!(completed["state"], "completed");
    assert_eq!(completed["output"]["truncated"], true);
    assert_eq!(completed["output"]["text"].as_str().unwrap().len(), 16384);
    let direct = query(
        &client,
        &endpoint,
        "query($ref:ID!){runtime{node(ref:$ref){__typename id ...on Message{text addressingMode}}}}",
        json!({"ref":streaming["id"]}),
    );
    assert_eq!(direct["data"]["runtime"]["node"], final_node);
    let result=query(&client,&endpoint,"query($ref:ID!){runtime{message(ref:$ref){action{__typename invocation{id requester{id} target{id}} ...on MessageActionResult{status summary}}}}}",json!({"ref":streaming["id"]}));
    let action = &result["data"]["runtime"]["message"]["action"];
    assert_eq!(action["__typename"], "MessageActionResult", "{result}");
    assert_eq!(action["status"], 200);
    assert!(
        action["invocation"]["requester"]["id"].is_string(),
        "{result}"
    );
    assert!(action["invocation"]["target"]["id"].is_string(), "{result}");
    assert_eq!(action["summary"], final_node["text"]);
    assert_eq!(
        action["invocation"]["id"],
        admitted["data"]["invokeAction"]["id"]
    );
    let changed=query(&client,&endpoint,"query($ref:ID!){runtime{invocation(ref:$ref){activities(last:1){edges{node{files(first:1){edges{node{path changeKind diff{text truncated}}}}}}}}}}",json!({"ref":invocation}));
    let file = &changed["data"]["runtime"]["invocation"]["activities"]["edges"][0]["node"]["files"]
        ["edges"][0]["node"];
    assert_eq!(file["path"], "fixture.rs");
    assert_eq!(file["diff"]["text"], "+fixture");
    check_attachments(&client, &endpoint, &streaming["id"]);
    socket.close(None).unwrap();
    let session: Value = client
        .post(format!("{}/session", endpoint.url()))
        .bearer_auth(&operator_token)
        .json(&json!({}))
        .send()
        .unwrap()
        .json()
        .unwrap();
    endpoint.token = session["token"].as_str().unwrap().into();
    let mut reconnected = connect_socket(&endpoint);
    reconnected.send(Message::Text(json!({"id":"reconnect","type":"subscribe","payload":{"query":inspector_document.replacen("query(","subscription(",1),"variables":{"ref":invocation}}}).to_string().into())).unwrap();
    let value: Value =
        serde_json::from_str(reconnected.read().unwrap().to_text().unwrap()).unwrap();
    assert!(value["payload"].get("errors").is_none(), "{value}");
    assert_eq!(
        value["payload"]["data"]["runtime"]["invocation"]["activities"]["edges"][0]["node"],
        *completed
    );
    reconnected.close(None).unwrap();
}

#[test]
fn resident_captures_tool_activity_without_a_rollout_switch() {
    let (_directory, _resident, endpoint, client, _operator_token) =
        resident_fixture_with_configuration(r#"{}"#);
    let admitted=query(&client,&endpoint,
        "mutation($url:ID!){invokeAction(url:$url,activation:\"@worker--run--task\",input:{},requestId:\"no-rollout\"){...on ActionInvocation{id} ...on ActionError{code message}}}",
        json!({"url":endpoint.runtime_ref}));
    let invocation = admitted["data"]["invokeAction"]["id"].clone();
    assert!(invocation.is_string(), "{admitted}");
    let until = Instant::now() + Duration::from_secs(20);
    loop {
        let value = query(
            &client,
            &endpoint,
            "query($ref:ID!){runtime{invocation(ref:$ref){id activities(first:1){edges{node{id}}}}}}",
            json!({"ref":invocation}),
        );
        if value["data"]["runtime"]["invocation"]["activities"]["edges"][0]["node"]["id"]
            .is_string()
        {
            break;
        }
        assert!(Instant::now() < until, "no captured tool activity: {value}");
        std::thread::sleep(Duration::from_millis(10));
    }
}
