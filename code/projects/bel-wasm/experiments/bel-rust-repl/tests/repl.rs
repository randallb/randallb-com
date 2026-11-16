use std::{
    fs,
    io::Write,
    process::{Command, Stdio},
};

#[test]
fn real_binary_loads_and_recovers_in_one_session() {
    let root = std::env::temp_dir().join(format!("bel repl acceptance {}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let source = root.join("loaded expression.bel");
    fs::write(&source, "'from-file\n").unwrap();
    let input = format!(
        ":help\n'hello\n(join 'a 'b)\n(car '(a b c))\n(if nil 'yes 'no)\n:bootstrap\n(no nil)\n(atom '(a b))\n(atom 'a)\n(all atom '(a b c))\n(some atom '((a) b c))\n(reduce join '(a b c))\n(cons 'a 'b 'c)\n(append '(a b) '(c d))\n(snoc '(a b) 'c)\n(list 'a 'b 'c)\n(map atom '((a) b (c)))\n(map join '(a b) '(c d))\n((fn (x) 'ignored x) 'macro-fn)\n(do 'ignored 'macro-do)\n(let x 'macro-let x)\n(or nil 'macro-or)\n(and 'a 'macro-and)\n(= '(a (b)) '(a (b)))\n([join _ 'tail] 'head)\n(mem 'b '(a b c))\n(with (x 'a y 'b) (list x y))\n(rev '(a b c))\n:load {}\n)\n'after-syntax\nmissing\n'after-unbound\n(\n:cancel\n'after-cancel\n:load {}\n'after-load-error\n:quit\n",
        source.display(),
        root.join("missing.bel").display()
    );
    let mut child = Command::new(env!("CARGO_BIN_EXE_bel-rust-repl"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let result = child.wait_with_output().unwrap();
    fs::remove_dir_all(root).unwrap();
    assert!(result.status.success());
    let output = String::from_utf8(result.stdout).unwrap();
    let errors = String::from_utf8(result.stderr).unwrap();
    for expected in [
        "bel> hello\n",
        "bel> (a . b)\n",
        "bel> a\n",
        "bel> no\n",
        "bel> t\n",
        "bel> nil\n",
        "bel> (b c)\n",
        "bel> (a b . c)\n",
        "bel> (a b c d)\n",
        "bel> (a b c)\n",
        "bel> (nil t nil)\n",
        "bel> ((a . c) (b . d))\n",
        "bel> macro-fn\n",
        "bel> macro-do\n",
        "bel> macro-let\n",
        "bel> macro-or\n",
        "bel> macro-and\n",
        "bel> t\n",
        "bel> (head . tail)\n",
        "bel> (b c)\n",
        "bel> (a b)\n",
        "bel> (c b a)\n",
        "bel> loaded 1 form(s)",
        "after-syntax\n",
        "after-unbound\n",
        "after-cancel\n",
        "after-load-error\n",
    ] {
        assert!(output.contains(expected), "missing {expected:?}: {output}");
    }
    assert!(output.contains("loaded 353 bootstrap form(s) from 31 fragment(s)"));
    assert_eq!(
        output
            .matches(
                "Use :bootstrap to load the complete unchanged 1,801-line published Bel source."
            )
            .count(),
        2
    );
    assert!(errors.contains("unexpected closing parenthesis"));
    assert!(errors.contains("unbound symbol: missing"));
    assert!(errors.contains("cannot load"));
}

#[test]
fn malformed_load_and_incomplete_eof_report_errors() {
    let mut output = Vec::new();
    let mut errors = Vec::new();
    bel_rust_repl::repl::run(
        &b"(\n:load absent.bel\n:cancel\n'works\n(\n"[..],
        &mut output,
        &mut errors,
    )
    .unwrap();
    let errors = String::from_utf8(errors).unwrap();
    assert!(errors.contains("finish the current expression"));
    assert!(errors.contains("incomplete expression at end"));
    assert!(String::from_utf8(output).unwrap().contains("works\n"));
}

#[test]
fn real_binary_recovers_from_exponentially_large_shared_pair_output() {
    let counters = vec!["n"; 40].join(" ");
    let input = format!(
        "(def grow (n x) (if n (grow (cdr n) (join x x)) x))\n(grow '({counters}) 'a)\n'after-output-error\n((grow '({counters}) 'a))\n'after-call-error\n:quit\n"
    );
    let mut child = Command::new(env!("CARGO_BIN_EXE_bel-rust-repl"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let result = child.wait_with_output().unwrap();
    assert!(result.status.success());
    let output = String::from_utf8(result.stdout).unwrap();
    let errors = String::from_utf8(result.stderr).unwrap();
    assert!(errors.contains("output limit reached"), "{errors}");
    assert!(
        errors.contains("cannot call #<value exceeds output limit>"),
        "{errors}"
    );
    assert!(
        output.contains("bel> bel> after-output-error\n"),
        "{output}"
    );
    assert!(output.contains("bel> bel> after-call-error\n"), "{output}");
    assert!(
        output.len() < 1024,
        "oversized values must not be partially printed"
    );
}

#[test]
fn real_binary_recovers_from_a_cyclic_closure_parameter_pattern() {
    let input = ":bootstrap\n(let p (join nil nil) (do (xar p p) ((list 'lit 'clo nil p 'ok))))\n'after-cycle\n:quit\n";
    let mut child = Command::new(env!("CARGO_BIN_EXE_bel-rust-repl"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let result = child.wait_with_output().unwrap();
    assert!(result.status.success(), "{:?}", result.status);
    let output = String::from_utf8(result.stdout).unwrap();
    let errors = String::from_utf8(result.stderr).unwrap();
    assert!(
        errors.contains("cyclic parameter pattern; the prompt is still usable"),
        "{errors}"
    );
    assert!(output.contains("bel> bel> after-cycle\n"), "{output}");
}

#[test]
fn real_binary_recovers_from_an_excessively_deep_complement_abbreviation() {
    let input = format!("{}a\n'after-abbreviation\n:quit\n", "~".repeat(50_000));
    let mut child = Command::new(env!("CARGO_BIN_EXE_bel-rust-repl"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let result = child.wait_with_output().unwrap();
    assert!(result.status.success(), "{:?}", result.status);
    let output = String::from_utf8(result.stdout).unwrap();
    let errors = String::from_utf8(result.stderr).unwrap();
    assert!(
        errors.contains("reader abbreviation exceeds the experiment's expansion limit"),
        "{errors}"
    );
    assert!(
        output.contains("bel> bel> after-abbreviation\n"),
        "{output}"
    );
}

#[test]
fn real_binary_rejects_a_large_string_before_allocating_its_pair_chain() {
    let input = format!("\"{}\"\n'after-large-string\n:quit\n", "a".repeat(60_000));
    let mut child = Command::new(env!("CARGO_BIN_EXE_bel-rust-repl"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let result = child.wait_with_output().unwrap();
    assert!(result.status.success(), "{:?}", result.status);
    let output = String::from_utf8(result.stdout).unwrap();
    let errors = String::from_utf8(result.stderr).unwrap();
    assert!(
        errors.contains("string literal exceeds the experiment's reader allocation limit"),
        "{errors}"
    );
    assert!(
        output.contains("bel> bel> after-large-string\n"),
        "{output}"
    );
}

#[test]
fn real_binary_recovers_after_discarding_a_large_flat_list() {
    let form = format!(
        "({})",
        std::iter::repeat_n("a", 32_767)
            .collect::<Vec<_>>()
            .join(" ")
    );
    assert_eq!(form.len() + 1, 65_536);
    let input = format!("{form}\n'after-flat-list\n:quit\n");
    let mut child = Command::new(env!("CARGO_BIN_EXE_bel-rust-repl"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let result = child.wait_with_output().unwrap();
    assert!(result.status.success(), "{:?}", result.status);
    let output = String::from_utf8(result.stdout).unwrap();
    let errors = String::from_utf8(result.stderr).unwrap();
    assert!(errors.contains("expected a finite proper list"), "{errors}");
    assert!(output.contains("bel> bel> after-flat-list\n"), "{output}");
}

#[test]
fn real_binary_recovers_from_cyclic_table_entries() {
    let input = ":bootstrap\n(let tab (table '((a . b))) (do (xdr (cddr tab) (cddr tab)) (where (tab 'missing))))\n'after-table-cycle\n:quit\n";
    let mut child = Command::new(env!("CARGO_BIN_EXE_bel-rust-repl"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let result = child.wait_with_output().unwrap();
    assert!(result.status.success(), "{:?}", result.status);
    let output = String::from_utf8(result.stdout).unwrap();
    let errors = String::from_utf8(result.stderr).unwrap();
    assert!(errors.contains("cyclic table entries"), "{errors}");
    assert!(output.contains("bel> bel> after-table-cycle\n"), "{output}");
}
