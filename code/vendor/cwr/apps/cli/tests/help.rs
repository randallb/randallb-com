use std::{fs, process::Command};

#[test]
fn help_exits_successfully_before_any_command_arguments_are_used() {
    let temporary = tempfile::tempdir().expect("temporary help workspace");
    let cases: &[(&[&str], &str)] = &[
        (&["--help"], "usage: cwr [--workspace"),
        (&["-h"], "usage: cwr [--workspace"),
        (&["init", "--help"], "usage: cwr init"),
        (&["init", "-h"], "usage: cwr init"),
        (&["check", "--help"], "usage: cwr [--workspace"),
        (&["exec", "--help"], "usage: cwr [--workspace"),
        (&["exec", "--json", "--help"], "usage: cwr [--workspace"),
        (
            &["exec", "resume", "--help", "ordinary message"],
            "usage: cwr [--workspace",
        ),
        (
            &["exec", "resume", "~@/conversations/example", "--help"],
            "usage: cwr [--workspace",
        ),
        (&["actions", "--help"], "usage: cwr [--workspace"),
        (&["actions", "list", "--help"], "usage: cwr [--workspace"),
        (&["actions", "invoke", "--help"], "usage: cwr [--workspace"),
        (
            &["actions", "invoke", "@helper--run--task", "--help"],
            "usage: cwr [--workspace",
        ),
        (
            &["actions", "invoke", "@cwr--stop--invocation", "--help"],
            "usage: cwr [--workspace",
        ),
        (&["resume", "--help"], "usage: cwr [--workspace"),
        (&["--actor", "--help"], "usage: cwr [--workspace"),
        (
            &["--actor", "--help", "resume", "last"],
            "usage: cwr [--workspace",
        ),
        (
            &["--actor", "~@/actors/person", "resume", "--help"],
            "usage: cwr [--workspace",
        ),
        (
            &["--actor", "~@/actors/person", "resume", "last", "--help"],
            "usage: cwr [--workspace",
        ),
    ];

    for (arguments, expected) in cases {
        let output = Command::new(env!("CARGO_BIN_EXE_cwr"))
            .args(*arguments)
            .current_dir(temporary.path())
            .output()
            .expect("run cwr help");
        assert!(output.status.success(), "{arguments:?}: {output:?}");
        assert!(output.stderr.is_empty(), "{arguments:?}: {output:?}");
        assert!(
            String::from_utf8_lossy(&output.stdout).starts_with(expected),
            "{arguments:?}: {output:?}"
        );
    }

    assert!(
        fs::read_dir(temporary.path())
            .expect("read temporary help workspace")
            .next()
            .is_none(),
        "help must not initialize a workspace or create command state"
    );
}
