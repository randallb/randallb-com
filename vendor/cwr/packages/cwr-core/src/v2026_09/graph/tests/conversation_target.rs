use super::*;

#[test]
fn conversation_default_resolves_paths_and_refs_to_coworker_identity() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path();
    person(dir, "helper", "");
    root(dir, "[[imports]]\npath = './person/COWORKER.toml'\n");
    let target =
        resolver::canonical_source_ref(&dir.join("helper/COWORKER.toml").canonicalize().unwrap());
    for declaration in [
        "[[imports]]\npath = '../helper/COWORKER.toml'\nas = 'different_alias'\ndefault_conversation_target = true\n".to_owned(),
        format!("[[imports]]\npath = '../helper/COWORKER.toml'\n[[imports]]\nref = {target:?}\ndefault_conversation_target = true\n"),
        "[[imports]]\npath = '../helper/COWORKER.toml'\nas = 'helper'\n[[imports]]\nref = '~@/helper'\ndefault_conversation_target = true\n".to_owned(),
    ] {
        person(dir, "person", &declaration);
        let graph = resolve_graph(dir.join("CWR.toml")).unwrap();
        let person = graph.actors.iter().find(|actor| actor.coworker_slug == "person").unwrap();
        assert_eq!(person.default_conversation_target_ref.as_deref(), Some(target.as_str()));
        let checked = check(dir.join("CWR.toml")).unwrap();
        assert!(checked["actors"].as_array().unwrap().iter().any(|actor| actor["default_conversation_target_ref"] == target));
        let catalog = crate::v2026_09::local::load_local_catalog(dir.join("CWR.toml"), &person.actor_ref).unwrap();
        assert_eq!(catalog.default_conversation_target_ref.as_deref(), Some(target.as_str()));
        // People remain valid defaults without an executable integration.
        let host = crate::v2026_09::host::project_host_catalog(&catalog);
        assert_eq!(host.default_conversation_target_ref.as_deref(), Some(target.as_str()));
        let helper_catalog = crate::v2026_09::local::load_local_catalog(dir.join("CWR.toml"), &target).unwrap();
        assert_eq!(helper_catalog.default_conversation_target_ref, None);
    }
}

#[test]
fn ordinary_imports_do_not_require_a_coworker_or_choose_a_default() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path();
    root(dir, "[[imports]]\npath = './person/COWORKER.toml'\n");
    role(dir, "helper", "gpt-5");
    for setting in ["", "default_conversation_target = false\n"] {
        person(
            dir,
            "person",
            &format!("[[imports]]\npath = '../helper/ROLE.toml'\n{setting}"),
        );
        let graph = resolve_graph(dir.join("CWR.toml")).unwrap();
        assert_eq!(graph.actors[0].default_conversation_target_ref, None);
    }
}

#[test]
fn invalid_conversation_defaults_reject_in_check_and_graph_loading() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path();
    root(dir, "[[imports]]\npath = './person/COWORKER.toml'\n");
    role(dir, "role", "gpt-5");
    person(dir, "helper", "");
    let valid =
        "[[imports]]\npath = '../helper/COWORKER.toml'\ndefault_conversation_target = true\n";
    for (declaration, expected) in [
        ("[[imports]]\npath = '../role/ROLE.toml'\ndefault_conversation_target = true\n".to_owned(), "coworker for default_conversation_target"),
        ("[[imports]]\nref = 'cwp://org.coworkerprotocol/2026-09/adapters/codex'\ndefault_conversation_target = true\n".to_owned(), "coworker for default_conversation_target"),
        (format!("{valid}{valid}"), "at most one import"),
        ("[[imports]]\nref = 'cwp://missing.example/resource'\ndefault_conversation_target = true\n".to_owned(), "unresolved resource ref"),
        ("[[imports]]\npath = './COWORKER.toml'\ndefault_conversation_target = true\n".to_owned(), "must select another coworker"),
        ("[[roles]]\npath = '../role/ROLE.toml'\ndefault_conversation_target = true\n".to_owned(), "default_conversation_target"),
    ] {
        person(dir, "person", &declaration);
        for error in [check(dir.join("CWR.toml")).unwrap_err(), resolve_graph(dir.join("CWR.toml")).unwrap_err()] {
            assert!(error.to_string().contains(expected), "{error}");
        }
    }
}
