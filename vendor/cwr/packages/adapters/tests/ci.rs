use cwr::{
    adapter::{
        run_process_adapter, ProcessAdapterOutcome, ProcessAdapterStopControl,
        ResolvedProcessAdapter,
    },
    v2026_09::graph::{resolve_graph_with_options, EmptyResourceResolver, GraphLoadOptions},
};
use serde_json::json;
use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf};

#[test]
fn repository_ci_preserves_check_failures_and_diagnostics() {
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let graph = resolve_graph_with_options(
        workspace.join("CWR.toml"),
        &EmptyResourceResolver,
        &GraphLoadOptions {
            source_roots: vec![workspace.clone()],
            working_path: Some(workspace.clone()),
            ..Default::default()
        },
    )
    .unwrap();
    let actor = graph
        .actors
        .iter()
        .find(|actor| actor.coworker_slug == "deveng_ops")
        .expect("deveng coworker");
    let route = graph
        .routes
        .iter()
        .find(|route| {
            route.actor_ref == actor.actor_ref && route.action_name == "check--rust_format"
        })
        .expect("repository CI formatting action");
    assert_eq!(route.working_path, workspace);
    assert_eq!(route.actor_integration_params["stream_output"], true);

    // Intercept direnv instead of checking the developer's formatting. Exercise
    // the checked-in route, parameter schema, adapter process, and terminal result.
    let tools = tempfile::tempdir().unwrap();
    fs::write(
        tools.path().join(".bash_profile"),
        "if shopt -q login_shell; then\nexport PATH=\"$HOME\"\nexport LOGIN_LOADED=yes\nfi\n",
    )
    .unwrap();
    // Preserve Bash login mode, but load only fixture startup through BASH_ENV.
    let shell = tools.path().join("bash");
    fs::write(&shell, "#!/bin/bash\nexec /bin/bash --noprofile \"$@\"\n").unwrap();
    fs::set_permissions(&shell, fs::Permissions::from_mode(0o700)).unwrap();
    let direnv = tools.path().join("direnv");
    fs::write(
        &direnv,
        r#"#!/bin/sh
[ "$#" = 5 ] && [ "$1" = exec ] && [ "$2" = . ] || exit 41
[ "$3" = "$HOME/bash" ] && [ "$4" = -c ] && [ "$5" = "cargo fmt --all -- --check" ] || exit 41
[ "$LOGIN_LOADED" = yes ] || exit 43
[ -f Cargo.toml ] || exit 42
printf 'format diagnostics'
printf 'check stderr' >&2
exit "$CHECK_EXIT"
"#,
    )
    .unwrap();
    fs::set_permissions(&direnv, fs::Permissions::from_mode(0o700)).unwrap();
    for (exit, status) in [("0", 200), ("1", 500)] {
        let adapter = ResolvedProcessAdapter::new(env!("CARGO_BIN_EXE_cwr-adapter-sh"))
            .with_current_directory(&route.working_path)
            .with_environment("HOME", tools.path())
            .with_environment("SHELL", &shell)
            .with_environment("BASH_ENV", tools.path().join(".bash_profile"))
            // System login profiles may invoke basic tools before our fixture profile.
            .with_environment("PATH", format!("{}:/usr/bin:/bin", tools.path().display()))
            .with_environment("CHECK_EXIT", exit);
        let invocation = route
            .project_adapter_invocation(
                format!("cwp://com.example.runtime/invocations/ci-{exit}"),
                json!({}),
                json!({}),
            )
            .unwrap();
        let outcome = run_process_adapter(
            &adapter,
            invocation,
            &ProcessAdapterStopControl::new(),
            |_| {},
        )
        .unwrap();
        let ProcessAdapterOutcome::Completed(terminal) = outcome else {
            panic!("CI check unexpectedly stopped");
        };
        assert_eq!(terminal.status, status, "{terminal:?}");
        let result = if status == 500 {
            assert!(terminal.result.is_none());
            let error = terminal.error.unwrap();
            assert_eq!(error.code, "command_failed");
            error.details.unwrap()
        } else {
            assert!(terminal.error.is_none());
            terminal.result.unwrap()
        };
        assert_eq!(result["exit_code"], exit.parse::<i32>().unwrap());
        assert_eq!(result["stdout"], "format diagnostics");
        assert_eq!(result["stderr"], "check stderr");
    }
}
