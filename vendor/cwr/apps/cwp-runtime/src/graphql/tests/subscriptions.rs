use super::*;
use std::os::unix::net::UnixStream;
use tungstenite::{client::IntoClientRequest, Message, WebSocket};

pub(super) type Socket = WebSocket<UnixStream>;
fn connect(endpoint: &LocalGraphqlEndpoint, _token: &str) -> Socket {
    let endpoint = session_endpoint(&client(endpoint), endpoint, None);
    connect_session(&endpoint)
}
pub(super) fn connect_session(endpoint: &LocalGraphqlEndpoint) -> Socket {
    connect_session_with_protocol(endpoint, "graphql-transport-ws")
}
fn connect_session_with_protocol(endpoint: &LocalGraphqlEndpoint, protocols: &str) -> Socket {
    let token = &endpoint.token;
    let mut request = endpoint
        .url()
        .replacen("http:", "ws:", 1)
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert("authorization", format!("Bearer {token}").parse().unwrap());
    request
        .headers_mut()
        .insert("sec-websocket-protocol", protocols.parse().unwrap());
    let (mut socket, response) =
        tungstenite::client(request, UnixStream::connect(&endpoint.socket_path).unwrap()).unwrap();
    assert_eq!(
        response.headers()["sec-websocket-protocol"],
        "graphql-transport-ws"
    );
    {
        socket
            .get_mut()
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
    }
    send(&mut socket, json!({"type":"connection_init"}));
    assert_eq!(read(&mut socket)["type"], "connection_ack");
    socket
}
pub(super) fn send(socket: &mut Socket, value: Value) {
    socket
        .send(Message::Text(value.to_string().into()))
        .unwrap();
}
pub(super) fn read(socket: &mut Socket) -> Value {
    loop {
        match socket.read().unwrap() {
            Message::Text(text) => return serde_json::from_str(text.as_str()).unwrap(),
            Message::Ping(_) => socket.flush().unwrap(),
            message => panic!("unexpected {message:?}"),
        }
    }
}
fn subscribe(socket: &mut Socket, id: &str) {
    send(
        socket,
        json!({"id":id,"type":"subscribe","payload":{"query":"subscription {runtime{conversations{totalCount edges{node{id title legacyTimeline{edges{node{...on Message{id text authorActorRef}}}}}}}}}"}}),
    );
}
fn count(socket: &mut Socket) -> i64 {
    let next = read(socket);
    assert_eq!(next["type"], "next", "{next}");
    assert!(next["payload"].get("errors").is_none(), "{next}");
    next["payload"]["data"]["runtime"]["conversations"]["totalCount"]
        .as_i64()
        .unwrap()
}

#[test]
fn two_clients_receive_authorized_snapshots_reconnect_and_shutdown() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = runtime(directory.path());
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
    let endpoint = server.endpoint.clone();
    let mut first = connect(&endpoint, &endpoint.token);
    let mut second = connect(&endpoint, &endpoint.token);
    subscribe(&mut first, "one");
    subscribe(&mut second, "two");
    assert_eq!(count(&mut first), 0);
    assert_eq!(count(&mut second), 0);
    let actor = crate::test_refs::actor_ref("person");
    runtime
        .submit_text_as(
            "cwp://example.test/integrations/graphql",
            "one",
            actor.as_str(),
            None,
            "Visible message",
        )
        .unwrap();
    changes.send_replace(());
    assert_eq!(count(&mut first), 1);
    assert_eq!(count(&mut second), 1);
    // A second operation on the same connection has its own initial snapshot.
    subscribe(&mut first, "other");
    assert_eq!(count(&mut first), 1);
    send(&mut first, json!({"type":"complete","id":"other"}));
    assert_eq!(read(&mut first)["type"], "complete");
    let helper = crate::test_refs::actor_ref("helper");
    runtime
        .submit_text_as(
            "cwp://example.test/integrations/graphql",
            "private",
            helper.as_str(),
            None,
            "Hidden message",
        )
        .unwrap();
    changes.send_replace(());
    assert_eq!(count(&mut first), 1);
    assert_eq!(count(&mut second), 1);
    first.close(None).unwrap();
    let mut reconnected = connect(&endpoint, &endpoint.token);
    subscribe(&mut reconnected, "reconnected");
    assert_eq!(count(&mut reconnected), 1);
    // Open subscriptions must not hold server destruction hostage.
    drop(server);
    // A graceful WebSocket close can arrive before the transport reports EOF.
    // Both confirm shutdown; ordinary subscription frames must still fail.
    let shutdown = reconnected.read();
    assert!(
        matches!(shutdown, Ok(Message::Close(_)) | Err(_)),
        "expected shutdown, received {shutdown:?}"
    );
    drop(second);
}

#[test]
fn websocket_rejects_invalid_credential_and_browser_origin() {
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
    for (token, origin, status) in [
        ("wrong", None, 401),
        (
            server.endpoint.token.as_str(),
            Some("http://untrusted.test"),
            403,
        ),
    ] {
        let mut request = server
            .endpoint
            .url()
            .replacen("http:", "ws:", 1)
            .into_client_request()
            .unwrap();
        request
            .headers_mut()
            .insert("authorization", format!("Bearer {token}").parse().unwrap());
        request.headers_mut().insert(
            "sec-websocket-protocol",
            "graphql-transport-ws".parse().unwrap(),
        );
        if let Some(origin) = origin {
            request
                .headers_mut()
                .insert("origin", origin.parse().unwrap());
        }
        match tungstenite::client(
            request,
            UnixStream::connect(&server.endpoint.socket_path).unwrap(),
        )
        .unwrap_err()
        {
            tungstenite::HandshakeError::Failure(tungstenite::Error::Http(response)) => {
                assert_eq!(response.status().as_u16(), status)
            }
            error => panic!("{error}"),
        }
    }
}

#[test]
fn operation_capacity_is_released_on_cancellation() {
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
    let mut socket = connect(&server.endpoint, &server.endpoint.token);
    for id in 0..64 {
        send(
            &mut socket,
            json!({"type":"subscribe","id":id.to_string(),"payload":{"query":"subscription{runtime{id}}"}}),
        );
        assert_eq!(read(&mut socket)["type"], "next");
    }
    send(
        &mut socket,
        json!({"type":"subscribe","id":"overflow","payload":{"query":"subscription{runtime{id}}"}}),
    );
    let overflow = read(&mut socket);
    assert!(
        overflow
            .to_string()
            .contains("Subscription capacity exhausted"),
        "{overflow}"
    );
    // The failed operation may emit a completion after the error result.
    if overflow["type"] == "next" {
        assert_eq!(read(&mut socket)["type"], "complete");
    }
    send(&mut socket, json!({"type":"complete","id":"0"}));
    assert_eq!(read(&mut socket)["type"], "complete");
    send(
        &mut socket,
        json!({"type":"subscribe","id":"replacement","payload":{"query":"subscription{runtime{id}}"}}),
    );
    let replacement = read(&mut socket);
    assert_eq!(replacement["type"], "next", "{replacement}");
    assert!(
        replacement["payload"].get("errors").is_none(),
        "{replacement}"
    );
    socket.close(None).unwrap();
}

#[test]
fn websocket_selects_supported_protocol_from_fallback_offers_in_either_order() {
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
    for protocols in [
        "graphql-transport-ws, graphql-ws",
        "graphql-ws, graphql-transport-ws",
    ] {
        let endpoint = session_endpoint(&client(&server.endpoint), &server.endpoint, None);
        let mut socket = connect_session_with_protocol(&endpoint, protocols);
        subscribe(&mut socket, "supported");
        assert_eq!(count(&mut socket), 0);
        socket.close(None).unwrap();
    }
    let endpoint = session_endpoint(&client(&server.endpoint), &server.endpoint, None);
    let mut request = endpoint
        .url()
        .replacen("http:", "ws:", 1)
        .into_client_request()
        .unwrap();
    request.headers_mut().insert(
        "authorization",
        format!("Bearer {}", endpoint.token).parse().unwrap(),
    );
    request
        .headers_mut()
        .insert("sec-websocket-protocol", "graphql-ws".parse().unwrap());
    match tungstenite::client(request, UnixStream::connect(&endpoint.socket_path).unwrap())
        .unwrap_err()
    {
        tungstenite::HandshakeError::Failure(tungstenite::Error::Http(response)) => {
            assert_eq!(response.status().as_u16(), 400)
        }
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn committed_generation_restart_is_delivered_to_graphql_before_disconnect() {
    let directory = tempfile::tempdir().unwrap();
    let activity = crate::idle::Activity::default();
    let server = start(
        runtime(directory.path()),
        "cwp://example.test/runtime".into(),
        directory.path(),
        activity.clone(),
        Arc::new(|_| Ok(())),
        tokio::sync::watch::channel(()).1,
    )
    .unwrap();
    let mut socket = connect(&server.endpoint, &server.endpoint.token);
    send(
        &mut socket,
        json!({"id":"restart","type":"subscribe","payload":{"query":"subscription {runtimeRestarting}"}}),
    );
    // This retained notice also covers a commit racing subscription setup.
    activity.restart_after_disconnect("replacement-generation");
    let notice = read(&mut socket);
    assert_eq!(notice["id"], "restart");
    assert_eq!(
        notice["payload"]["data"]["runtimeRestarting"],
        "replacement-generation"
    );
    assert_eq!(read(&mut socket)["type"], "complete");
}

#[test]
fn shared_fork_action_notifies_and_exposes_its_owned_destination() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = LocalRuntime::open_with_state_path_and_runtime_ref_and_options(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(
            "../../packages/cwr-core/fixtures/contracts/agent-recovery-turn/workspace/CWR.toml",
        ),
        directory.path().join("cwpdb.sqlite3"),
        "cwp://example.test/runtime",
        LocalHostIdentity::new(
            crate::test_refs::actor_ref("person").as_str(),
            "cwp://example.test/integrations/graphql",
        ),
        cwr::v2026_09::local::LocalRuntimeOptions {
            conversation_timeline_rollout:
                cwr::v2026_09::conversation::ConversationTimelineRollout::NewConversations,
            ..Default::default()
        },
    )
    .unwrap();
    let source = runtime
        .submit_text_as(
            "cwp://example.test/integrations/graphql",
            "fork-source",
            crate::test_refs::actor_ref("person").as_str(),
            None,
            "Branch this",
        )
        .unwrap();
    let (changes, observe) = tokio::sync::watch::channel(());
    let executor = runtime.clone();
    let server = start(
        runtime,
        "cwp://example.test/runtime".into(),
        directory.path(),
        crate::idle::Activity::default(),
        Arc::new(move |_| {
            if let Some(mut task) = executor.try_schedule_execution()? {
                task.execute_with_model_output(
                    &cwr::adapter::AdapterModelOutputMailbox::discard(),
                    |_| {},
                )?;
            }
            changes.send_replace(());
            Ok(())
        }),
        observe,
    )
    .unwrap();
    let client = client(&server.endpoint);
    let endpoint = session_endpoint(&client, &server.endpoint, None);
    let placement = query_session(
        &client,
        &endpoint,
        "query($ref:ID!){runtime{conversation(ref:$ref){timeline(first:10){edges{node{id}}}}}}",
        json!({"ref":source.conversation_ref()}),
    );
    let item = placement["data"]["runtime"]["conversation"]["timeline"]["edges"][0]["node"]["id"]
        .as_str()
        .unwrap();
    let mut socket = connect_session(&endpoint);
    send(
        &mut socket,
        json!({"id":"fork","type":"subscribe","payload":{"query":"subscription{runtime{conversations{totalCount}}}"}}),
    );
    assert_eq!(count(&mut socket), 1);
    let mutation = "mutation($url:ID!,$input:JSON!){invokeAction(url:$url,activation:\"@cwr--fork--conversation\",input:$input,requestId:\"shared-fork\"){...on CompletedInvocation{id state forkedConversationRef actionPresentation{title resultMessage { id action { ...on MessageActionResult { result } } } links{kind label reference}}} ...on ActionError{code message}}}";
    let variables = json!({"url":source.conversation_ref(),"input":{"message_ref":source.record.source_message_ref,"timeline_item_ref":item}});
    let result = query_session(&client, &endpoint, mutation, variables.clone());
    assert!(result.get("errors").is_none(), "{result}");
    assert_eq!(
        result["data"]["invokeAction"]["state"], "completed",
        "{result}"
    );
    assert!(
        result["data"]["invokeAction"]["forkedConversationRef"].is_string(),
        "{result}"
    );
    assert_eq!(
        result["data"]["invokeAction"]["actionPresentation"]["title"],
        "Fork conversation"
    );
    assert_eq!(
        result["data"]["invokeAction"]["actionPresentation"]["links"][0]["reference"],
        result["data"]["invokeAction"]["forkedConversationRef"]
    );
    assert_eq!(
        result["data"]["invokeAction"]["actionPresentation"]["links"][0]["kind"],
        "CONVERSATION"
    );
    assert!(
        result["data"]["invokeAction"]["actionPresentation"]["resultMessage"]["action"]["result"]
            .is_object(),
        "{result}"
    );
    assert_eq!(count(&mut socket), 2);
    let retry = query_session(&client, &endpoint, mutation, variables);
    assert_eq!(retry, result);
    assert_eq!(count(&mut socket), 2);
    socket.close(None).unwrap();
    drop(server);
}
