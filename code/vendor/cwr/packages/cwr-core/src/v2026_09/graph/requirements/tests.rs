use super::*;
use serde_json::json;

fn write(root: &Path, name: &str, text: &str) {
    fs::write(root.join(name), text).unwrap();
}
fn input(name: &str, kind: &str) -> String {
    format!("[[inputs]]\ntype = '{kind}'\nas = '{name}'\n")
}
fn fixture(kind: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "CWR.toml",
        &format!(
            "type = '{RUNTIME_TYPE}'\n{}[[imports]]\npath = './person.toml'\nas = 'person'\n[[imports.inputs]]\nref = '~@/outer'\nas = 'inner'\n",
            input("outer", kind)
        ),
    );
    write(
        dir.path(),
        "person.toml",
        &format!(
            "type = '{COWORKER_TYPE}'\nname = 'Person'\n{}",
            input("inner", kind)
        ),
    );
    dir
}

#[test]
fn typed_requirements_follow_renamed_inputs_without_reading_values() {
    let dir = fixture(SECRET_TYPE);
    let report = inspect_requirements(dir.path().join("CWR.toml"), &EmptyResourceResolver).unwrap();
    assert!(report.issues.is_empty(), "{:?}", report.issues);
    assert_eq!(report.requirements.len(), 2);
    assert!(report
        .requirements
        .iter()
        .all(|row| row.status == RequirementStatus::Missing));
    let child = &report.requirements[1];
    assert_eq!(child.alias, "inner");
    assert_eq!(child.sources[0].alias, "outer");
    assert_eq!(child.sources[0].type_ref, SECRET_TYPE);
    assert!(!dir.path().join(".cwr").exists());
}

#[test]
fn forwarding_preserves_secret_types_even_when_a_value_is_concrete() {
    for destination in [PLAINTEXT_TYPE, SECRET_TYPE] {
        let dir = fixture(SECRET_TYPE);
        write(
            dir.path(),
            "person.toml",
            &format!(
                "type = '{COWORKER_TYPE}'\nname = 'Person'\n{}",
                input("inner", destination)
            ),
        );
        let options = GraphLoadOptions {
            inputs: json!({"outer":"must-not-appear-in-errors"}),
            ..Default::default()
        };
        let result = resolve_graph_with_options(
            dir.path().join("CWR.toml"),
            &EmptyResourceResolver,
            &options,
        );
        if destination == SECRET_TYPE {
            result.unwrap();
        } else {
            let error = result.unwrap_err().to_string();
            assert!(error.contains("preserve"), "{error}");
            assert!(!error.contains("must-not-appear"));
        }
    }
}

#[test]
fn legacy_schema_boundaries_are_opaque_and_reject_typed_secrets() {
    let dir = fixture(SECRET_TYPE);
    write(
        dir.path(),
        "person.toml",
        &format!(
            "type = '{COWORKER_TYPE}'\nname = 'Person'\n[input_schema]\npath = './schema.json'\n"
        ),
    );
    write(
        dir.path(),
        "schema.json",
        r#"{"allOf":[{"type":"object"},{"properties":{"inner":{"type":"string"}},"required":["inner"]}]}"#,
    );
    let report = inspect_requirements(dir.path().join("CWR.toml"), &EmptyResourceResolver).unwrap();
    assert!(report
        .issues
        .iter()
        .any(|issue| issue.status == RequirementStatus::Unsupported));
    assert!(report
        .issues
        .iter()
        .any(|issue| issue.message.contains("secret input cannot cross")));
    let options = GraphLoadOptions {
        inputs: json!({"outer":"private-credential"}),
        ..Default::default()
    };
    assert!(resolve_graph_with_options(
        dir.path().join("CWR.toml"),
        &EmptyResourceResolver,
        &options
    )
    .is_err());
}

#[test]
fn omitted_optional_secrets_can_cross_opaque_boundaries() {
    for mapping in [
        "[[imports.inputs]]\nref = '~@/outer'\nas = 'inner'\n",
        "[imports.inputs]\ninner = { input = 'outer' }\n",
    ] {
        let dir = fixture(SECRET_TYPE);
        write(
            dir.path(),
            "CWR.toml",
            &format!(
                "type = '{RUNTIME_TYPE}'\n{}required = false\n[[imports]]\npath = './person.toml'\n{mapping}",
                input("outer", SECRET_TYPE)
            ),
        );
        write(
            dir.path(),
            "person.toml",
            &format!(
                "type = '{COWORKER_TYPE}'\nname = 'Person'\n[input_schema]\npath = './schema.json'\n"
            ),
        );
        // Only an empty object can satisfy this schema: the omitted input must
        // disappear from execution values as well as the secret-boundary check.
        write(
            dir.path(),
            "schema.json",
            r#"{"type":"object","maxProperties":0}"#,
        );
        let path = dir.path().join("CWR.toml");
        let report = inspect_requirements(&path, &EmptyResourceResolver).unwrap();
        assert!(!report.issues.is_empty());
        assert!(
            report
                .issues
                .iter()
                .all(|issue| issue.status == RequirementStatus::Unsupported),
            "{report:?}"
        );
        assert!(report.requirements.iter().any(|row| row.alias == "outer"
            && !row.required
            && row.status == RequirementStatus::Missing));
        resolve_graph(&path).unwrap();

        // An optional value that is present, even an empty string or an unread
        // available environment binding, still cannot cross the opaque boundary.
        write(
            dir.path(),
            "schema.json",
            r#"{"type":"object","properties":{"inner":{"type":"string"}}}"#,
        );
        let mut unread = InputBinding::missing();
        unread.status = RequirementStatus::Supplied;
        unread.environment = Some("CWR_OPAQUE_UNREAD_SECRET".into());
        for binding in [
            InputBinding::concrete(json!("private-opaque-secret")),
            InputBinding::concrete(json!("")),
            unread,
        ] {
            let (report, _) = inspect_bound_graph(
                &path,
                &EmptyResourceResolver,
                &GraphLoadOptions::default(),
                InputBindings::from([("outer".into(), binding.clone())]),
            )
            .unwrap();
            assert!(
                report
                    .issues
                    .iter()
                    .any(|issue| issue.status == RequirementStatus::Invalid
                        && issue.message.contains("secret input cannot cross")),
                "{report:?}"
            );
            let wire = serde_json::to_string(&report).unwrap();
            assert!(!wire.contains("private-opaque-secret"));
            assert!(!wire.contains("CWR_OPAQUE_UNREAD_SECRET"));
            if let Some(value) = binding.value {
                let options = GraphLoadOptions {
                    inputs: json!({"outer":value}),
                    ..Default::default()
                };
                let error = resolve_graph_with_options(&path, &EmptyResourceResolver, &options)
                    .unwrap_err()
                    .to_string();
                assert!(error.contains("secret input cannot cross"), "{error}");
                assert!(!error.contains("private-opaque-secret"));
            }
        }
        assert!(!dir.path().join(".cwr").exists());
    }
}

#[test]
fn arbitrary_plaintext_objects_remain_values_through_multiple_mappings() {
    let dir = fixture(PLAINTEXT_TYPE);
    let body = fs::read_to_string(dir.path().join("person.toml")).unwrap();
    write(
        dir.path(),
        "person.toml",
        &(body
            + "[[integrations]]\npath = './integration.toml'\nas = 'loopback'\n[integrations.inputs]\npayload = { input = 'inner' }\n"),
    );
    write(
        dir.path(),
        "integration.toml",
        &format!(
            "type = '{INTEGRATION_TYPE}'\n{}[adapter]\npath = './adapter.toml'\n[params]\npayload = {{ input = 'payload' }}\n",
            input("payload", PLAINTEXT_TYPE)
        ),
    );
    write(
        dir.path(),
        "adapter.toml",
        &format!("type = '{ADAPTER_TYPE}'\nref = 'cwp://example.test/adapter'\nname = 'Fixture'\n"),
    );
    let value =
        json!({"input":"literal-input-name","env":"literal-env-name","$cwr":{"missing":true}});
    let options = GraphLoadOptions {
        inputs: json!({"outer":value}),
        ..Default::default()
    };
    let graph = resolve_graph_with_options(
        dir.path().join("CWR.toml"),
        &EmptyResourceResolver,
        &options,
    )
    .unwrap();
    let integration = graph
        .resources
        .iter()
        .find(|resource| resource.type_ref == INTEGRATION_TYPE)
        .unwrap();
    assert_eq!(integration.document["params"]["payload"], value);
}

#[test]
fn optional_inputs_and_unknown_types_are_reported_without_schema_inference() {
    let dir = fixture(PLAINTEXT_TYPE);
    let body = fs::read_to_string(dir.path().join("CWR.toml")).unwrap();
    write(
        dir.path(),
        "CWR.toml",
        &(body
            + &input("optional", PLAINTEXT_TYPE)
            + "required = false\n"
            + &input("unknown", "cwp://example.invalid/type")),
    );
    let report = inspect_requirements(dir.path().join("CWR.toml"), &EmptyResourceResolver).unwrap();
    assert!(report
        .requirements
        .iter()
        .any(|row| row.alias == "optional" && !row.required));
    assert!(report
        .issues
        .iter()
        .any(|issue| issue.status == RequirementStatus::Unsupported));
}

#[test]
fn a_reused_receiver_still_checks_secret_type_on_every_edge() {
    for locator in ["path = './person.toml'", "ref = '~@/person'"] {
        let dir = fixture(SECRET_TYPE);
        write(
            dir.path(),
            "person.toml",
            &format!(
                "type = '{COWORKER_TYPE}'\nname = 'Person'\n{}",
                input("inner", PLAINTEXT_TYPE)
            ),
        );
        write(
            dir.path(),
            "CWR.toml",
            &format!(
                "type = '{RUNTIME_TYPE}'\n{}[[imports]]\npath = './person.toml'\nas = 'person'\n[imports.inputs]\ninner = 'same-value'\n[[imports]]\n{locator}\n[[imports.inputs]]\nref = '~@/outer'\nas = 'inner'\n",
                input("outer", SECRET_TYPE)
            ),
        );
        let options = GraphLoadOptions {
            inputs: json!({"outer":"same-value"}),
            ..Default::default()
        };
        let error = resolve_graph_with_options(
            dir.path().join("CWR.toml"),
            &EmptyResourceResolver,
            &options,
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("preserve"), "{error}");
        assert!(!error.contains("same-value"));
    }
}

#[test]
fn opaque_forwarding_does_not_hide_requirements_from_other_imports() {
    let dir = fixture(PLAINTEXT_TYPE);
    write(
        dir.path(),
        "opaque.toml",
        &format!(
            "type = '{RUNTIME_TYPE}'\n[input_schema]\npath = './opaque.json'\n[[imports]]\npath = './person.toml'\n[imports.inputs]\ninner = {{ input = 'unknown_legacy_input' }}\n"
        ),
    );
    write(
        dir.path(),
        "opaque.json",
        r#"{"anyOf":[{"type":"object","required":["unknown_legacy_input"]},{"type":"null"}]}"#,
    );
    let original = fs::read_to_string(dir.path().join("CWR.toml")).unwrap();
    write(
        dir.path(),
        "CWR.toml",
        &(original + "[[imports]]\npath = './opaque.toml'\n"),
    );
    let report = inspect_requirements(dir.path().join("CWR.toml"), &EmptyResourceResolver).unwrap();
    assert!(report
        .issues
        .iter()
        .any(|issue| issue.status == RequirementStatus::Unsupported));
    assert!(report.requirements.iter().any(|row| row.alias == "inner"));
}

#[test]
fn unbound_loop_inspection_includes_separate_volume_requirements() {
    let dir = fixture(PLAINTEXT_TYPE);
    let original = fs::read_to_string(dir.path().join("CWR.toml")).unwrap();
    write(
        dir.path(),
        "CWR.toml",
        &(original + "[[volumes]]\nas = 'project'\naccess = 'read'\n"),
    );
    let report = inspect_requirements(dir.path().join("CWR.toml"), &EmptyResourceResolver).unwrap();
    let volume = report
        .requirements
        .iter()
        .find(|row| row.alias == "project")
        .unwrap();
    assert_eq!(volume.type_ref, "volume");
    assert_eq!(volume.status, RequirementStatus::Missing);
    assert_eq!(volume.requested_access.as_deref(), Some("read"));
}

#[test]
fn omitted_optional_secrets_do_not_taint_nested_plaintext_objects_or_arrays() {
    for nested in [
        "{ keep = 'literal', omit = { input = 'outer' } }",
        "['literal', { input = 'outer' }]",
    ] {
        let dir = fixture(SECRET_TYPE);
        write(
            dir.path(),
            "CWR.toml",
            &format!(
                "type = '{RUNTIME_TYPE}'\n{}required = false\n[[imports]]\npath = './person.toml'\n[imports.inputs]\ninner = {nested}\n",
                input("outer", SECRET_TYPE)
            ),
        );
        write(
            dir.path(),
            "person.toml",
            &format!(
                "type = '{COWORKER_TYPE}'\nname = 'Person'\n{}",
                input("inner", PLAINTEXT_TYPE)
            ),
        );
        let path = dir.path().join("CWR.toml");
        let report = inspect_requirements(&path, &EmptyResourceResolver).unwrap();
        assert!(report.issues.is_empty(), "{report:?}");
        let row = report
            .requirements
            .iter()
            .find(|row| row.alias == "inner")
            .unwrap();
        assert_eq!(row.status, RequirementStatus::Supplied);
        assert!(row.sources.is_empty());
        resolve_graph(&path).unwrap();

        // Optional does not mean omitted when the host says the value is available.
        let mut unknown = InputBinding::missing();
        unknown.status = RequirementStatus::Supplied;
        unknown.environment = Some("unread-secret".into());
        let (report, _) = inspect_bound_graph(
            &path,
            &EmptyResourceResolver,
            &GraphLoadOptions::default(),
            InputBindings::from([("outer".into(), unknown)]),
        )
        .unwrap();
        assert!(
            report
                .issues
                .iter()
                .any(|issue| issue.message.contains("preserve")),
            "{report:?}"
        );
        let options = GraphLoadOptions {
            inputs: json!({"outer":"supplied-secret"}),
            ..Default::default()
        };
        assert!(resolve_graph_with_options(&path, &EmptyResourceResolver, &options).is_err());
    }
}

#[test]
fn repeated_resources_compare_concrete_values_independently_of_alias_provenance() {
    for locator in ["path = './person.toml'", "ref = '~@/person'"] {
        let dir = fixture(PLAINTEXT_TYPE);
        write(
            dir.path(),
            "CWR.toml",
            &format!(
                "type = '{RUNTIME_TYPE}'\n{}{}[[imports]]\npath = './person.toml'\nas = 'person'\n[imports.inputs]\ninner = {{ input = 'left' }}\n[[imports]]\n{locator}\n[imports.inputs]\ninner = {{ input = 'right' }}\n",
                input("left", PLAINTEXT_TYPE),
                input("right", PLAINTEXT_TYPE)
            ),
        );
        let path = dir.path().join("CWR.toml");
        for value in [
            json!("equal"),
            json!({"input":"literal", "env":"literal", "nested":[1,null]}),
        ] {
            let options = GraphLoadOptions {
                inputs: json!({"left":value,"right":value}),
                ..Default::default()
            };
            let (report, projected) = inspect_bound_graph(
                &path,
                &EmptyResourceResolver,
                &options,
                concrete_bindings(&options.inputs).unwrap(),
            )
            .unwrap();
            assert!(report.issues.is_empty(), "{report:?}");
            assert!(projected.is_some());
            let graph =
                resolve_graph_with_options(&path, &EmptyResourceResolver, &options).unwrap();
            assert_eq!(graph.actors.len(), 1);
        }
        let options = GraphLoadOptions {
            inputs: json!({"left":1,"right":2}),
            ..Default::default()
        };
        // Eager path imports fail while loading; deferred refs return a failed
        // inspection report. Neither route may project an activatable graph.
        match inspect_bound_graph(
            &path,
            &EmptyResourceResolver,
            &options,
            concrete_bindings(&options.inputs).unwrap(),
        ) {
            Err(error) => assert!(error.to_string().contains("conflicting effective inputs")),
            Ok((report, projected)) => {
                assert!(projected.is_none());
                assert!(report
                    .issues
                    .iter()
                    .any(|issue| issue.message.contains("conflicting effective inputs")));
            }
        }
        assert!(resolve_graph_with_options(&path, &EmptyResourceResolver, &options).is_err());
    }
}

#[test]
fn repeated_optional_inputs_distinguish_omission_from_a_supplied_value_in_either_order() {
    for supplied_first in [false, true] {
        let dir = fixture(PLAINTEXT_TYPE);
        write(
            dir.path(),
            "person.toml",
            &format!(
                "type = '{COWORKER_TYPE}'\nname = 'Person'\n{}required = false\n",
                input("inner", PLAINTEXT_TYPE)
            ),
        );
        let absent = "[[imports]]\npath = './person.toml'\n";
        let supplied = "[[imports]]\npath = './person.toml'\n[imports.inputs]\ninner = 'present'\n";
        let edges = if supplied_first {
            format!("{supplied}{absent}")
        } else {
            format!("{absent}{supplied}")
        };
        write(
            dir.path(),
            "CWR.toml",
            &format!("type = '{RUNTIME_TYPE}'\n{edges}"),
        );
        let path = dir.path().join("CWR.toml");
        let error = inspect_requirements(&path, &EmptyResourceResolver).unwrap_err();
        assert!(error.to_string().contains("conflicting effective inputs"));
        assert!(resolve_graph(&path).is_err());
    }
}

#[test]
fn unresolved_reuse_preserves_object_shape_and_defers_unknown_equality() {
    for (second, expected) in [
        ("{ first = { input = 'outer' }, literal = 1 }", None),
        (
            "{ second = { input = 'outer' }, literal = 1 }",
            Some(RequirementStatus::Unsupported),
        ),
        (
            "{ first = { input = 'outer' }, literal = 2 }",
            Some(RequirementStatus::Unsupported),
        ),
    ] {
        let dir = fixture(PLAINTEXT_TYPE);
        write(
            dir.path(),
            "CWR.toml",
            &format!(
                "type = '{RUNTIME_TYPE}'\n{}[[imports]]\npath = './person.toml'\nas = 'person'\n[imports.inputs]\ninner = {{ first = {{ input = 'outer' }}, literal = 1 }}\n[[imports]]\nref = '~@/person'\n[imports.inputs]\ninner = {second}\n",
                input("outer", PLAINTEXT_TYPE)
            ),
        );
        let path = dir.path().join("CWR.toml");
        let report = inspect_requirements(&path, &EmptyResourceResolver).unwrap();
        match expected {
            Some(status) => assert!(
                report.issues.iter().any(|issue| issue.status == status
                    && issue
                        .message
                        .contains("equivalence requires concrete values")),
                "{report:?}"
            ),
            None => assert!(report.issues.is_empty(), "{report:?}"),
        }
        let options = GraphLoadOptions {
            inputs: json!({"outer":"concrete"}),
            ..Default::default()
        };
        assert_eq!(
            resolve_graph_with_options(&path, &EmptyResourceResolver, &options).is_ok(),
            expected.is_none()
        );
    }
}
