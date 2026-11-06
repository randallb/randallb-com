use std::{
    fs,
    process::{Command, Output},
    sync::atomic::{AtomicUsize, Ordering},
};

static NEXT: AtomicUsize = AtomicUsize::new(0);

fn evaluate(source: &str, data: Option<&str>) -> Output {
    evaluate_with_options(source, data, &[])
}

fn evaluate_with_options(source: &str, data: Option<&str>, options: &[&str]) -> Output {
    let directory = std::env::temp_dir().join(format!(
        "bel text {} {}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&directory).unwrap();
    let library = directory.join("library.bel");
    fs::write(&library, source).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_bel-eval-text"));
    command.args(options);
    if let Some(data) = data {
        let path = directory.join("data.bel");
        fs::write(&path, data).unwrap();
        command.args(["--call", "emit"]).arg(path);
    }
    let output = command.arg(library).output().unwrap();
    fs::remove_dir_all(directory).unwrap();
    output
}

#[test]
fn batch_outputs_text_without_repl_framing() {
    let output = evaluate("(list \"hello\" (list \" \" \"world\") \\lf)", None);
    assert!(output.status.success(), "{:?}", output.stderr);
    assert_eq!(output.stdout, b"hello world\n");
    assert!(output.stderr.is_empty());
}

#[test]
fn batch_data_is_parsed_as_all_forms_and_never_evaluated() {
    let output = evaluate(
        "(def emit (forms) (if (= forms '((err 'not-executed) tail)) \"data\" (err 'bad)))",
        Some("(err 'not-executed) tail"),
    );
    assert!(output.status.success(), "{:?}", output.stderr);
    assert_eq!(output.stdout, b"data");
    let malformed = evaluate("(def emit (forms) \"ignored\")", Some(")"));
    assert!(!malformed.status.success());
    assert!(malformed.stdout.is_empty());
}

#[test]
fn batch_rejects_errors_and_invalid_or_cyclic_text_without_partial_output() {
    for source in [
        "(err 'deliberate)",
        "(list \"prefix\" 'invalid)",
        "(let x (list \\a) (do (xdr x x) x))",
        "(def grow (xs value) (if xs (grow (cdr xs) (join value value)) value))\n(grow '(a b c d e f g h i j k l m n o p q) \"x\")",
    ] {
        let output = evaluate(source, None);
        assert!(!output.status.success(), "accepted {source}");
        assert!(output.stdout.is_empty(), "partial output for {source}");
        assert!(!output.stderr.is_empty());
    }
}

#[test]
fn bel_compiler_uses_source_and_rejects_unsupported_forms() {
    let library = format!(
        "{}\n(def emit (forms) (wasm-compile-forms forms))",
        include_str!("../wasm/compiler.bel")
    );
    for (source, expected) in [
        (
            "(fn (x) (if x 't 'nil))",
            "(if (result i32) (local.get 0) (then (i32.const 1)) (else (i32.const 0)))",
        ),
        (
            "(fn (x) (if x 'nil 'nil))",
            "(if (result i32) (local.get 0) (then (i32.const 0)) (else (i32.const 0)))",
        ),
    ] {
        let output = evaluate(&library, Some(source));
        assert!(output.status.success(), "{:?}", output.stderr);
        let text = String::from_utf8(output.stdout).unwrap();
        assert!(text.contains(expected), "{text}");
    }
    for source in [
        "(fn (x) (if x 0 nil))",
        "(fn (o) (if o nil t))",
        "(fn (apply) (if apply nil t))",
    ] {
        let rejected = evaluate(&library, Some(source));
        assert!(!rejected.status.success());
        assert!(rejected.stdout.is_empty());
        assert!(
            String::from_utf8(rejected.stderr)
                .unwrap()
                .contains("unsupported-wasm-source")
        );
    }
    // These are literals in published Bel, not legal bindable parameters.
    let mut host = bel_rust_repl::Interpreter::default();
    bel_rust_repl::eval_bootstrap(&mut host).unwrap();
    for source in ["(fn (o) (if o nil t))", "(fn (apply) (if apply nil t))"] {
        assert!(
            host.eval_source(&format!("({source} nil)")).is_err(),
            "host bound literal parameter in {source}"
        );
    }
}

#[test]
fn larger_batch_output_requires_explicit_allowance() {
    let source = "(def grow (xs value) (if xs (grow (cdr xs) (join value value)) value))\n(grow '(a b c d e f g h i j k l m n o p q) \"x\")";
    let default = evaluate(source, None);
    assert!(!default.status.success());
    assert!(default.stdout.is_empty());
    let expanded = evaluate_with_options(source, None, &["--max-text-bytes", "262144"]);
    assert!(expanded.status.success(), "{:?}", expanded.stderr);
    assert_eq!(expanded.stdout.len(), 131_072);
    assert!(expanded.stdout.iter().all(|byte| *byte == b'x'));
}

#[test]
fn invalid_batch_limits_fail_before_reading_or_output() {
    for options in [
        ["--max-text-bytes", "65535"],
        ["--max-text-bytes", "16777217"],
        ["--max-eval-depth", "0"],
        ["--max-eval-depth", "4097"],
    ] {
        let output = evaluate_with_options("\"unused\"", None, &options);
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(
            String::from_utf8(output.stderr)
                .unwrap()
                .contains("must be between")
        );
    }
}

#[test]
fn reader_json_preserves_dotted_pairs_and_escaped_characters_without_evaluation() {
    let path = std::env::temp_dir().join(format!("bel-reader-json-{}.bel", std::process::id()));
    fs::write(&path, r#"(err 'must-not-run) (a . b) \tab \lf \" \\ \🙂"#).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_bel-read-json"))
        .arg(&path)
        .output()
        .unwrap();
    fs::remove_file(path).unwrap();
    assert!(output.status.success(), "{:?}", output.stderr);
    assert_eq!(
        String::from_utf8(output.stdout).unwrap().trim(),
        r#"[["err",["quote","must-not-run"]],{"pair":["a","b"]},{"char":"\t"},{"char":"\n"},{"char":"\""},{"char":"\\"},{"char":"🙂"}]"#
    );
}
