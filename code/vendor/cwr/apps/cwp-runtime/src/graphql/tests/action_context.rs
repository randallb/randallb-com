//! Scoped action identity does not require exposing a private coworker globally.
use super::*;

#[test]
fn scoped_actions_preserve_identity_without_global_node_visibility() {
    use cwr::v2026_09::upstream::ConversationContextPolicy;
    let directory = tempfile::tempdir().unwrap();
    let runtime = mutations::fixture(directory.path());
    let helper = runtime
        .catalog()
        .unwrap()
        .known_actors
        .into_iter()
        .find(|actor| actor.handle == "@worker")
        .unwrap()
        .actor_ref;
    runtime
        .configure_conversation_contexts(Some(ConversationContextPolicy {
            action_permissions: Vec::new(),
            suspended: false,
            default_context_ref: "workspace".into(),
            contexts: std::collections::BTreeMap::from([
                ("workspace".into(), vec![]),
                ("pack".into(), vec![helper.clone()]),
            ]),
        }))
        .unwrap();
    let server = serve(directory.path(), runtime);
    let response = query(&client(&server.endpoint), &server.endpoint,
        "query($actor:ID!){runtime{node(ref:$actor){id} actions{activation actorRef actionName} scoped:actions(ownerContextRef:\"pack\"){activation actorRef actionName}}}",
        json!({"actor":helper}));
    assert!(response.get("errors").is_none(), "{response}");
    assert!(response["data"]["runtime"]["node"].is_null(), "{response}");
    let normal = response["data"]["runtime"]["actions"].as_array().unwrap();
    assert!(
        !normal.iter().any(|action| action["actorRef"] == helper),
        "{response}"
    );
    let scoped = response["data"]["runtime"]["scoped"].as_array().unwrap();
    let matching: Vec<_> = scoped
        .iter()
        .filter(|action| action["actorRef"] == helper)
        .collect();
    assert_eq!(matching.len(), 1, "{response}");
    assert_eq!(matching[0]["activation"], "@worker--run--task");
    assert_eq!(matching[0]["actionName"], "run--task");
}
