//! Production consumers enter conversation operations through the shared module.
//! Persistence, execution/recovery and host transport projections remain owned
//! by cwr-core; this check prevents adding another external domain caller.
use std::path::Path;

#[test]
fn production_apps_do_not_bypass_the_conversation_service() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let forbidden = [
        "host_conversation",
        "host_conversation_metadata",
        "host_conversations",
        "host_conversation_bootstrap",
        "conversation_turn_statuses",
        "root_conversation_summaries",
        "conversation_records",
        "record_host_exchange",
        "submit_text_as",
        "submit_text_in_upstream_scope",
        "submit_message_in_context",
        "submit_message_in_upstream_scope",
        "replay_text_in_upstream_scope",
        "admit_ingress",
        "admit_ingress_in_scope",
        "participant_conversation_timeline",
        "participant_conversation_timeline_cancellable",
        "participant_conversation_browse",
        "participant_root_conversation_summaries",
        "participant_root_conversation_summaries_cancellable",
        "publication_snapshot_until",
        "rename_conversation_as",
        "fork_conversation_as",
        "fork_conversation_at_as",
        "import_conversation_as",
        "export_conversation_as",
        "reserve_conversation_archive",
        "stop_selected_request_as",
        "stop_conversation_request_as",
        "set_conversation_archived_as",
        "set_conversation_archived_with_stops_as",
        "set_conversation_archived_reserved_as",
    ];
    let mut violations = Vec::new();
    for tree in ["apps", "packages"] {
        for entry in std::fs::read_dir(root.join(tree)).unwrap() {
            let path = entry.unwrap().path();
            if path.file_name().is_some_and(|name| name == "cwr-core") {
                continue; // Owning implementation and its native service internals.
            }
            visit(&path.join("src"), &root, &forbidden, &mut violations);
        }
    }
    assert!(
        violations.is_empty(),
        "external conversation service bypasses:\n{}",
        violations.join("\n")
    );
}

fn visit(directory: &Path, root: &Path, forbidden: &[&str], violations: &mut Vec<String>) {
    if !directory.is_dir() {
        return;
    }
    for entry in std::fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy();
        if name == "tests" || name == "tests.rs" || name.ends_with("_tests.rs") {
            continue;
        }
        if path.is_dir() {
            visit(&path, root, forbidden, violations);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            let source = std::fs::read_to_string(&path).unwrap();
            // Inline test modules are conventionally placed after production code.
            let production = source.split("#[cfg(test)]\nmod ").next().unwrap();
            let compact: String = production.chars().filter(|c| !c.is_whitespace()).collect();
            for method in forbidden {
                if compact.contains(&format!(".{method}(")) {
                    violations.push(format!(
                        "{}: {method}",
                        path.strip_prefix(root).unwrap().display()
                    ));
                }
            }
        }
    }
}
