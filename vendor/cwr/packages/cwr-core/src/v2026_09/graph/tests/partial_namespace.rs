use super::*;

#[test]
fn partial_loading_rejects_duplicate_canonical_refs_in_either_order() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path();
    for name in ["one", "two"] {
        write(dir, &format!("{name}.toml"), &format!("type = {ADAPTER_TYPE:?}\nref = 'cwp://example.test/shared-adapter'\nname = '{name}'\n"));
    }
    for (first, second) in [("one", "two"), ("two", "one")] {
        root(dir, &format!("[[imports]]\npath = './{first}.toml'\nas = '{first}'\n[[imports]]\npath = './{second}.toml'\nas = '{second}'\n"));
        let error = resolve_graph_with_options(
            dir.join("CWR.toml"),
            &EmptyResourceResolver,
            &GraphLoadOptions {
                allow_partial: true,
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(
            error.to_string().contains("duplicate canonical ref"),
            "{error}"
        );
    }
}

#[test]
fn unavailable_coworkers_do_not_reserve_a_healthy_coworkers_handle() {
    for failure in ["missing_role", "invalid_role", "invalid_binding"] {
        for (first, second) in [("good", "bad"), ("bad", "good")] {
            let temp = tempfile::tempdir().unwrap();
            let dir = temp.path();
            root(dir, &format!("[[imports]]\npath = './{first}/COWORKER.toml'\nas = '{first}'\n[[imports]]\npath = './{second}/COWORKER.toml'\nas = '{second}'\n"));
            helper(dir, "good");
            helper(dir, "bad");
            let path = dir.join("bad/COWORKER.toml");
            let text = fs::read_to_string(&path)
                .unwrap()
                .replace("name = 'bad'", "name = 'good'")
                .replace("name = \"bad\"", "name = \"good\"")
                .replace("slug = \"bad\"", "slug = \"good\"");
            fs::write(path, text).unwrap();
            match failure {
                "missing_role" => fs::remove_file(dir.join("bad/ROLE.toml")).unwrap(),
                "invalid_role" => write(dir, "bad/schema.json", "invalid JSON"),
                "invalid_binding" => fs::remove_file(dir.join("bad.toml")).unwrap(),
                _ => unreachable!(),
            }
            let graph = partial(dir);
            assert_eq!(names(&graph), ["good"], "{failure}");
            assert_eq!(graph.routes.len(), 1);
            assert!(graph.routes[0]
                .coworker_source
                .ends_with("good/COWORKER.toml"));
        }
    }
}

#[test]
fn duplicate_handles_disable_both_declarations_in_either_import_order() {
    for order in [["one", "two"], ["two", "one"]] {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path();
        root(
            dir,
            &format!(
                "[[imports]]\npath = './{}/COWORKER.toml'\n[[imports]]\npath = './{}/COWORKER.toml'\n[[imports]]\npath = './good/COWORKER.toml'\n",
                order[0], order[1]
            ),
        );
        for name in order {
            write(
                dir,
                &format!("{name}/COWORKER.toml"),
                &format!("type = {COWORKER_TYPE:?}\nname = {name:?}\nslug = 'duplicate'\n"),
            );
        }
        helper(dir, "good");
        assert_eq!(names(&partial(dir)), ["good"]);
    }
}

#[test]
fn partial_loading_does_not_choose_between_ambiguous_namespace_aliases() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path();
    root(dir, "[[imports]]\npath = './parent/COWORKER.toml'\n");
    person(
        dir,
        "parent",
        "[[imports]]\npath = '../one/COWORKER.toml'\nas = 'same'\n[[imports]]\npath = '../two/COWORKER.toml'\nas = 'same'\n",
    );
    person(dir, "one", "");
    person(dir, "two", "");
    let result = resolve_graph_with_options(
        dir.join("CWR.toml"),
        &EmptyResourceResolver,
        &GraphLoadOptions {
            allow_partial: true,
            ..Default::default()
        },
    );
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("resolves to two declarations"));
}
