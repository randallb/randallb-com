fn ingress(
    event: &str,
    author_actor_ref: &str,
    conversation_ref: Option<&str>,
    addressing: ConversationIngressAddressing,
    text: &str,
) -> ConversationIngress {
    ConversationIngress {
        owner_context_ref: None,
        file_refs: Vec::new(),
        integration_ref: "cwp://com.example.fixture/integrations/local".into(),
        provider_event_ref: event.into(),
        author_actor_ref: author_actor_ref.into(),
        conversation: conversation_ref.map_or(
            ConversationIngressConversation::Create,
            |conversation_ref| ConversationIngressConversation::Continue {
                conversation_ref: conversation_ref.into(),
            },
        ),
        addressing,
        text: text.into(),
    }
}

fn fixture_graph_with_reviewer() -> ResolvedCwrGraph {
    let mut graph = crate::v2026_09::graph::resolve_graph(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures/contracts/agent-recovery-turn/workspace/CWR.toml"),
    )
    .expect("resolve conversation fixture");
    let mut actor = graph
        .actors
        .iter()
        .find(|actor| actor.actor_ref == crate::v2026_09::test_fixture::actor_ref("helper"))
        .expect("helper actor")
        .clone();
    actor.actor_ref = "cwp://com.example.fixture/actors/reviewer".into();
    actor.actor_name = Some("Reviewer".into());
    actor.coworker_ref = "cwp://com.example.fixture/coworkers/reviewer".into();
    actor.coworker_name = Some("Reviewer".into());
    actor.coworker_slug = "reviewer".into();
    graph.actors.push(actor);
    let mut actor_resource = graph
        .resources
        .iter()
        .find(|resource| resource.reference == crate::v2026_09::test_fixture::actor_ref("helper"))
        .expect("helper actor resource")
        .clone();
    actor_resource.reference = "cwp://com.example.fixture/actors/reviewer".into();
    actor_resource.document["ref"] = json!("cwp://com.example.fixture/actors/reviewer");
    actor_resource.document["coworker_ref"] = json!("cwp://com.example.fixture/coworkers/reviewer");
    graph.resources.push(actor_resource);

    let mut integration = graph
        .actor_integrations
        .iter()
        .find(|integration| {
            integration.actor_ref == crate::v2026_09::test_fixture::actor_ref("helper")
        })
        .expect("helper actor integration")
        .clone();
    integration.actor_ref = "cwp://com.example.fixture/actors/reviewer".into();
    integration.actor_name = Some("Reviewer".into());
    integration.coworker_ref = "cwp://com.example.fixture/coworkers/reviewer".into();
    integration.actor_integration_ref =
        "cwp://com.example.fixture/actor_integrations/reviewer_local_execution".into();
    graph.actor_integrations.push(integration);
    graph
}

