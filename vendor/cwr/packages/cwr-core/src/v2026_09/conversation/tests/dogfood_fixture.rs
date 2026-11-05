//! Explicit fixture generation for the task-owned Desktop dogfood workspace.
//! Run only with CWR_CONVERSATIONS_DOGFOOD_ROOT set to a fresh directory.
use super::*;

fn replace_fixture_actors(value: &mut Value, replacements: &[(&str, &str)]) {
    match value {
        Value::String(text) => {
            if let Some((_, replacement)) = replacements
                .iter()
                .find(|(source, _)| text.as_str() == *source)
            {
                *text = (*replacement).to_owned();
            }
        }
        Value::Array(values) => {
            for value in values {
                replace_fixture_actors(value, replacements);
            }
        }
        Value::Object(values) => {
            let old = std::mem::take(values);
            for (mut key, mut value) in old {
                if let Some((_, replacement)) = replacements
                    .iter()
                    .find(|(source, _)| key.as_str() == *source)
                {
                    key = (*replacement).to_owned();
                }
                replace_fixture_actors(&mut value, replacements);
                values.insert(key, value);
            }
        }
        _ => {}
    }
}

fn copy_dogfood_module(source: &std::path::Path, destination: &std::path::Path) {
    std::fs::create_dir(destination).unwrap();
    for entry in std::fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let target = destination.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dogfood_module(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn dogfood_workspace(directory: &std::path::Path) -> crate::v2026_09::graph::ResolvedCwrGraph {
    std::fs::create_dir(directory).expect("fresh task-owned dogfood directory");
    for (filename, name, slug) in [
        ("person.toml", "Person", "person"),
        ("helper.toml", "Fixture Helper", "fixture_helper"),
    ] {
        std::fs::write(directory.join(filename), format!("type = \"cwp://org.coworkerprotocol/2026-09/schemas/coworker\"\nname = {name:?}\nslug = {slug:?}\n")).unwrap();
    }
    let module = PathBuf::from(
        std::env::var("CWR_CONVERSATIONS_DOGFOOD_MODULE")
            .expect("explicit candidate conversations CWR.toml"),
    );
    // Workspace imports must stay inside their source roots. Copy the exact
    // candidate declarations rather than importing another checkout by path.
    copy_dogfood_module(module.parent().unwrap(), &directory.join("module"));
    std::fs::write(directory.join("CWR.toml"), "type = \"cwp://org.coworkerprotocol/2026-09/schemas\"\n[[imports]]\npath = \"./module/CWR.toml\"\nas = \"conversations\"\n[[imports]]\npath = \"./person.toml\"\nas = \"person\"\n[[imports]]\npath = \"./helper.toml\"\nas = \"fixture_helper\"\n").unwrap();
    crate::v2026_09::graph::resolve_graph(directory.join("CWR.toml")).unwrap()
}

fn ambiguous_dogfood_history(store: &mut ConversationAdmissionStore) -> String {
    let first = settled_legacy_turn(store, None, "FICTIONAL LEGACY 01: paper moon inventory");
    let reference = first.record.conversation_ref.clone();
    for index in 2..=36 {
        settled_legacy_turn(
            store,
            Some(&reference),
            &format!("FICTIONAL LEGACY {index:02}: paper moon inventory"),
        );
    }
    let invocation = first.record.invocation_ref.as_deref().unwrap();
    let alternate = store
        .conversation(&reference)
        .unwrap()
        .messages
        .iter()
        .find(|message| message.message_ref != first.record.source_message_ref)
        .unwrap()
        .message_ref
        .clone();
    let execution = store
        .execution_conversation_ref(invocation)
        .unwrap()
        .to_owned();
    let mut state = store.state.clone();
    let conversation = state
        .conversations
        .iter_mut()
        .find(|conversation| conversation.conversation_ref == execution)
        .unwrap();
    conversation.activity.push(ConversationActivity {
        activity_ref: format!("{execution}/ambiguous-dogfood-source"),
        sequence: conversation.activity.len() as u64 + 1,
        occurred_at_unix_ms: 1,
        event: ConversationActivityEvent::TurnStarted {
            invocation_ref: invocation.to_owned(),
            source_message_ref: alternate,
            invocation: None,
        },
    });
    store.replace_state(state).unwrap();
    assert!(matches!(
        store
            .migrate_legacy_conversation_timeline(&reference)
            .unwrap(),
        ConversationTimelineMigrationOutcome::Skipped {
            reason: ConversationTimelineMigrationSkipReason::AmbiguousCausalSource { .. }
        }
    ));
    reference
}

#[test]
#[ignore = "writes a fresh task-owned fictional native Desktop fixture"]
fn generate_conversations_slices_native_dogfood_fixture() {
    let directory = PathBuf::from(
        std::env::var("CWR_CONVERSATIONS_DOGFOOD_ROOT")
            .expect("explicit fresh dogfood output directory"),
    );
    let graph = dogfood_workspace(&directory);
    let actor = |name| {
        graph
            .actors
            .iter()
            .find(|actor| actor.actor_name.as_deref() == Some(name))
            .unwrap()
            .actor_ref
            .clone()
    };
    let person = actor("Person");
    let helper = actor("Fixture Helper");
    let base_directory = |name| {
        let path = PathBuf::from(std::env::var(name).expect("explicit task-owned XDG base"));
        assert!(path.is_absolute(), "task-owned XDG base must be absolute");
        path
    };
    let state_home = base_directory("CWR_CONVERSATIONS_DOGFOOD_STATE_HOME");
    let runtime_home = base_directory("CWR_CONVERSATIONS_DOGFOOD_RUNTIME_HOME");
    assert_eq!(
        PathBuf::from(std::env::var("XDG_STATE_HOME").unwrap()),
        state_home
    );
    assert_eq!(
        PathBuf::from(std::env::var("XDG_RUNTIME_DIR").unwrap()),
        runtime_home
    );
    // Desktop mounts account routes into this supported composition. Its
    // runtime identity depends only on this path and the selected runtime name.
    let composition_path = directory.join("WORKSPACE.toml");
    std::fs::write(
        &composition_path,
        r#"type = "cwp://org.coworkerprotocol/2026-09/schemas/workspace"
[[volumes]]
name = "workspace"
path = "."
[[runtimes]]
name = "workloop"
path = "./CWR.toml"
source_roots = ["."]
working_directory = "."
volume = "workspace"
acting_coworker = "person"
"#,
    )
    .unwrap();
    let composition =
        crate::v2026_09::local::LocalWorkspaceComposition::load(&composition_path).unwrap();
    let selected = composition.configured_runtime("workloop").unwrap();
    assert_eq!(
        selected
            .host_identity("cwp://example.test/fixture-ingress")
            .unwrap()
            .acting_actor_ref,
        person
    );
    let path = selected.database_path();
    assert!(!path.exists(), "fresh task-owned database");
    let mut store = ConversationAdmissionStore::open(&path, &selected.runtime_ref).unwrap();
    let legacy = ambiguous_dogfood_history(&mut store);
    let first = settled_legacy_turn(
        &mut store,
        None,
        "FICTIONAL CANONICAL 01: glass orchard inventory",
    );
    let canonical = first.record.conversation_ref;
    for index in 2..=36 {
        settled_legacy_turn(
            &mut store,
            Some(&canonical),
            &format!("FICTIONAL CANONICAL {index:02}: glass orchard inventory"),
        );
    }
    store
        .migrate_legacy_conversation_timeline(&canonical)
        .unwrap();
    let mut state = store.state.clone();
    for conversation in &mut state.conversations {
        if conversation.conversation_ref == canonical {
            conversation.title = Some("Fictional canonical: Glass orchard".to_owned());
        }
        if conversation.conversation_ref == legacy {
            conversation.title = Some("Fictional legacy: Paper moon".to_owned());
        }
    }
    let mut state = serde_json::to_value(state).unwrap();
    replace_fixture_actors(
        &mut state,
        &[
            (crate::v2026_09::test_fixture::actor_ref("person"), &person),
            (crate::v2026_09::test_fixture::actor_ref("helper"), &helper),
        ],
    );
    store
        .replace_state(serde_json::from_value(state).unwrap())
        .unwrap();
    store
        .configure_timeline_rollout(ConversationTimelineRollout::MigrateLegacy)
        .unwrap();
    assert!(store.retained_timeline(&canonical).is_some());
    assert!(store.retained_timeline(&legacy).is_none());
    drop(store);
    let reopened = ConversationAdmissionStore::open_managed(&path, &selected.runtime_ref).unwrap();
    assert!(reopened.retained_timeline(&canonical).is_some());
    assert!(reopened.retained_timeline(&legacy).is_none());
    assert!(reopened
        .conversation(&canonical)
        .unwrap()
        .participant_actor_refs
        .contains(&person));
    // The configured runtime owns the seed; legacy configured-runtime files
    // are intentionally rejected by customer workspace startup.
    assert!(!directory.join(".cwr/runtime.toml").exists());
    std::fs::write(
        directory.join("fixture.json"),
        serde_json::to_vec_pretty(&json!({
            "workspace": directory.join("CWR.toml"), "workspace_composition": composition_path,
            "configured_runtime": selected.name,
            "xdg_state_home": state_home, "xdg_runtime_dir": runtime_home,
            "state": path, "runtime_directory": selected.runtime_directory().unwrap(),
            "runtime_root_ref": selected.runtime_ref, "person_actor_ref": person,
            "canonical_conversation_ref": canonical, "legacy_conversation_ref": legacy,
            "legacy_skip_reason": "AmbiguousCausalSource"
        }))
        .unwrap(),
    )
    .unwrap();
    println!(
        "Fictional Desktop fixture: {}",
        directory.join("fixture.json").display()
    );
}
