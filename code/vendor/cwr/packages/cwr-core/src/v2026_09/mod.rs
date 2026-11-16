//! Resource-graph and conversation runtime for the 2026-09 protocol draft.

pub mod conversation;
pub mod execution;
pub mod graph;
pub mod host;
pub mod invocation;
pub mod local;
pub mod service;
pub mod startup_profile;
pub mod upstream;
pub mod workspace;

#[cfg(test)]
pub(crate) mod test_fixture {
    use super::graph::{resolve_graph, ResolvedCwrActor, ResolvedCwrGraph};
    use std::{path::PathBuf, sync::OnceLock};

    pub(crate) fn graph() -> &'static ResolvedCwrGraph {
        static GRAPH: OnceLock<ResolvedCwrGraph> = OnceLock::new();
        GRAPH.get_or_init(|| {
            resolve_graph(
                PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("fixtures/contracts/agent-recovery-turn/workspace/CWR.toml"),
            )
            .expect("resolve test coworker graph")
        })
    }
    pub(crate) fn actor(slug: &str) -> &'static ResolvedCwrActor {
        graph()
            .actors
            .iter()
            .find(|actor| actor.coworker_slug == slug)
            .expect("test coworker slug")
    }
    pub(crate) fn actor_ref(slug: &str) -> &'static str {
        &actor(slug).actor_ref
    }
    pub(crate) fn role_ref(slug: &str) -> &'static str {
        &actor(slug).role_refs[0]
    }
    pub(crate) fn binding_ref(slug: &str) -> &'static str {
        actor(slug)
            .default_actor_integration_ref
            .as_deref()
            .expect("test coworker default binding")
    }
}
