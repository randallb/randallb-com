use super::subscriptions::{connect_session, read, send, Socket};
use super::*;

fn message(endpoint: &LocalGraphqlEndpoint, key: &str, text: &str) -> Value {
    query_session(
        &client(endpoint),
        endpoint,
        "mutation($key:ID!,$text:String!){createConversation(requestId:$key,text:$text){conversationRef conversation { id } messageRef}}",
        json!({"key":key,"text":text}),
    )
}
#[test]
fn independent_credentials_bind_authorship_and_do_not_grant_bootstrap_authority() {
    let directory = tempfile::tempdir().unwrap();
    let server = serve(directory.path(), runtime(directory.path()));
    let client = client(&server.endpoint);
    let person = session_endpoint(&client, &server.endpoint, None);
    let helper = session_endpoint(
        &client,
        &server.endpoint,
        Some(&crate::test_refs::actor_ref("helper")),
    );
    for endpoint in [&person, &helper] {
        assert_eq!(
            client
                .post(format!("{}/session", endpoint.url()))
                .bearer_auth(&endpoint.token)
                .json(&json!({"actorRef":server.endpoint.actor_ref}))
                .send()
                .unwrap()
                .status(),
            401
        );
        let result = query_session(
            &client,
            endpoint,
            "{runtime{actorRef messageTargets { id handle name }}}",
            json!({}),
        );
        assert_eq!(result["data"]["runtime"]["actorRef"], endpoint.actor_ref);
        let targets = result["data"]["runtime"]["messageTargets"]
            .as_array()
            .unwrap();
        assert!(!targets.is_empty());
        assert!(targets
            .iter()
            .all(|target| target["id"] != endpoint.actor_ref));
    }
    assert_eq!(
        client
            .post(server.endpoint.url())
            .bearer_auth(&server.endpoint.token)
            .json(&json!({"query":"{runtime{id}}"}))
            .send()
            .unwrap()
            .status(),
        401
    );
    let first = message(&person, "same-key", "Person's message");
    let other = message(&helper, "same-key", "Helper's message");
    assert!(first.get("errors").is_none(), "{first}");
    assert!(other.get("errors").is_none(), "{other}");
    assert_ne!(
        first["data"]["createConversation"]["messageRef"],
        other["data"]["createConversation"]["messageRef"]
    );
    assert_eq!(
        first["data"]["createConversation"]["conversation"]["id"],
        first["data"]["createConversation"]["conversationRef"]
    );
    assert_eq!(first, message(&person, "same-key", "Person's message"));
    for (endpoint, own, foreign) in [(&person, &first, &other), (&helper, &other, &first)] {
        let result = query_session(
            &client,
            endpoint,
            "query($own:ID!,$foreign:ID!){runtime{conversations{totalCount} own:conversation(id:$own){legacyTimeline{edges{node{...on Message{authorActorRef text}}}}} foreign:conversation(id:$foreign){id}}}",
            json!({"own":own["data"]["createConversation"]["conversationRef"],"foreign":foreign["data"]["createConversation"]["conversationRef"]}),
        );
        assert!(result.get("errors").is_none(), "{result}");
        assert_eq!(result["data"]["runtime"]["conversations"]["totalCount"], 1);
        assert!(result["data"]["runtime"]["foreign"].is_null());
        assert_eq!(
            result["data"]["runtime"]["own"]["legacyTimeline"]["edges"][0]["node"]
                ["authorActorRef"],
            endpoint.actor_ref
        );
    }
    let shared = message(&person, "shared", "@helper A shared conversation");
    let shared_ref = &shared["data"]["createConversation"]["conversationRef"];
    let reply = query_session(
        &client,
        &helper,
        "mutation($conversation:ID!){sendMessage(text:\"A second actor writes here\",requestId:\"shared-reply\",conversationRef:$conversation){conversationRef conversation { id } messageRef}}",
        json!({"conversation":shared_ref}),
    );
    assert!(reply.get("errors").is_none(), "{reply}");
    for endpoint in [&person, &helper] {
        let view = query_session(
            &client,
            endpoint,
            "query($id:ID!){runtime{conversation(id:$id){legacyTimeline{edges{node{...on Message{authorActorRef}}}}}}}",
            json!({"id":shared_ref}),
        );
        assert!(view.get("errors").is_none(), "{view}");
        let messages = view["data"]["runtime"]["conversation"]["legacyTimeline"]["edges"]
            .as_array()
            .unwrap();
        assert!(messages
            .iter()
            .any(|m| m["node"]["authorActorRef"] == person.actor_ref));
        assert!(messages
            .iter()
            .any(|m| m["node"]["authorActorRef"] == helper.actor_ref));
    }
    for (endpoint, targets) in [(&person, vec![helper.actor_ref.clone()]), (&helper, vec![])] {
        let view = query_session(
            &client,
            endpoint,
            "query($id:ID!){runtime{conversation(ref:$id){lastAddressedActorRefs}}}",
            json!({"id": shared_ref}),
        );
        assert!(view.get("errors").is_none(), "{view}");
        assert_eq!(
            view["data"]["runtime"]["conversation"]["lastAddressedActorRefs"],
            json!(targets)
        );
    }
    client
        .delete(format!("{}/session", person.url()))
        .bearer_auth(&person.token)
        .send()
        .unwrap()
        .error_for_status()
        .unwrap();
    assert_eq!(
        client
            .post(person.url())
            .bearer_auth(&person.token)
            .json(&json!({"query":"{runtime{id}}"}))
            .send()
            .unwrap()
            .status(),
        401
    );
    assert!(query_session(&client, &helper, "{runtime{id}}", json!({}))
        .get("errors")
        .is_none());
}
pub(super) fn terminal(endpoint: &LocalGraphqlEndpoint, label: &str) -> (Socket, String, String) {
    let mut socket = connect_session(endpoint);
    send(
        &mut socket,
        json!({"id":"events","type":"subscribe","payload":{"query":"subscription{terminalEvents{__typename ...on SessionReady{sessionRef} ...on TerminalWork{invocationRef input providerSessionRef targetActorRef coworkerRef} ...on TerminalStop{invocationRef} ...on TerminalResultValidation{validationRef}}}"}}),
    );
    let ready = read(&mut socket);
    assert_eq!(
        ready["payload"]["data"]["terminalEvents"]["__typename"], "SessionReady",
        "{ready}"
    );
    let actor = format!("cwp://example.test/terminals/{label}/actors/tui");
    let coworker = json!({"actorRef":actor,"coworkerRef":format!("cwp://example.test/terminals/{label}/coworkers/tui"),"handle":"@tui","name":"Terminal","actions":[{"actionName":"run--task","title":"Run","description":"Run","inputSchema":{"type":"object","additionalProperties":false},"outputSchema":{"type":"object"}}]});
    let registered = query_session(
        &client(endpoint),
        endpoint,
        "mutation($coworkers:[TerminalCoworkerInput!]!){registerTerminalProvider(coworkers:$coworkers)}",
        json!({"coworkers":[coworker]}),
    );
    assert!(registered.get("errors").is_none(), "{registered}");
    let provider = registered["data"]["registerTerminalProvider"]
        .as_str()
        .unwrap()
        .to_owned();
    let enabled = enable(endpoint, &provider, &actor);
    assert!(enabled.get("errors").is_none(), "{enabled}");
    (socket, provider, actor)
}
fn enable(endpoint: &LocalGraphqlEndpoint, provider: &str, actor: &str) -> Value {
    query_session(
        &client(endpoint),
        endpoint,
        "mutation($provider:ID!,$actor:ID!){enableTerminalCoworker(providerSessionRef:$provider,actorRef:$actor,enabled:true)}",
        json!({"provider":provider,"actor":actor}),
    )
}
fn complete(endpoint: &LocalGraphqlEndpoint, id: &str) -> Value {
    query_session(
        &client(endpoint),
        endpoint,
        "mutation($id:ID!,$terminal:JSON!){completeTerminalWork(invocationRef:$id,terminal:$terminal)}",
        json!({"id":id,"terminal":{"invocation_ref":id,"sequence":1,"status":200,"summary":"Done","result":{}}}),
    )
}
#[test]
fn same_actor_terminals_keep_private_catalogs_delivery_and_completion_ownership() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = runtime(directory.path());
    let server = serve(directory.path(), runtime.clone());
    let first = session_endpoint(&client(&server.endpoint), &server.endpoint, None);
    let second = session_endpoint(&client(&server.endpoint), &server.endpoint, None);
    let (mut first_socket, first_provider, first_actor) = terminal(&first, "first");
    let (mut second_socket, second_provider, second_actor) = terminal(&second, "second");
    assert_ne!(first_provider, second_provider);
    assert!(enable(&second, &first_provider, &first_actor)
        .get("errors")
        .is_some());
    assert!(enable(&first, &second_provider, &second_actor)
        .get("errors")
        .is_some());
    for endpoint in [&first, &second] {
        let result = query_session(
            &client(&server.endpoint),
            endpoint,
            "{runtime{actions{activation} messageTargets{id}}}",
            json!({}),
        );
        assert!(result.get("errors").is_none(), "{result}");
        assert_eq!(
            result["data"]["runtime"]["actions"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|a| a["activation"] == "@tui--run--task")
                .count(),
            1
        );
    }
    let admitted = query_session(
        &client(&server.endpoint),
        &first,
        "mutation($url:ID!){invokeAction(url:$url,activation:\"@tui--run--task\",input:{},requestId:\"terminal-work\"){...on ActionInvocation{id} ...on ActionError{code}}}",
        json!({"url":first.runtime_ref}),
    );
    assert!(admitted.get("errors").is_none(), "{admitted}");
    let id = admitted["data"]["invokeAction"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let executing = runtime.clone();
    let worker = std::thread::spawn(move || executing.execute_pending(|_| {}));
    let event = read(&mut first_socket);
    assert_eq!(
        event["payload"]["data"]["terminalEvents"]["invocationRef"], id,
        "{event}"
    );
    assert_eq!(
        event["payload"]["data"]["terminalEvents"]["providerSessionRef"],
        first_provider
    );
    assert_eq!(
        event["payload"]["data"]["terminalEvents"]["targetActorRef"],
        first_actor
    );
    let running = query_session(
        &client(&server.endpoint),
        &first,
        "query($id:ID!){runtime{invocation(id:$id){__typename id state outcome{status}}}}",
        json!({"id":id}),
    );
    assert_eq!(
        running["data"]["runtime"]["invocation"]["__typename"], "RunningInvocation",
        "{running}"
    );
    assert!(running["data"]["runtime"]["invocation"]["outcome"].is_null());
    assert!(complete(&second, &id).get("errors").is_some());
    let completed = complete(&first, &id);
    assert!(completed.get("errors").is_none(), "{completed}");
    worker.join().unwrap().unwrap();
    let terminal = query_session(
        &client(&server.endpoint),
        &first,
        "query($id:ID!){runtime{invocation(id:$id){__typename id state outcome{status}}}}",
        json!({"id":id}),
    );
    assert_eq!(
        terminal["data"]["runtime"]["invocation"]["__typename"], "CompletedInvocation",
        "{terminal}"
    );
    assert_eq!(terminal["data"]["runtime"]["invocation"]["id"], id);
    assert_eq!(
        terminal["data"]["runtime"]["invocation"]["outcome"]["status"],
        200
    );

    // Work already delivered to a disappearing terminal must fail there, never
    // move to the other provider advertising the same handle.
    let pending = query_session(
        &client(&server.endpoint),
        &first,
        "mutation($url:ID!){invokeAction(url:$url,activation:\"@tui--run--task\",input:{},requestId:\"disconnect-work\"){...on ActionInvocation{id}}}",
        json!({"url":first.runtime_ref}),
    );
    let pending_id = pending["data"]["invokeAction"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let executing = runtime.clone();
    let interrupted = std::thread::spawn(move || executing.execute_pending(|_| {}));
    assert_eq!(
        read(&mut first_socket)["payload"]["data"]["terminalEvents"]["invocationRef"],
        pending_id
    );
    // Closing one terminal revokes its credential and does not disturb the other.
    first_socket.close(None).unwrap();
    let limit = std::time::Instant::now() + Duration::from_secs(2);
    while client(&server.endpoint)
        .post(first.url())
        .bearer_auth(&first.token)
        .json(&json!({"query":"{runtime{id}}"}))
        .send()
        .unwrap()
        .status()
        != 401
    {
        assert!(std::time::Instant::now() < limit);
        std::thread::sleep(Duration::from_millis(10));
    }
    interrupted.join().unwrap().unwrap();
    let failed = query_session(
        &client(&server.endpoint),
        &second,
        "query($id:ID!){runtime{invocation(id:$id){state}}}",
        json!({"id":pending_id}),
    );
    assert!(
        matches!(
            failed["data"]["runtime"]["invocation"]["state"].as_str(),
            Some("failed" | "stopped" | "blocked")
        ),
        "{failed}"
    );
    let replacement = session_endpoint(&client(&server.endpoint), &server.endpoint, None);
    assert!(enable(&replacement, &first_provider, &first_actor)
        .get("errors")
        .is_some());
    assert!(complete(&replacement, &id).get("errors").is_some());
    assert!(enable(&second, &second_provider, &second_actor)
        .get("errors")
        .is_none());
    second_socket.close(None).unwrap();
}

#[test]
fn operator_session_ingress_scopes_admission_and_cannot_be_changed_by_session_credentials() {
    let directory = tempfile::tempdir().unwrap();
    let admissions = Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured = admissions.clone();
    let server = start(
        super::mutations::fixture(directory.path()),
        "cwp://example.test/runtime".into(),
        directory.path(),
        crate::idle::Activity::default(),
        Arc::new(move |admission| {
            captured.lock().unwrap().push(admission.clone());
            Ok(())
        }),
        tokio::sync::watch::channel(()).1,
    )
    .unwrap();
    let client = client(&server.endpoint);
    let ingress = "cwp://org.coworkerprotocol.local/integrations/tui";
    let opened: Value = client
        .post(format!("{}/session", server.endpoint.url()))
        .bearer_auth(&server.endpoint.token)
        .json(&json!({"ingressIntegrationRef":ingress}))
        .send()
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .unwrap();
    let mut tui = server.endpoint.clone();
    tui.token = opened["token"].as_str().unwrap().into();
    let first = message(&tui, "same-key", "Hello");
    assert!(first.get("errors").is_none(), "{first}");
    assert_eq!(first, message(&tui, "same-key", "Hello"));
    let generic = session_endpoint(&client, &server.endpoint, None);
    let other = message(&generic, "same-key", "Hello");
    assert_ne!(
        first["data"]["createConversation"]["messageRef"],
        other["data"]["createConversation"]["messageRef"]
    );
    let action = query_session(
        &client,
        &tui,
        r#"mutation($url:ID!){invokeAction(url:$url,activation:"@worker--run--task",input:{value:"one"},requestId:"action-key"){__typename}}"#,
        json!({"url":server.endpoint.runtime_ref}),
    );
    assert!(action.get("errors").is_none(), "{action}");
    assert_eq!(
        action["data"]["invokeAction"]["__typename"],
        "AdmittedInvocation"
    );
    let records = admissions.lock().unwrap();
    assert_eq!(records[0].record.integration_ref, ingress);
    assert_eq!(records[3].record.integration_ref, ingress);
    assert_eq!(
        records[2].record.integration_ref,
        "cwp://org.coworkerprotocol.local/integrations/graphql"
    );
    assert_eq!(
        client
            .post(format!("{}/session", server.endpoint.url()))
            .bearer_auth(&tui.token)
            .json(&json!({"ingressIntegrationRef":"cwp://example.test/other"}))
            .send()
            .unwrap()
            .status(),
        401
    );
    for invalid in ["~@helper", "cwp://example.test/path#fragment", ""] {
        assert_eq!(
            client
                .post(format!("{}/session", server.endpoint.url()))
                .bearer_auth(&server.endpoint.token)
                .json(&json!({"ingressIntegrationRef":invalid}))
                .send()
                .unwrap()
                .status(),
            400
        );
    }
}

#[test]
fn bootstrap_can_bind_browser_admission_to_a_live_native_provider_without_owning_it() {
    let directory = tempfile::tempdir().unwrap();
    let server = serve(directory.path(), runtime(directory.path()));
    let client = client(&server.endpoint);
    let native = session_endpoint(&client, &server.endpoint, None);
    let (mut socket, provider, actor) = terminal(&native, "native");
    let opened: Value = client
        .post(format!("{}/session", server.endpoint.url()))
        .bearer_auth(&server.endpoint.token)
        .json(&json!({"actorRef":native.actor_ref,"upstreamProviderSessionRef":provider}))
        .send()
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .unwrap();
    let mut browser = server.endpoint.clone();
    browser.token = opened["token"].as_str().unwrap().into();
    let catalog = query_session(
        &client,
        &browser,
        "{runtime{actions{activation} messageTargets{id}}}",
        json!({}),
    );
    assert!(catalog["data"]["runtime"]["actions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|a| a["activation"] == "@tui--run--task"));
    assert!(catalog["data"]["runtime"]["messageTargets"]
        .as_array()
        .unwrap()
        .iter()
        .all(|target| target["id"] != actor));
    // Scope sharing does not transfer provider-control ownership.
    assert!(enable(&browser, &provider, &actor).get("errors").is_some());
    let admitted = query_session(
        &client,
        &browser,
        "mutation($url:ID!){invokeAction(url:$url,activation:\"@tui--run--task\",input:{},requestId:\"browser-action\"){__typename}}",
        json!({"url":browser.runtime_ref}),
    );
    assert_eq!(
        admitted["data"]["invokeAction"]["__typename"], "AdmittedInvocation",
        "{admitted}"
    );
    client
        .delete(format!("{}/session", browser.url()))
        .bearer_auth(&browser.token)
        .send()
        .unwrap()
        .error_for_status()
        .unwrap();
    assert!(enable(&native, &provider, &actor).get("errors").is_none());
    assert_eq!(
        client
            .post(format!("{}/session", server.endpoint.url()))
            .bearer_auth(&native.token)
            .json(&json!({"upstreamProviderSessionRef":provider}))
            .send()
            .unwrap()
            .status(),
        401
    );
    socket.close(None).unwrap();
}

#[test]
fn retained_files_follow_message_participation_and_exact_retry_input() {
    use base64::{engine::general_purpose::STANDARD, Engine};
    let directory = tempfile::tempdir().unwrap();
    let server = serve(directory.path(), runtime(directory.path()));
    let client = client(&server.endpoint);
    let person = session_endpoint(&client, &server.endpoint, None);
    let helper = session_endpoint(
        &client,
        &server.endpoint,
        Some(&crate::test_refs::actor_ref("helper")),
    );
    let bytes = vec![42_u8; 5 * 1024 * 1024];
    let url = format!("{}/files", person.url().trim_end_matches("/graphql"));
    // Keep the missing-auth probe small so early rejection cannot race a bulk
    // request write. The authenticated upload below still checks the full body.
    assert_eq!(
        client
            .post(&url)
            .query(&[("filename", "photo.png"), ("mediaType", "image/png")])
            .body(vec![42_u8])
            .send()
            .unwrap()
            .status(),
        401
    );
    let uploaded: Value = client
        .post(&url)
        .bearer_auth(&person.token)
        .query(&[("filename", "photo.png"), ("mediaType", "image/png")])
        .body(bytes.clone())
        .send()
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .unwrap();
    assert_eq!(uploaded["sizeBytes"], bytes.len());
    let upload = json!({"data":{"ingestFile":uploaded}});
    let reference = upload["data"]["ingestFile"]["id"].as_str().unwrap();
    let read = "query($ref:ID!){runtime{file(ref:$ref){file{id filename} contentBase64}}}";
    let denied = query_session(&client, &helper, read, json!({"ref":reference}));
    assert!(denied.get("errors").is_some());
    let send = "mutation($refs:[ID!]){createConversation(text:\"@helper see the file\",requestId:\"file-send\",fileRefs:$refs){conversationRef messageRef}}";
    let admitted = query_session(&client, &person, send, json!({"refs":[reference]}));
    assert!(admitted.get("errors").is_none(), "{admitted}");
    let replay = query_session(&client, &person, send, json!({"refs":[reference]}));
    assert_eq!(admitted["data"], replay["data"]);
    let changed = query_session(&client, &person, send, json!({"refs":[]}));
    assert!(changed.get("errors").is_some());
    let allowed = query_session(&client, &helper, read, json!({"ref":reference}));
    assert!(allowed.get("errors").is_none(), "{allowed}");
    assert_eq!(
        allowed["data"]["runtime"]["file"]["contentBase64"],
        STANDARD.encode(bytes)
    );
}

#[test]
fn binary_upload_rejects_invalid_metadata_and_oversized_bodies() {
    let directory = tempfile::tempdir().unwrap();
    let server = serve(directory.path(), runtime(directory.path()));
    let client = client(&server.endpoint);
    let person = session_endpoint(&client, &server.endpoint, None);
    let url = format!("{}/files", person.url().trim_end_matches("/graphql"));
    for (filename, media_type, bytes, status) in [
        ("../escape", "image/png", vec![1], 400),
        ("image.png", "invalid", vec![1], 400),
        ("image.png", "image/png", vec![1; 10 * 1024 * 1024 + 1], 413),
    ] {
        assert_eq!(
            client
                .post(&url)
                .bearer_auth(&person.token)
                .query(&[("filename", filename), ("mediaType", media_type)])
                .body(bytes)
                .send()
                .unwrap()
                .status()
                .as_u16(),
            status
        );
    }
}
