use super::*;

fn decision_role(dir: &Path, name: &str, actions: &[&str]) {
    let mut text = format!(
        r#"type = "{ROLE_TYPE}"
[[adapters]]
ref = "cwp://org.coworkerprotocol/2026-09/adapters/decisions"
[adapters.params]
model = "test/model"
action_name = "grade--result"
rubric_version = "test-1"
[adapters.params.questions.support]
type = "noul"
instructions = "Is the result supported?"
"#
    );
    for object in actions {
        text.push_str(&format!(
            r#"
[[actions]]
verb = "grade"
object = "{object}"
title = "Grade"
description = "Grade supplied evidence"
[actions.input_schema]
path = "./schema.json"
[actions.output_schema]
path = "./schema.json"
"#
        ));
    }
    write(dir, &format!("{name}/ROLE.toml"), &text);
    write(dir, &format!("{name}/schema.json"), r#"{"type":"object"}"#);
}

fn fixture(dir: &Path, extra_role: bool) {
    root(dir, "[[imports]]\npath = './grader/COWORKER.toml'\n");
    write(
        dir,
        "decisions.toml",
        &format!(
            r#"type = "{INTEGRATION_TYPE}"
[adapter]
ref = "cwp://org.coworkerprotocol/2026-09/adapters/decisions"
[params]
endpoint = "https://openrouter.ai/api/alpha/decisions"
"#
        ),
    );
    let additional = if extra_role {
        "[[roles]]\npath = '../other/ROLE.toml'\n"
    } else {
        ""
    };
    person(dir, "grader", &format!("[[roles]]\npath = '../judge/ROLE.toml'\n{additional}[[integrations]]\npath = '../decisions.toml'\nas = 'decisions'\nmessage_mode = 'actions_only'\n"));
}

#[test]
fn decisions_routes_must_match_the_effective_binding_action() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path();
    fixture(dir, false);
    decision_role(dir, "judge", &["result"]);
    let graph = resolve_graph(dir.join("CWR.toml")).unwrap();
    assert_eq!(graph.routes.len(), 1);
    assert_eq!(graph.routes[0].action_name, "grade--result");
    assert_eq!(
        graph.routes[0].adapter_params["action_name"],
        "grade--result"
    );
    for actions in [vec!["other"], vec!["result", "other"]] {
        decision_role(dir, "judge", &actions);
        let error = resolve_graph(dir.join("CWR.toml")).unwrap_err().to_string();
        assert!(
            error.contains("each advertised action must match"),
            "{error}"
        );
        assert!(error.contains("grade--other"), "{error}");
    }
}

#[test]
fn decisions_composed_roles_cannot_add_an_unrelated_action() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path();
    fixture(dir, true);
    decision_role(dir, "judge", &["result"]);
    decision_role(dir, "other", &[]);
    assert_eq!(resolve_graph(dir.join("CWR.toml")).unwrap().routes.len(), 1);
    decision_role(dir, "other", &["other"]);
    let error = resolve_graph(dir.join("CWR.toml")).unwrap_err().to_string();
    assert!(
        error.contains("each advertised action must match"),
        "{error}"
    );
    assert!(error.contains("grade--other"), "{error}");
}

#[test]
fn decisions_graph_rejects_invalid_endpoint_semantics() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path();
    fixture(dir, false);
    decision_role(dir, "judge", &["result"]);
    let path = dir.join("decisions.toml");
    let original = std::fs::read_to_string(&path).unwrap();
    for endpoint in [
        "https://example.com:65536/grade",
        "https://example.com:bad/grade",
        "https://example.com:0/grade",
        "https://user@example.com/grade",
        "https://example.com/grade?x=1",
        "https://example.com/grade#part",
        "http://example.com/grade",
    ] {
        std::fs::write(
            &path,
            original.replace("https://openrouter.ai/api/alpha/decisions", endpoint),
        )
        .unwrap();
        let error = resolve_graph(dir.join("CWR.toml")).unwrap_err().to_string();
        assert!(
            error.contains("invalid Decisions endpoint") || error.contains("at /endpoint"),
            "{endpoint}: {error}"
        );
    }
}
