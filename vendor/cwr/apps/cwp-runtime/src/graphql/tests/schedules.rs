use super::*;
use cwr::v2026_09::{
    local::{route_action_target, ScheduleDeclaration},
    upstream::ConversationContextPolicy,
};

#[test]
fn installed_schedule_collection_reads_and_owner_actions_keep_their_scope() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = mutations::fixture(directory.path());
    let person = runtime.catalog().unwrap().acting_actor.actor_ref;
    let worker = runtime
        .catalog()
        .unwrap()
        .known_actors
        .into_iter()
        .find(|actor| actor.handle == "@worker")
        .unwrap()
        .actor_ref;
    let workspace = "cwp://example.test/contexts/workspace";
    let pack = "cwp://example.test/contexts/pack";
    let other = "cwp://example.test/contexts/other";
    runtime
        .configure_conversation_contexts(Some(ConversationContextPolicy {
            action_permissions: Vec::new(),
            suspended: false,
            default_context_ref: workspace.into(),
            contexts: std::collections::BTreeMap::from([
                (workspace.into(), Vec::new()),
                (pack.into(), vec![worker.clone()]),
                (other.into(), vec![worker.clone()]),
            ]),
        }))
        .unwrap();
    let graph = runtime.graph().unwrap();
    let route = graph
        .routes
        .iter()
        .find(|route| route.actor_ref == worker && route.action_name == "run--task")
        .unwrap();
    let declaration = ScheduleDeclaration {
        declaration_ref: "cwp://example.test/releases/test/schedules/tick".into(),
        revision: "v1".into(),
        target: route_action_target(&graph.runtime_ref, route),
        input: json!({"value":"private saved input"}),
        anchor_unix_ms: 1_800_000_000_000,
        interval_ms: 2000,
    };
    let owned = runtime
        .schedules_module_as(pack, &person)
        .unwrap()
        .install(declaration.clone(), 1_799_999_999_000)
        .unwrap();
    runtime
        .schedules_module_as(other, &person)
        .unwrap()
        .install(declaration, 1_799_999_999_000)
        .unwrap();
    runtime.tick_schedules(1_800_000_000_000).unwrap();
    runtime.tick_schedules(1_800_000_002_000).unwrap();
    runtime.tick_schedules(1_800_000_002_001).unwrap();
    let server = serve(directory.path(), runtime.clone());
    let client = client(&server.endpoint);
    let session = session_endpoint(&client, &server.endpoint, None);
    let selected = query_session(
        &client,
        &session,
        "query($owner:ID!,$ref:ID!){runtime{schedules(ownerContextRef:$owner){id scheduleRef ownerContextRef definition{declarationRef revision anchorUnixMs intervalMs} availability paused nextDueUnixMs} scheduleRuns(ownerContextRef:$owner,scheduleRef:$ref){id occurrenceRef scheduleRef dueUnixMs outcome invocationRef conversationRef archived}}}",
        json!({"owner":pack,"ref":owned.schedule_ref}),
    );
    assert!(selected.get("errors").is_none(), "{selected}");
    let rows = selected["data"]["runtime"]["schedules"].as_array().unwrap();
    assert_eq!(rows.len(), 1, "{selected}");
    assert_eq!(rows[0]["id"], owned.schedule_ref);
    assert_eq!(rows[0]["ownerContextRef"], pack);
    assert!(!selected.to_string().contains("private saved input"));
    let runs = selected["data"]["runtime"]["scheduleRuns"]
        .as_array()
        .unwrap();
    assert_eq!(runs.len(), 1, "{selected}");
    assert!(runs[0]["conversationRef"].is_string(), "{selected}");
    let foreign = query_session(
        &client,
        &session,
        "query($owner:ID!,$ref:ID!){runtime{scheduleRuns(ownerContextRef:$owner,scheduleRef:$ref){id}}}",
        json!({"owner":other,"ref":owned.schedule_ref}),
    );
    assert!(foreign.get("errors").is_some(), "{foreign}");
    let root = query_session(
        &client,
        &session,
        "query($owner:ID!){runtime{schedules(ownerContextRef:$owner){id}}}",
        json!({"owner":workspace}),
    );
    assert!(root.get("errors").is_some(), "{root}");

    let mutation = "mutation($url:ID!,$input:JSON!){invokeAction(url:$url,activation:\"@cwr--pause--schedule\",input:$input,requestId:\"owner-pause\"){__typename ...on ActionInvocation{id conversationRef}}}";
    let variables = json!({"url":pack,"input":{"schedule_ref":owned.schedule_ref}});
    let admitted = query_session(&client, &session, mutation, variables.clone());
    assert!(admitted.get("errors").is_none(), "{admitted}");
    assert_eq!(
        admitted["data"]["invokeAction"]["__typename"], "AdmittedInvocation",
        "{admitted}"
    );
    assert_eq!(
        admitted,
        query_session(&client, &session, mutation, variables),
    );
    let conversation_ref = admitted["data"]["invokeAction"]["conversationRef"]
        .as_str()
        .unwrap();
    assert_eq!(
        runtime
            .conversation(conversation_ref)
            .unwrap()
            .unwrap()
            .owner_context_ref
            .as_deref(),
        Some(pack)
    );
    let retarget = query_session(
        &client,
        &session,
        mutation,
        json!({"url":other,"input":{"schedule_ref":owned.schedule_ref}}),
    );
    assert!(retarget.get("errors").is_some(), "{retarget}");

    let before_replay = runtime.conversations().unwrap().len();
    let runtime_root = query_session(
        &client,
        &session,
        mutation,
        json!({"url":graph.runtime_ref,"input":{"schedule_ref":owned.schedule_ref}}),
    );
    assert_eq!(
        runtime_root["data"]["invokeAction"]["__typename"], "ActionError",
        "{runtime_root}"
    );
    runtime
        .configure_conversation_contexts(Some(ConversationContextPolicy {
            action_permissions: Vec::new(),
            suspended: false,
            default_context_ref: pack.into(),
            contexts: std::collections::BTreeMap::from([
                (workspace.into(), Vec::new()),
                (pack.into(), vec![worker.clone()]),
                (other.into(), vec![worker.clone()]),
            ]),
        }))
        .unwrap();
    let default_changed_via_root = query_session(
        &client,
        &session,
        mutation,
        json!({"url":graph.runtime_ref,"input":{"schedule_ref":owned.schedule_ref}}),
    );
    assert_eq!(
        default_changed_via_root["data"]["invokeAction"]["__typename"], "ActionError",
        "{default_changed_via_root}"
    );
    let default_changed_via_owner = query_session(
        &client,
        &session,
        mutation,
        json!({"url":pack,"input":{"schedule_ref":owned.schedule_ref}}),
    );
    assert!(
        default_changed_via_owner.get("errors").is_some(),
        "{default_changed_via_owner}"
    );
    runtime
        .configure_conversation_contexts(Some(ConversationContextPolicy {
            action_permissions: Vec::new(),
            suspended: false,
            default_context_ref: workspace.into(),
            contexts: std::collections::BTreeMap::from([
                (workspace.into(), Vec::new()),
                (other.into(), vec![worker]),
            ]),
        }))
        .unwrap();
    let removed_owner = query_session(
        &client,
        &session,
        mutation,
        json!({"url":pack,"input":{"schedule_ref":owned.schedule_ref}}),
    );
    assert!(removed_owner.get("errors").is_some(), "{removed_owner}");
    let removed_via_root = query_session(
        &client,
        &session,
        mutation,
        json!({"url":graph.runtime_ref,"input":{"schedule_ref":owned.schedule_ref}}),
    );
    assert_eq!(
        removed_via_root["data"]["invokeAction"]["__typename"], "ActionError",
        "{removed_via_root}"
    );
    assert_eq!(runtime.conversations().unwrap().len(), before_replay);
}
