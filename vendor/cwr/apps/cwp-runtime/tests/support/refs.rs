//! Test refs come from the actual fixture location, including copied workspaces.
use std::{cell::RefCell, collections::BTreeMap, path::Path};

thread_local! {
    static ACTORS: RefCell<BTreeMap<String, String>> = const { RefCell::new(BTreeMap::new()) };
}

pub fn select_workspace(manifest: &Path) {
    let graph =
        cwr::v2026_09::graph::resolve_graph(manifest).expect("resolve test coworker identities");
    ACTORS.with(|actors| {
        actors.borrow_mut().extend(
            graph
                .actors
                .into_iter()
                .map(|actor| (actor.coworker_slug, actor.actor_ref)),
        )
    });
}

pub fn actor_ref(slug: &str) -> String {
    if ACTORS.with(|actors| actors.borrow().is_empty()) {
        select_workspace(&Path::new(env!("CARGO_MANIFEST_DIR")).join(
            "../../packages/cwr-core/fixtures/contracts/agent-recovery-turn/workspace/CWR.toml",
        ));
    }
    ACTORS.with(|actors| {
        actors
            .borrow()
            .get(slug)
            .cloned()
            .unwrap_or_else(|| panic!("missing test coworker {slug}"))
    })
}
