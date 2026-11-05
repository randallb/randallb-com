use super::*;
use std::sync::atomic::{AtomicBool, Ordering};

const MUTATION: &str = "mutation($url:ID!,$input:JSON!,$request:ID!){invokeAction(url:$url,activation:\"@worker--run--task\",input:$input,requestId:$request){__typename ...on ActionInvocation{id state conversationRef sourceMessageRef requester{id} target{id}} ...on ActionError{code retryable}}}";

pub(super) fn fixture(directory: &Path) -> LocalRuntime {
    fixture_with_options(directory, Default::default())
}
pub(super) fn fixture_with_options(
    directory: &Path,
    options: cwr::v2026_09::local::LocalRuntimeOptions,
) -> LocalRuntime {
    let workspace = directory.join("workspace");
    fs::create_dir_all(&workspace).unwrap();
    for (name, contents) in [
        (
            "CWR.toml",
            "type='cwp://org.coworkerprotocol/2026-09/schemas'\n[[imports]]\npath='./person.toml'\nas='person'\n[[imports]]\npath='./worker.toml'\nas='worker'\n",
        ),
        (
            "person.toml",
            "type='cwp://org.coworkerprotocol/2026-09/schemas/coworker'\nname='Person'\nslug='person'\n",
        ),
        (
            "worker.toml",
            "type='cwp://org.coworkerprotocol/2026-09/schemas/coworker'\nname='Worker'\nslug='worker'\n[[roles]]\npath='./ROLE.toml'\n[[integrations]]\npath='./integration.toml'\nas='local'\n[integrations.params.commands]\nrun--task='printf task'\n",
        ),
        (
            "ROLE.toml",
            "type='cwp://org.coworkerprotocol/2026-09/schemas/role'\n[[adapters]]\nref='cwp://org.coworkerprotocol/2026-09/adapters/sh'\n[[actions]]\nverb='run'\nobject='task'\ntitle='Run task'\ndescription='Run a test task'\n[actions.input_schema]\npath='./input.json'\n[actions.output_schema]\npath='./output.json'\n",
        ),
        (
            "integration.toml",
            "type='cwp://org.coworkerprotocol/2026-09/schemas/integration'\n[adapter]\nref='cwp://org.coworkerprotocol/2026-09/adapters/sh'\n",
        ),
        (
            "input.json",
            "{\"type\":\"object\",\"properties\":{\"value\":{\"type\":\"string\"}},\"required\":[\"value\"],\"additionalProperties\":false}",
        ),
        ("output.json", "{\"type\":\"object\"}"),
    ] {
        fs::write(workspace.join(name), contents).unwrap();
    }
    let manifest = workspace.join("CWR.toml");
    let actor = cwr::v2026_09::local::load_default_local_acting_actor_ref(&manifest).unwrap();
    LocalRuntime::open_with_state_path_and_runtime_ref_and_options(
        &manifest,
        directory.join("cwpdb.sqlite3"),
        "cwp://example.test/runtime",
        LocalHostIdentity::new(actor, "cwp://example.test/integrations/graphql"),
        options,
    )
    .unwrap()
}
fn serve(directory: &Path, runtime: LocalRuntime, publisher: AdmissionPublisher) -> GraphqlServer {
    start(
        runtime,
        "cwp://example.test/runtime".into(),
        directory,
        crate::idle::Activity::default(),
        publisher,
        tokio::sync::watch::channel(()).1,
    )
    .unwrap()
}
fn invoke(endpoint: &LocalGraphqlEndpoint, request: &str, value: Value) -> Value {
    query(
        &client(endpoint),
        endpoint,
        MUTATION,
        json!({"url":endpoint.runtime_ref,"request":request,"input":value}),
    )
}

#[test]
fn mutation_retries_survive_restart_and_reject_changed_input() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = fixture(directory.path());
    let server = serve(directory.path(), runtime.clone(), Arc::new(|_| Ok(())));
    let first = invoke(&server.endpoint, "one", json!({"value":"original"}));
    assert!(first.get("errors").is_none(), "{first}");
    assert_eq!(
        first["data"]["invokeAction"]["__typename"],
        "AdmittedInvocation"
    );
    assert_eq!(first["data"]["invokeAction"]["state"], "admitted");
    assert_eq!(
        first,
        invoke(&server.endpoint, "one", json!({"value":"original"}))
    );
    let changed = invoke(&server.endpoint, "one", json!({"value":"changed"}));
    assert!(changed.get("errors").is_some(), "{changed}");
    let invalid = invoke(&server.endpoint, "invalid", json!({"value":42}));
    assert_eq!(
        invalid["data"]["invokeAction"]["__typename"], "ActionError",
        "{invalid}"
    );
    assert!(
        first["data"]["invokeAction"]["requester"]["id"].is_string(),
        "{first}"
    );
    assert!(
        first["data"]["invokeAction"]["target"]["id"].is_string(),
        "{first}"
    );
    let conversation = first["data"]["invokeAction"]["conversationRef"]
        .as_str()
        .unwrap();
    assert_eq!(
        runtime
            .conversation(conversation)
            .unwrap()
            .unwrap()
            .messages
            .len(),
        1
    );
    drop(server);
    drop(runtime);
    let server = serve(
        directory.path(),
        fixture(directory.path()),
        Arc::new(|_| Ok(())),
    );
    assert_eq!(
        first,
        invoke(&server.endpoint, "one", json!({"value":"original"}))
    );
}

#[test]
fn concurrent_retries_admit_one_invocation() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = fixture(directory.path());
    let server = serve(directory.path(), runtime.clone(), Arc::new(|_| Ok(())));
    let barrier = Arc::new(std::sync::Barrier::new(3));
    let tasks = (0..2)
        .map(|_| {
            let endpoint = server.endpoint.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                invoke(&endpoint, "concurrent", json!({"value":"same"}))
            })
        })
        .collect::<Vec<_>>();
    barrier.wait();
    let results = tasks
        .into_iter()
        .map(|task| task.join().unwrap())
        .collect::<Vec<_>>();
    assert!(results[0].get("errors").is_none(), "{:?}", results);
    assert_eq!(results[0], results[1]);
    assert_eq!(
        runtime
            .conversations()
            .unwrap()
            .iter()
            .filter(|c| c.parent_conversation_ref.is_none())
            .count(),
        1
    );
}

#[test]
fn ordinary_owner_url_admits_private_named_actions_without_user_conversations() {
    use cwr::v2026_09::upstream::ConversationContextPolicy;
    let directory = tempfile::tempdir().unwrap();
    let runtime = fixture(directory.path());
    let catalog = runtime.catalog().unwrap();
    let person = catalog.acting_actor.actor_ref;
    let worker = catalog
        .known_actors
        .iter()
        .find(|actor| actor.handle == "@worker")
        .unwrap()
        .actor_ref
        .clone();
    let pack = "urn:uuid:installed-pack-owner";
    let other = "urn:uuid:other-pack-owner";
    let policy = ConversationContextPolicy {
        action_permissions: Vec::new(),
        suspended: false,
        default_context_ref: "workspace".into(),
        contexts: std::collections::BTreeMap::from([
            ("workspace".into(), Vec::new()),
            (pack.into(), vec![worker.clone()]),
            (other.into(), vec![worker.clone()]),
            ("disabled-owner".into(), Vec::new()),
        ]),
    };
    runtime
        .configure_conversation_contexts(Some(policy.clone()))
        .unwrap();
    let server = serve(directory.path(), runtime.clone(), Arc::new(|_| Ok(())));
    let client = client(&server.endpoint);
    let call = |url: &str, request: &str, value: &str| {
        query(
            &client,
            &server.endpoint,
            MUTATION,
            json!({"url":url,"request":request,"input":{"value":value}}),
        )
    };
    for url in [worker.as_str(), "missing-owner", "disabled-owner"] {
        let denied = call(url, url, "private action input");
        assert_eq!(
            denied["data"]["invokeAction"]["code"], "resource_unavailable",
            "{denied}"
        );
    }
    let denied_default = call(
        &server.endpoint.runtime_ref,
        "default",
        "private action input",
    );
    assert!(denied_default.get("errors").is_some(), "{denied_default}");
    assert!(runtime.conversations().unwrap().is_empty());
    let admitted = call(pack, "private-action", "private action input");
    assert!(admitted.get("errors").is_none(), "{admitted}");
    let invocation = &admitted["data"]["invokeAction"];
    assert_eq!(invocation["__typename"], "AdmittedInvocation", "{admitted}");
    assert_eq!(invocation["requester"]["id"], person);
    assert_eq!(invocation["target"]["id"], worker);
    let conversation = runtime
        .conversation(invocation["conversationRef"].as_str().unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(conversation.owner_context_ref.as_deref(), Some(pack));
    assert!(!conversation.is_sidebar_eligible());
    assert_eq!(
        call(pack, "private-action", "private action input"),
        admitted
    );
    let before_replay_denial = runtime.conversations().unwrap().len();
    let root_replay = call(
        &server.endpoint.runtime_ref,
        "private-action",
        "private action input",
    );
    assert!(root_replay["data"].is_null(), "{root_replay}");
    assert_eq!(
        root_replay["errors"][0]["extensions"]["code"], "ACTION_OUTCOME_UNKNOWN",
        "{root_replay}"
    );
    assert_eq!(runtime.conversations().unwrap().len(), before_replay_denial);
    for changed in [
        call(other, "private-action", "private action input"),
        call(pack, "private-action", "changed input"),
    ] {
        assert!(changed.get("errors").is_some(), "{changed}");
        assert_eq!(runtime.conversations().unwrap().len(), before_replay_denial);
    }
    let lists = query(&client, &server.endpoint,
        "query($owner:ID!){runtime{conversations(ownerContextRef:$owner){totalCount edges{node{id}}} actions(ownerContextRef:$owner){activation}}}",
        json!({"owner":pack}));
    assert!(lists.get("errors").is_none(), "{lists}");
    assert_eq!(lists["data"]["runtime"]["conversations"]["totalCount"], 0);
    assert!(lists["data"]["runtime"]["conversations"]["edges"]
        .as_array()
        .unwrap()
        .is_empty());
    assert!(lists["data"]["runtime"]["actions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|action| action["activation"] == "@worker--run--task"));
    let mut suspended = policy.clone();
    suspended.suspended = true;
    runtime
        .configure_conversation_contexts(Some(suspended))
        .unwrap();
    let before_denial = runtime.conversations().unwrap().len();
    let suspended = call(pack, "suspended", "private action input");
    // Main's read boundary hides suspended owner contexts before admission.
    // Preserve that safe rejection rather than relying on a data-registry error.
    assert!(suspended.get("errors").is_none(), "{suspended}");
    assert_eq!(
        suspended["data"]["invokeAction"]["code"], "resource_unavailable",
        "{suspended}"
    );
    assert!(
        suspended["data"]["invokeAction"]["id"].is_null(),
        "{suspended}"
    );
    assert_eq!(runtime.conversations().unwrap().len(), before_denial);
    let mut disabled = policy;
    disabled.contexts.insert(pack.into(), Vec::new());
    runtime
        .configure_conversation_contexts(Some(disabled))
        .unwrap();
    let disabled = call(pack, "disabled", "private action input");
    assert!(disabled.get("errors").is_none(), "{disabled}");
    assert_eq!(
        disabled["data"]["invokeAction"]["code"], "resource_unavailable",
        "{disabled}"
    );
    assert!(
        disabled["data"]["invokeAction"]["id"].is_null(),
        "{disabled}"
    );
    assert_eq!(runtime.conversations().unwrap().len(), before_denial);
    let retained = runtime.conversations().unwrap();
    assert_eq!(retained.len(), 2, "one action root and its execution child");
    assert_eq!(
        retained
            .iter()
            .filter(|record| record.parent_conversation_ref.is_none())
            .count(),
        1
    );
    assert!(retained
        .iter()
        .any(|record| record.parent_conversation_ref.as_deref()
            == Some(conversation.conversation_ref.as_str())));
    assert!(retained.iter().all(|record| !record.is_sidebar_eligible()));
}

#[test]
fn failure_after_admission_is_ambiguous_and_recovers_the_original() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = fixture(directory.path());
    let failed = Arc::new(AtomicBool::new(false));
    let marker = failed.clone();
    let server = serve(
        directory.path(),
        runtime.clone(),
        Arc::new(move |_| {
            if !marker.swap(true, Ordering::SeqCst) {
                return Err(std::io::Error::other("publication unavailable").into());
            }
            Ok(())
        }),
    );
    let first = invoke(&server.endpoint, "recover", json!({"value":"same"}));
    assert_eq!(
        first["errors"][0]["extensions"]["code"], "ACTION_OUTCOME_UNKNOWN",
        "{first}"
    );
    let recovered = invoke(&server.endpoint, "recover", json!({"value":"same"}));
    assert_eq!(
        recovered["data"]["invokeAction"]["__typename"], "AdmittedInvocation",
        "{recovered}"
    );
    assert_eq!(
        runtime
            .conversations()
            .unwrap()
            .iter()
            .filter(|c| c.parent_conversation_ref.is_none())
            .count(),
        1
    );
}

#[test]
fn discovery_and_target_authority_are_checked_before_admission() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = fixture(directory.path());
    let catalog = runtime.catalog().unwrap();
    let worker = &catalog
        .actors
        .iter()
        .find(|a| a.handle == "@worker")
        .unwrap()
        .actor_ref;
    let hidden = runtime
        .submit_text_as(
            "cwp://example.test/integrations/test",
            "private",
            worker,
            None,
            "private",
        )
        .unwrap();
    let server = serve(directory.path(), runtime.clone(), Arc::new(|_| Ok(())));
    let actions = query(
        &client(&server.endpoint),
        &server.endpoint,
        "{runtime{actions{activation inputSchema}}}",
        json!({}),
    );
    assert_eq!(
        actions["data"]["runtime"]["actions"][0]["activation"], "@worker--run--task",
        "{actions}"
    );
    let denied = query(
        &client(&server.endpoint),
        &server.endpoint,
        MUTATION,
        json!({"url":hidden.conversation_ref(),"request":"denied","input":{"value":"x"}}),
    );
    assert_eq!(
        denied["data"]["invokeAction"]["code"], "resource_unavailable",
        "{denied}"
    );
    let visible = runtime
        .submit_text_as(
            "cwp://example.test/integrations/test",
            "visible",
            &catalog.acting_actor.actor_ref,
            None,
            "visible",
        )
        .unwrap();
    let accepted = query(
        &client(&server.endpoint),
        &server.endpoint,
        MUTATION,
        json!({"url":visible.conversation_ref(),"request":"allowed","input":{"value":"x"}}),
    );
    assert_eq!(
        accepted["data"]["invokeAction"]["conversationRef"],
        visible.conversation_ref(),
        "{accepted}"
    );
    let read = query(
        &client(&server.endpoint),
        &server.endpoint,
        "query($id:ID!){runtime{invocation(id:$id){id state sourceMessageRef}}}",
        json!({"id":accepted["data"]["invokeAction"]["id"]}),
    );
    assert_eq!(
        read["data"]["runtime"]["invocation"]["id"], accepted["data"]["invokeAction"]["id"],
        "{read}"
    );
}

#[test]
fn removed_action_recovers_through_shared_admission_and_graphql_after_restart() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = fixture(directory.path());
    let actor = runtime.catalog().unwrap().acting_actor.actor_ref;
    let server = serve(directory.path(), runtime.clone(), Arc::new(|_| Ok(())));
    let first = invoke(&server.endpoint, "removed", json!({"value":"original"}));
    let id = first["data"]["invokeAction"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let conversation = first["data"]["invokeAction"]["conversationRef"]
        .as_str()
        .unwrap();
    fs::write(directory.path().join("workspace/worker.toml"), "type='cwp://org.coworkerprotocol/2026-09/schemas/coworker'\nname='Worker'\nslug='worker'\n").unwrap();
    runtime.reload_graph().unwrap();
    assert_eq!(
        first,
        invoke(&server.endpoint, "removed", json!({"value":"original"}))
    );
    assert!(
        invoke(&server.endpoint, "removed", json!({"value":"changed"}))
            .get("errors")
            .is_some()
    );
    let key = serde_json::to_string(&(&actor, "removed")).unwrap();
    let recovered = runtime
        .admit_action_as(
            "cwp://org.coworkerprotocol.local/integrations/graphql",
            &key,
            &actor,
            None,
            "@worker--run--task",
            json!({"value":"original"}),
        )
        .unwrap();
    assert_eq!(recovered.invocation_ref(), Some(id.as_str()));
    assert!(runtime
        .admit_action_as(
            "cwp://org.coworkerprotocol.local/integrations/graphql",
            &key,
            &actor,
            Some(conversation),
            "@worker--run--task",
            json!({"value":"original"})
        )
        .is_err());
    drop(server);
    drop(runtime);
    let runtime = LocalRuntime::open_with_state_path_and_runtime_ref(
        directory.path().join("workspace/CWR.toml"),
        directory.path().join("cwpdb.sqlite3"),
        "cwp://example.test/runtime",
        LocalHostIdentity::new(&actor, "cwp://example.test/integrations/graphql"),
    )
    .unwrap();
    let server = serve(directory.path(), runtime, Arc::new(|_| Ok(())));
    assert_eq!(
        first,
        invoke(&server.endpoint, "removed", json!({"value":"original"}))
    );
    let new = invoke(&server.endpoint, "new", json!({"value":"original"}));
    assert_eq!(new["data"]["invokeAction"]["__typename"], "ActionError");
}
