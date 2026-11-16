use std::{
    fs,
    process::{Command, Output},
    sync::atomic::{AtomicUsize, Ordering},
};

static NEXT: AtomicUsize = AtomicUsize::new(0);

fn check(definitions: &str, examples: &str) -> Output {
    let directory = std::env::temp_dir().join(format!(
        "rbel literate {} {}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&directory).unwrap();
    let source = directory.join("definitions.rbel");
    let rows = directory.join("examples.bel");
    fs::write(&source, definitions).unwrap();
    fs::write(&rows, examples).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_rbel-check-examples"))
        .arg(source)
        .arg(rows)
        .output()
        .unwrap();
    fs::remove_dir_all(directory).unwrap();
    output
}

#[test]
fn examples_load_extracted_definitions_and_start_fresh() {
    let output = check(
        "(def no (x) (id x nil)) (set x 'initial)",
        "(definition (no nil) t) (mutate (set x 'changed) changed) (fresh x initial)",
    );
    assert!(output.status.success(), "{:?}", output.stderr);
    assert!(String::from_utf8_lossy(&output.stdout).contains("3 literate examples passed"));
}

#[test]
fn expected_values_are_data_and_failures_name_the_example() {
    let output = check("", "(literal 'a (err expected-was-executed))");
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("literal: expected (err expected-was-executed), got a")
    );
}

#[test]
fn missing_definitions_errors_mismatches_and_invalid_rows_fail() {
    for examples in [
        "(missing (no nil) t)",
        "(error (err 'bad) nil)",
        "(wrong 'a b)",
        "(extra 'a a b)",
        "(dotted 'a . b)",
        "",
        ")",
    ] {
        let output = check("", examples);
        assert!(!output.status.success(), "{examples}");
    }
}
