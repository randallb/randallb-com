use std::{fs, path::Path, process::Command};

fn workspace(root: &Path, unbound_count: usize) {
    fs::write(
        root.join("CWR.toml"),
        "type = 'cwp://org.coworkerprotocol/2026-09/schemas'\n[[imports]]\npath = './alice.toml'\n[[imports]]\npath = './bob.toml'\n",
    )
    .unwrap();
    for (index, name) in ["alice", "bob"].iter().enumerate() {
        let mut declaration = format!(
            "type = 'cwp://org.coworkerprotocol/2026-09/schemas/coworker'\nname = '{name}'\nslug = '{name}'\n"
        );
        if index >= unbound_count {
            declaration.push_str(
                "[[roles]]\npath = './ROLE.toml'\n[[integrations]]\npath = './integration.toml'\nas = 'local'\n",
            );
        }
        fs::write(root.join(format!("{name}.toml")), declaration).unwrap();
    }
    fs::write(
        root.join("ROLE.toml"),
        "type = 'cwp://org.coworkerprotocol/2026-09/schemas/role'\nname = 'Helper'\n[[adapters]]\nref = 'cwp://org.coworkerprotocol/2026-09/adapters/codex'\n[adapters.params]\nentrypoint = './PROMPT.md'\n",
    )
    .unwrap();
    fs::write(
        root.join("integration.toml"),
        "type = 'cwp://org.coworkerprotocol/2026-09/schemas/integration'\n[adapter]\nref = 'cwp://org.coworkerprotocol/2026-09/adapters/codex'\n",
    )
    .unwrap();
    fs::write(root.join("PROMPT.md"), "Help with the requested work.").unwrap();
}

fn actions(
    root: &Path,
    actor: Option<&str>,
    environment_actor: Option<&str>,
) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_cwr"));
    command
        .arg("--workspace")
        .arg(root)
        .current_dir(root)
        .env_remove("CWR_WORKSPACE")
        .env_remove("CWR_ACTOR_REF");
    if let Some(actor) = actor {
        command.args(["--actor", actor]);
    }
    if let Some(actor) = environment_actor {
        command.env("CWR_ACTOR_REF", actor);
    }
    command.args(["actions", "list"]).output().unwrap()
}

#[test]
fn explicit_actor_selection_works_without_an_implicit_default() {
    for unbound_count in [0, 2] {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        workspace(root, unbound_count);
        let graph = cwr::v2026_09::graph::resolve_graph(root.join("CWR.toml")).unwrap();
        let alice = &graph
            .actors
            .iter()
            .find(|actor| actor.coworker_slug == "alice")
            .unwrap()
            .actor_ref;
        let unknown = "cwp://example.test/actors/unavailable";

        for (argument, environment) in [
            (Some(alice.as_str()), Some(unknown)),
            (None, Some(alice.as_str())),
        ] {
            let output = actions(root, argument, environment);
            assert!(
                output.status.success(),
                "unbound={unbound_count}: {output:?}"
            );
            assert!(String::from_utf8_lossy(&output.stdout).contains("@cwr--list--coworkers"));
        }

        let unspecified = actions(root, None, None);
        assert!(!unspecified.status.success());
        assert!(
            String::from_utf8_lossy(&unspecified.stderr).contains(&format!(
                "found {unbound_count} actors without an actor integration"
            ))
        );

        let invalid = actions(root, Some(unknown), None);
        assert!(!invalid.status.success());
        assert!(String::from_utf8_lossy(&invalid.stderr).contains("is not in the resolved graph"));
        assert!(
            !root.join(".cwr").exists(),
            "action discovery must not open runtime state"
        );
    }
}
