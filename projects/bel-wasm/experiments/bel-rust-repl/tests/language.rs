use std::rc::Rc;

use bel_rust_repl::{
    BOOTSTRAP_FRAGMENTS, Interpreter, Value, bootstrap_source, eval_bootstrap, read,
};

fn results(interpreter: &mut Interpreter, source: &str) -> Vec<String> {
    interpreter
        .eval_source(source)
        .unwrap()
        .iter()
        .map(ToString::to_string)
        .collect()
}

#[test]
fn public_pair_payload_remains_borrowable_and_mutable() {
    let value = Value::pair(Value::symbol("a"), Value::symbol("b"));
    let Value::Pair(pair) = &value else {
        panic!("pair constructor returned a non-pair");
    };
    assert_eq!(pair.borrow().0.name(), Some("a"));
    pair.borrow_mut().1 = Value::symbol("c");
    assert_eq!(value.to_string(), "(a . c)");
}

#[test]
fn dropping_an_interpreter_releases_its_global_environment() {
    let weak = {
        let mut interpreter = Interpreter::default();
        let value = interpreter.eval_source("car").unwrap().pop().unwrap();
        let Value::Pair(pair) = value else {
            panic!("primitive is not represented by a pair");
        };
        Rc::downgrade(&pair)
    };
    assert!(weak.upgrade().is_none());
}

#[test]
fn intent_expressions_and_loaded_functions_persist() {
    let mut interpreter = Interpreter::default();
    assert_eq!(
        results(
            &mut interpreter,
            "'hello (join 'a 'b) (car '(a b c)) (if nil 'yes 'no)"
        ),
        ["hello", "(a . b)", "a", "no"]
    );
    assert!(
        interpreter
            .eval_source("(no nil)")
            .unwrap_err()
            .0
            .contains("unbound")
    );
    eval_bootstrap(&mut interpreter).unwrap();
    assert_eq!(
        results(&mut interpreter, "(no nil) (atom '(a b)) (atom 'a)"),
        ["t", "nil", "t"]
    );
    interpreter.eval_source("(def no (x) 'changed)").unwrap();
    assert_eq!(results(&mut interpreter, "(atom 'a)"), ["changed"]);
}

#[test]
fn complete_published_source_is_the_acceptance_fixture() {
    let mut interpreter = Interpreter::default();
    assert_eq!(BOOTSTRAP_FRAGMENTS.len(), 31);
    assert_eq!(eval_bootstrap(&mut interpreter).unwrap(), 353);
    assert_eq!(read(&bootstrap_source()).unwrap().len(), 353);
    for (name, source) in BOOTSTRAP_FRAGMENTS {
        assert!(!source.trim().is_empty(), "{name}");
        assert!(read(source).is_ok(), "{name}");
    }
    assert_eq!(
        results(
            &mut interpreter,
            "(all atom nil) (all atom '(a b c)) (all atom '(a (b) c))"
        ),
        ["t", "t", "nil"]
    );
    assert_eq!(
        results(
            &mut interpreter,
            "(some atom nil) (some atom '((a) b c)) (some atom '((a) (b)))"
        ),
        ["nil", "(b c)", "nil"]
    );
    assert_eq!(
        results(
            &mut interpreter,
            "(reduce join '(a)) (reduce join '(a b c))"
        ),
        ["a", "(a b . c)"]
    );
    assert_eq!(
        results(
            &mut interpreter,
            "(cons 'a 'b 'c) (append '(a b) '(c d) 'e) (append nil '(a) nil '(b c)) (snoc '(a b) 'c 'd) (list) (list 'a 'b 'c)"
        ),
        [
            "(a b . c)",
            "(a b c d . e)",
            "(a b c)",
            "(a b c d)",
            "nil",
            "(a b c)"
        ]
    );
    assert_eq!(
        results(
            &mut interpreter,
            "(map atom '((a) b (c))) (map join '(a b) '(c d)) (map join '(a b c) '(d)) (map atom)"
        ),
        ["(nil t nil)", "((a . c) (b . d))", "((a . d))", "nil"]
    );
    assert_eq!(
        results(
            &mut interpreter,
            "(parameters '(x (o y z) . rest)) (all [get _ forms id] '(quote if where dyn after ccc thread)) (list (pair locfns) (no virfns)) (list (okparms '(x (o y) . rest)) (no (no (some [mem 'join _] prims))))"
        ),
        ["(x y rest)", "t", "(t nil)", "(t t)"]
    );
}

#[test]
fn published_reader_registers_list_and_bracket_syntax() {
    let mut interpreter = Interpreter::default();
    eval_bootstrap(&mut interpreter).unwrap();
    assert_eq!(
        results(
            &mut interpreter,
            "(no (no (source (list \"abc\")))) (no (no (get \\( syntax))) (no (no (get \\[ syntax)))"
        ),
        ["t", "t", "t"]
    );
}

#[test]
fn published_reader_registers_quote_character_and_share_syntax() {
    let mut interpreter = Interpreter::default();
    eval_bootstrap(&mut interpreter).unwrap();
    assert_eq!(
        results(
            &mut interpreter,
            "(no (no (get (car \"'\") syntax))) (no (no (get (car \"\\\\\") syntax))) (no (no (get (car \"#\") syntax)))"
        ),
        ["t", "t", "t"]
    );
}

#[test]
fn published_reader_parses_lists_words_abbreviations_and_numbers() {
    let mut interpreter = Interpreter::default();
    eval_bootstrap(&mut interpreter).unwrap();
    assert_eq!(
        results(
            &mut interpreter,
            "(car (rdex (list \"'(a . b)\") i10 nil nil)) (parseword \"car:cdr\" i10) (parseword \"s|source\" i10) (= (parseword \"1/2\" i10) 1/2) (read (list \"(a)\")) (read (list \"'(a b)\"))"
        ),
        [
            "(quote (a . b))",
            "(compose car cdr)",
            "(t s source)",
            "t",
            "(a)",
            "(quote (a b))"
        ]
    );
}

#[test]
fn published_backquote_expands_unquote_splicing_and_nested_forms() {
    let mut interpreter = Interpreter::default();
    eval_bootstrap(&mut interpreter).unwrap();
    assert_eq!(
        results(
            &mut interpreter,
            "(let x 'b `(a ,x c)) (let xs '(b c) `(a ,@xs d)) (let x 'value `(outer `(inner ,x) ,x))"
        ),
        [
            "(a b c)",
            "(a b c d)",
            "(outer (bquote (inner (comma x))) value)"
        ]
    );
    interpreter
        .eval_source("(mac dotted (xs) `(quote (head . ,@xs)))")
        .unwrap();
    assert_eq!(
        results(&mut interpreter, "(dotted (tail))"),
        ["(head . tail)"]
    );
    interpreter
        .eval_source("(mac probe (xs) `(quote (outer `(inner ,@,@xs))))")
        .unwrap();
    assert_eq!(
        results(&mut interpreter, "(probe (a b))"),
        ["(outer (bquote (inner (comma-at a b))))"]
    );
    interpreter
        .eval_source("(def template () `(a b)) (mac identity-probe () (list 'quote (template)))")
        .unwrap();
    assert_eq!(
        results(
            &mut interpreter,
            "(id (template) (template)) (id (identity-probe) (template))"
        ),
        ["t", "t"]
    );
}

#[test]
fn published_printer_core_writes_characters_and_escaped_strings_to_queues() {
    let mut interpreter = Interpreter::default();
    eval_bootstrap(&mut interpreter).unwrap();
    assert_eq!(
        results(
            &mut interpreter,
            r#"(let q (newq) (do (prc \a q) (prstring "bc" q nil nil) (car q))) (no (namedups '(a b)))"#
        ),
        ["(\\a \\\" \\b \\c \\\")", "t"]
    );
}

#[test]
fn published_printer_renders_numbers_and_pairs_to_queues() {
    let mut interpreter = Interpreter::default();
    eval_bootstrap(&mut interpreter).unwrap();
    assert_eq!(
        results(
            &mut interpreter,
            r#"(let q (newq) (do (print 12 q) (car q))) (let q (newq) (do (print '(\a \b) q) (car q)))"#
        ),
        ["(\\1 \\2)", "(\\\" \\a \\b \\\")"]
    );
}

#[test]
fn published_sequence_utilities_slice_compare_and_partially_apply() {
    let mut interpreter = Interpreter::default();
    eval_bootstrap(&mut interpreter).unwrap();
    assert_eq!(
        results(
            &mut interpreter,
            "(drop 2 '(a b c d)) (nth 2 '(a b c)) (cut '(a b c d) 2 3) (<= 1 2 2) (>= 3 2 1) ((part cons 'a) 'b nil)"
        ),
        ["(c d)", "b", "(b c)", "t", "t", "(a b)"]
    );
    for source in ["(drop -1 '(a b))", "(nth 0 '(a b))", "(first 1/2 '(a b))"] {
        assert!(interpreter.eval_source(source).is_err(), "{source}");
        assert_eq!(results(&mut interpreter, "'ok"), ["ok"]);
    }
}

#[test]
fn published_rounding_and_loop_macros_execute() {
    let mut interpreter = Interpreter::default();
    eval_bootstrap(&mut interpreter).unwrap();
    assert_eq!(
        results(
            &mut interpreter,
            "(= (floor 7/2) 3) (= (ceil 7/2) 4) (= (floor -7/2) -4) (= (ceil -7/2) -3) (= (mod 7 3) 1) (let xs '(a b c) (accum collect (whilet x (if xs (let y (car xs) (do (zap cdr xs) y))) (collect x)))) (nof 0 'y)"
        ),
        ["t", "t", "t", "t", "t", "(a b c)", "nil"]
    );
}

#[test]
fn published_exponent_prelude_and_wipe_execute() {
    let mut interpreter = Interpreter::default();
    eval_bootstrap(&mut interpreter).unwrap();
    assert_eq!(
        results(
            &mut interpreter,
            "(= (^w 2 0) 1) (= (clog2 2) 1) (let (x y) '(a b) (do (wipe x y) (list x y)))"
        ),
        ["t", "t", "(nil nil)"]
    );
}

#[test]
fn published_mutation_and_ordering_helpers_execute() {
    let mut interpreter = Interpreter::default();
    eval_bootstrap(&mut interpreter).unwrap();
    assert_eq!(
        results(
            &mut interpreter,
            "(let xs '(a b c) (list (pop xs) xs)) (let xs '(a c) (do (pushnew 'b xs) xs)) (dedup '(a b a c b)) (let xs (sort < '(2 1)) (list (= (car xs) 1) (= (car:cdr xs) 2))) (= (max 2 1) 2) (= (min 2 1) 1) (list (even 2) (odd 3))"
        ),
        [
            "(a (b c))",
            "(b a c)",
            "(a b c)",
            "(t t)",
            "t",
            "t",
            "(t t)"
        ]
    );
}

#[test]
fn published_output_recording_executes_without_an_os_stream() {
    let mut interpreter = Interpreter::default();
    eval_bootstrap(&mut interpreter).unwrap();
    assert_eq!(
        results(
            &mut interpreter,
            "(record (prc \\a)) (record (do (prc \\a) (prc \\b)))"
        ),
        ["(\\a)", "(\\a \\b)"]
    );
}

#[test]
fn published_array_and_table_constructors_execute() {
    let mut interpreter = Interpreter::default();
    eval_bootstrap(&mut interpreter).unwrap();
    assert_eq!(
        results(
            &mut interpreter,
            "(array nil 'default) (array '(0) 'default) (tabref (table '((a . b))) 'a) (tabref (table) 'missing 'fallback) (let a '(lit arr a b) (list (a 1) (a 2))) (let tab (table '((a . b))) (list (tab 'a) (tab 'missing 'fallback)))"
        ),
        [
            "default",
            "(lit arr)",
            "b",
            "fallback",
            "(a b)",
            "(b fallback)"
        ]
    );
}

#[test]
fn table_places_use_the_active_equality_binding() {
    let mut interpreter = Interpreter::default();
    eval_bootstrap(&mut interpreter).unwrap();
    assert_eq!(
        results(
            &mut interpreter,
            "(let tab (table '((a . old))) (dyn = (fn (x y) t) (car (where (tab 'b))))) (let tab (table '((a . old))) (dyn = (fn (x y) nil) (car (where (tab 'a))))) (let tab (table '((a . old))) (do (let loc (dyn = (fn (x y) t) (where (tab 'b))) (xdr (car loc) 'new)) tab)) (let tab (table '((a . old))) (let = (fn (x y) nil) (list (tab 'a) (car (where (tab 'a)))))) (let tab (table) (do (where (tab (do (xdr (cdr tab) '((b . keep))) 'a))) tab))"
        ),
        [
            "(a . old)",
            "(a)",
            "(lit tab (a . new))",
            "(old (a . old))",
            "(lit tab (a) (b . keep))"
        ]
    );
}

#[test]
fn dynamic_virtual_function_registry_overrides_the_global_registry() {
    let mut interpreter = Interpreter::default();
    eval_bootstrap(&mut interpreter).unwrap();
    assert!(
        interpreter
            .eval_source("(dyn virfns nil (1 '(a b)))")
            .is_err()
    );
    assert_eq!(
        results(
            &mut interpreter,
            "(dyn virfns (list (join 'num (fn (f args) ''replacement))) (1 '(a b)))"
        ),
        ["replacement"]
    );
}

#[test]
fn mutable_host_list_walks_are_bounded_and_recoverable() {
    let mut interpreter = Interpreter::default();
    eval_bootstrap(&mut interpreter).unwrap();
    for (source, expected) in [
        (
            "(let tab (table '((a . b))) (do (xdr (cddr tab) (cddr tab)) (where (tab 'missing))))",
            "cyclic table entries",
        ),
        (
            "(let env (join (join 'x 'value) nil) (do (xdr env env) ((list 'lit 'clo env nil 'missing))))",
            "cyclic closure environment",
        ),
        (
            "(let env (join (join 'x 'value) nil) (do (xdr env env) ((list 'lit 'clo env nil (list 'where 'missing)))))",
            "cyclic closure environment",
        ),
        (
            "(let old virfns (let v (join (join 'known 'value) nil) (after (do (xdr v v) (set virfns v) ((list 'lit 'unknown))) (set virfns old))))",
            "cyclic virtual function registry",
        ),
    ] {
        let error = interpreter.eval_source(source).unwrap_err();
        assert!(error.0.contains(expected), "{source}: {error}");
        assert_eq!(
            results(&mut interpreter, "'after-host-cycle"),
            ["after-host-cycle"]
        );
    }

    let mut host = Interpreter::default();
    host.eval_source("(set p (join 'a nil)) (xdr p p) (set body (join 'bquote (join p nil)))")
        .unwrap();
    let error = host
        .eval_source("((join 'lit (join 'clo (join nil (join nil (join body nil))))))")
        .unwrap_err();
    assert!(
        error.0.contains("quasiquote expansion depth limit"),
        "{error}"
    );
    assert_eq!(
        results(&mut host, "'after-quasiquote-cycle"),
        ["after-quasiquote-cycle"]
    );

    std::thread::spawn(|| {
        let deep = (0..720).fold(Value::symbol("leaf"), |value, _| Value::list([value]));
        let body = Value::list([Value::symbol("bquote"), deep]);
        let closure = Value::list([
            Value::symbol("lit"),
            Value::symbol("clo"),
            Value::Nil,
            Value::Nil,
            body,
        ]);
        let mut host = Interpreter::default();
        let error = host.eval_forms(vec![Value::list([closure])]).unwrap_err();
        assert!(
            error.0.contains("quasiquote expansion depth limit"),
            "{error}"
        );
        assert_eq!(
            results(&mut host, "'after-deep-quasiquote"),
            ["after-deep-quasiquote"]
        );
    })
    .join()
    .expect("deep quasiquote test thread should return normally");
}

#[test]
fn published_template_registry_is_initialized() {
    let mut interpreter = Interpreter::default();
    eval_bootstrap(&mut interpreter).unwrap();
    assert_eq!(results(&mut interpreter, "(isa!tab templates)"), ["t"]);
    assert_eq!(
        results(
            &mut interpreter,
            "(let tab (table) (do (set (tab 'a) 'b) (tab 'a))) (do (tem person name 'unknown age 0) (let p (make person name 'alice) (list p!name (= p!age 0))))"
        ),
        ["b", "(alice t)"]
    );
    assert!(interpreter.eval_source("(inst 'missing nil)").is_err());
    assert_eq!(
        results(&mut interpreter, "'after-template-error"),
        ["after-template-error"]
    );
}

#[test]
fn published_stream_buffering_supports_list_backed_sources() {
    let mut interpreter = Interpreter::default();
    eval_bootstrap(&mut interpreter).unwrap();
    assert_eq!(
        results(
            &mut interpreter,
            "(let s (list \"ab\") (list (peek s) (rdc s) (rdc s) (rdc s))) (no (no (digit \\f i16))) (no (digit \\g i16)) (no (no (signc \\+))) (no (no (intrac \\.)))"
        ),
        ["(\\a \\a \\b nil)", "t", "t", "t", "t"]
    );
}

#[test]
fn published_place_and_mutation_library_executes() {
    let mut interpreter = Interpreter::default();
    eval_bootstrap(&mut interpreter).unwrap();
    assert_eq!(
        results(
            &mut interpreter,
            "(tokens '(\\a \\sp \\b \\tab \\c)) (dups '(a b a c b)) (let q (newq) (do (enq 'a q) (enq 'b q) (list (deq q) (deq q)))) (set global-place 'old) (set global-place 'new) global-place (let p (join 'a 'b) (do (set (car p) 'changed) p)) (let x 1 (do (++ x 2) (= x 3))) (let xs '(b c) (do (push 'a xs) xs))"
        ),
        [
            "((\\a) (\\b) (\\c))",
            "(a b)",
            "(a b)",
            "old",
            "new",
            "new",
            "(changed . b)",
            "t",
            "(a b c)"
        ]
    );
}

#[test]
fn published_numeric_tower_executes_arithmetic_and_comparison() {
    let mut interpreter = Interpreter::default();
    eval_bootstrap(&mut interpreter).unwrap();
    assert_eq!(
        results(
            &mut interpreter,
            "(= (+ 1 2) 3) (= (- 3 1) 2) (= (* 2 3) (+ 3 3)) (= (/ 1 2) 1/2) (< 1 2 3) (> 3 2 1) (= (len '(a b c)) 3) (int 1/2) (int 2) (= 2/2 1) (= (+ 1/4 1/4) 2/4) (int 2/2) (= 0/4096 0) (wait (let p (join nil nil) (fn () (if (car p) 'ready (xar p t))))) (< 'a 'b) (< \\a \\b) (sort < '(b a)) (dyn bin< (fn args nil) (< 'a 'b)) (id (charn \\a) (charn \\a))"
        ),
        [
            "t", "t", "t", "t", "t", "t", "t", "nil", "t", "t", "t", "t", "t", "t", "t", "t",
            "(a b)", "nil", "nil"
        ]
    );
}

#[test]
fn published_number_representation_matches_reader_literals() {
    let mut interpreter = Interpreter::default();
    eval_bootstrap(&mut interpreter).unwrap();
    assert_eq!(
        results(
            &mut interpreter,
            "(number 1) (real 1) (= (rpart 1) 1) (= (recip 2) 1/2) (= (abs -2) 2)"
        ),
        ["t", "t", "t", "t", "t"]
    );
}

#[test]
fn published_unary_and_rational_arithmetic_executes() {
    let mut interpreter = Interpreter::default();
    eval_bootstrap(&mut interpreter).unwrap();
    assert_eq!(
        results(
            &mut interpreter,
            "(i+ i2 i1) (i- i2 i1) (i* i2 i2) (i/ i10 i2) (sr+ srone srone) (srrecip (list '+ i2 i1))"
        ),
        [
            "(t t t)",
            "(+ (t))",
            "(t t t t)",
            "((t t t t t) nil)",
            "(+ (t t) (t))",
            "(+ (t) (t t))"
        ]
    );
}

#[test]
fn published_higher_order_and_pattern_library_executes() {
    let mut interpreter = Interpreter::default();
    eval_bootstrap(&mut interpreter).unwrap();
    assert_eq!(
        results(
            &mut interpreter,
            "((compose car cdr) '((a) (b))) (car:cdr '((a) (b))) ((cor atom car) '(x)) (foldl join nil '(a b c)) (match '(a b) '(a b)) (split (is 'stop) '(a b stop c)) (unless nil 'ran)"
        ),
        ["(b)", "(b)", "x", "(c b a)", "t", "((a b) (stop c))", "ran"]
    );
}

#[test]
fn published_evaluator_runs_quotes_conditionals_and_primitives() {
    let mut interpreter = Interpreter::default();
    eval_bootstrap(&mut interpreter).unwrap();
    assert_eq!(
        results(
            &mut interpreter,
            "(bel ''self-hosted nil) (bel '(if nil 'wrong 'right) nil) (bel '(id 'a 'a) '((id lit prim id)))"
        ),
        ["self-hosted", "right", "t"]
    );
    assert_eq!(
        results(
            &mut interpreter,
            "(bel ''ok) (bel 'car) (bel '(car '(a b))) (set host-visible 'before) (bel 'host-visible)"
        ),
        ["ok", "(lit prim car)", "a", "before", "before"]
    );
}

#[test]
fn published_fn_and_do_macros_capture_scope_and_sequence_forms() {
    let mut interpreter = Interpreter::default();
    eval_bootstrap(&mut interpreter).unwrap();
    assert_eq!(
        results(
            &mut interpreter,
            "((fn (x) 'ignored x) 'ok) (((fn (x) (fn () x)) 'captured)) (do (set marker 'first) (set marker 'second) marker) (id (car (uvar)) (car (uvar)))"
        ),
        ["ok", "captured", "second", "t"]
    );
}

#[test]
fn published_core_macros_replace_bootstrap_forms_and_short_circuit() {
    let mut interpreter = Interpreter::default();
    eval_bootstrap(&mut interpreter).unwrap();
    interpreter
        .eval_source("(def multi (x) 'ignored x) (mac first (a b) 'ignored a)")
        .unwrap();
    assert_eq!(
        results(
            &mut interpreter,
            "(multi 'ok) (first 'raw missing) (let x 'value 'ignored x) (or) (or nil 'yes) (or 'first missing) (and) (and 'a 'b) (and nil missing)"
        ),
        ["ok", "raw", "value", "nil", "yes", "first", "t", "b", "nil"]
    );
}

#[test]
fn apply_invokes_macro_literals_in_the_callers_environment() {
    let mut interpreter = Interpreter::default();
    eval_bootstrap(&mut interpreter).unwrap();
    interpreter.eval_source("(mac caller-x args 'x)").unwrap();
    assert_eq!(
        results(
            &mut interpreter,
            "(apply and '(t t)) (apply or '(nil t)) (let x 'captured (apply caller-x nil))"
        ),
        ["t", "t", "captured"]
    );
}

#[test]
fn published_equality_and_type_predicates_cover_structural_values() {
    let mut interpreter = Interpreter::default();
    eval_bootstrap(&mut interpreter).unwrap();
    assert_eq!(
        results(
            &mut interpreter,
            "(=) (= 'a) (= 'a 'a 'a) (= '(a (b) . c) '(a (b) . c)) (= '(a b) '(a c)) (symbol 'x) (symbol '(x)) (pair '(x)) (char 'x) (stream 'x) (proper nil) (proper '(a b)) (proper '(a . b)) (string nil) (string '(a)) ([id _ 'x] 'x)"
        ),
        [
            "t", "t", "t", "t", "nil", "t", "nil", "t", "nil", "nil", "t", "t", "nil", "t", "nil",
            "t"
        ]
    );
}

#[test]
fn published_parameter_macros_and_list_helpers_execute_unchanged() {
    let mut interpreter = Interpreter::default();
    eval_bootstrap(&mut interpreter).unwrap();
    assert_eq!(
        results(
            &mut interpreter,
            "(mem 'b '(a b c)) (in 'b 'a 'b 'c) (caddr '(a b c d)) (case 'b a 'first b 'second 'fallback) (iflet x nil 'yes 'no) (aif '(a b) (car it) 'no) (find atom '((a) b c)) (begins '(a b c) '(a b)) (caris '(a b) 'a) (hug '(a b c d)) (with (x 'a y 'b) (list x y)) (keep atom '((a) b (c) d)) (rem 'b '(a b c b)) (get 'b '((a . first) (b . second))) (put 'b 'new '((a . first) (b . old))) (rev '(a b c)) (snap '(x y) '(a b c d)) (udrop '(x y) '(a b c d)) (idfn 'same) ((is 'a) 'a)"
        ),
        [
            "(b c)",
            "(b c)",
            "c",
            "second",
            "no",
            "a",
            "b",
            "t",
            "t",
            "((a b) (c d))",
            "(a b)",
            "(b d)",
            "(a c)",
            "(b . second)",
            "((b . new) (a . first))",
            "(c b a)",
            "((a b) (c d))",
            "(c d)",
            "same",
            "t"
        ]
    );
}

#[test]
fn published_literal_variable_and_literal_kind_predicates_execute() {
    let mut interpreter = Interpreter::default();
    eval_bootstrap(&mut interpreter).unwrap();
    assert_eq!(
        results(
            &mut interpreter,
            "(literal t) (literal 'x) (literal '(lit example)) (variable 'x) (variable t) (variable (uvar)) ((isa 'clo) (fn (x) x)) ((isa 'mac) mac)"
        ),
        ["(t nil o apply)", "nil", "t", "t", "nil", "t", "t", "t"]
    );
}

#[test]
fn closures_bind_rest_and_dotted_parameters_and_apply_splices_its_final_list() {
    let mut interpreter = Interpreter::default();
    assert_eq!(
        results(
            &mut interpreter,
            "((fn args args) 'a 'b 'c) ((fn args args)) ((fn (first . rest) (join first rest)) 'a 'b 'c) ((fn (first . rest) rest) 'a) (apply join 'a '(b)) (apply (fn (a b c) (join a (join b c))) 'a 'b '(c))"
        ),
        ["(a b c)", "nil", "(a b c)", "nil", "(a . b)", "(a b . c)"]
    );

    for source in [
        "((fn (required . rest) rest))",
        "(fn (x . x) x)",
        "(apply join)",
        "(apply join 'a 'b)",
    ] {
        assert!(interpreter.eval_source(source).is_err(), "{source}");
        assert_eq!(results(&mut interpreter, "'ok"), ["ok"]);
    }
}

#[test]
fn closures_bind_optional_defaults_and_nested_pair_patterns() {
    let mut interpreter = Interpreter::default();
    assert_eq!(
        results(
            &mut interpreter,
            "((fn (x (o y 'fallback)) (join x y)) 'a) ((fn (x (o y x)) y) 'prior) ((fn (x (o y 'fallback)) y) 'a nil) ((fn ((x y)) (join x y)) '(a b)) ((fn ((x . xs)) xs) '(a b c)) ((fn (nil) 'ok) nil)"
        ),
        ["(a . fallback)", "prior", "nil", "(a . b)", "(b c)", "ok"]
    );
    for source in [
        "((fn ((x y)) x) 'atom)",
        "((fn ((x y)) x) '(a))",
        "((fn (x (o y)) y))",
        "(fn ((x x)) x)",
        "(fn ((o)) nil)",
        "(fn (t) t)",
    ] {
        assert!(interpreter.eval_source(source).is_err(), "{source}");
        assert_eq!(results(&mut interpreter, "'ok"), ["ok"]);
    }
}

#[test]
fn parameter_validation_limits_deep_and_excessively_large_patterns() {
    let body = Value::list([Value::symbol("quote"), Value::symbol("ok")]);
    let call = |parameters| {
        Value::list([Value::list([
            Value::symbol("lit"),
            Value::symbol("clo"),
            Value::Nil,
            parameters,
            body.clone(),
        ])])
    };

    let deep = (0..160).fold(Value::symbol("leaf"), |pattern, _| Value::list([pattern]));
    let mut interpreter = Interpreter::default();
    let error = interpreter.eval_forms(vec![call(deep)]).unwrap_err();
    assert!(error.0.contains("parameter pattern depth limit"), "{error}");

    let wide = Value::list((0..1_400).map(|index| Value::symbol(&format!("p{index}"))));
    let error = interpreter.eval_forms(vec![call(wide)]).unwrap_err();
    assert!(
        error.0.contains("parameter validation step limit"),
        "{error}"
    );
    assert_eq!(results(&mut interpreter, "'after-limits"), ["after-limits"]);
}

#[test]
fn parameter_binding_revalidates_mutated_pattern_bounds() {
    let mut interpreter = Interpreter::default();
    eval_bootstrap(&mut interpreter).unwrap();
    let error = interpreter
        .eval_source(
            "(let p (join 'rest nil)
               (let pred (fn (v) (do (xdr p p) t))
                 (let nested (join (list 't 'first pred) p)
                   (let params (list nested)
                     (let arg (join 'value nil)
                       (do (xdr arg arg)
                           ((list 'lit 'clo nil params ''ok) arg)))))))",
        )
        .unwrap_err();
    assert!(error.0.contains("parameter binding"), "{error}");
    assert_eq!(
        results(&mut interpreter, "'after-binding-limit"),
        ["after-binding-limit"]
    );
}

#[test]
fn closures_enforce_typed_parameters() {
    let mut interpreter = Interpreter::default();
    eval_bootstrap(&mut interpreter).unwrap();
    assert_eq!(
        results(
            &mut interpreter,
            "((fn ((t x atom)) x) 'accepted) ((fn (((t x atom) y)) (list x y)) '(a b)) ((fn args|proper args) 'a 'b) ((fn (x . rest|proper) rest) 'a 'b 'c)"
        ),
        ["accepted", "(a b)", "(a b)", "(b c)"]
    );
    for source in [
        "((fn ((t x atom)) x) '(rejected))",
        "((fn ((t x)) x) 'a)",
        "((fn ((t x atom extra)) x) 'a)",
    ] {
        assert!(interpreter.eval_source(source).is_err(), "{source}");
        assert_eq!(results(&mut interpreter, "'ok"), ["ok"]);
    }
}

#[test]
fn typed_parameters_honor_redefined_and_lexically_shadowed_predicates() {
    let mut interpreter = Interpreter::default();
    eval_bootstrap(&mut interpreter).unwrap();
    interpreter.eval_source("(set whole (fn (x) nil))").unwrap();
    assert!(interpreter.eval_source("((fn ((t x whole)) x) 1)").is_err());

    let mut interpreter = Interpreter::default();
    eval_bootstrap(&mut interpreter).unwrap();
    assert!(
        interpreter
            .eval_source("((fn (whole) ((fn ((t x whole)) x) 1)) (fn (x) nil))")
            .is_err()
    );
    assert_eq!(results(&mut interpreter, "'after-shadow"), ["after-shadow"]);
}

#[test]
fn typed_numeric_shortcuts_honor_predicate_dependency_rebindings() {
    let mut interpreter = Interpreter::default();
    eval_bootstrap(&mut interpreter).unwrap();
    assert!(
        interpreter
            .eval_source("(dyn int (fn (x) nil) ((fn ((t x whole)) 'accepted) 1))")
            .is_err()
    );

    let mut interpreter = Interpreter::default();
    eval_bootstrap(&mut interpreter).unwrap();
    interpreter.eval_source("(set int (fn (x) nil))").unwrap();
    assert!(
        interpreter
            .eval_source("((fn ((t x whole)) 'accepted) 1)")
            .is_err()
    );

    let mut interpreter = Interpreter::default();
    eval_bootstrap(&mut interpreter).unwrap();
    interpreter
        .eval_source("(xar (cdr (cdr (cdr (cdr whole)))) nil)")
        .unwrap();
    assert_eq!(results(&mut interpreter, "(whole 1)"), ["nil"]);
    assert!(
        interpreter
            .eval_source("((fn ((t x whole)) 'accepted) 1)")
            .is_err()
    );

    let mut interpreter = Interpreter::default();
    eval_bootstrap(&mut interpreter).unwrap();
    let noncanonical_zero = "(litnum srone (list '- i0 i1))";
    assert_eq!(
        results(&mut interpreter, &format!("(whole {noncanonical_zero})")),
        ["nil"]
    );
    assert!(
        interpreter
            .eval_source(&format!(
                "((fn ((t x whole)) 'accepted) {noncanonical_zero})"
            ))
            .is_err()
    );

    let mut interpreter = Interpreter::default();
    eval_bootstrap(&mut interpreter).unwrap();
    let negative_zero = "(litnum (list '- i0 i1) srzero)";
    assert_eq!(
        results(
            &mut interpreter,
            &format!(
                "(let n {negative_zero} (whole n)) (let n {negative_zero} ((fn ((t x whole)) 'accepted) n)) (let n {negative_zero} (pint n))"
            )
        ),
        ["t", "accepted", "nil"]
    );
    assert!(
        interpreter
            .eval_source(&format!(
                "(let n {negative_zero} ((fn ((t x pint)) 'accepted) n))"
            ))
            .is_err()
    );

    let mut interpreter = Interpreter::default();
    eval_bootstrap(&mut interpreter).unwrap();
    let non_t_digits = "(litnum (list '+ '(a) i1) srzero)";
    assert_eq!(
        results(
            &mut interpreter,
            &format!(
                "(let n {non_t_digits} (whole n)) (let n {non_t_digits} ((fn ((t x whole)) 'accepted) n))"
            )
        ),
        ["t", "accepted"]
    );
}

#[test]
fn pairs_identity_and_function_literals_match_supported_bel() {
    let mut interpreter = Interpreter::default();
    assert_eq!(
        results(
            &mut interpreter,
            "(car nil) (cdr nil) (type nil) (type car) (car car) (cdr car) (id '(a) '(a)) ((fn (x) (id x x)) '(a)) (id 'x 'x) (join) (car) (id)"
        ),
        [
            "nil",
            "nil",
            "symbol",
            "pair",
            "lit",
            "(prim car)",
            "nil",
            "t",
            "t",
            "(nil)",
            "nil",
            "t"
        ]
    );
    assert_eq!(
        results(
            &mut interpreter,
            "(cdr '(a b . c)) (car '(a . b)) (type (fn (x) x))"
        ),
        ["(b . c)", "a", "pair"]
    );
}

#[test]
fn characters_and_symbol_names_round_trip() {
    let mut interpreter = Interpreter::default();
    assert_eq!(
        results(
            &mut interpreter,
            r"\a \sp \lf (type \a) (nom 'abc) (sym (nom 'abc))"
        ),
        ["\\a", "\\sp", "\\lf", "char", "(\\a \\b \\c)", "abc"]
    );
    for source in [r"(nom '(a))", r"(sym '(a))", r"\unknown"] {
        assert!(interpreter.eval_source(source).is_err(), "{source}");
        assert_eq!(results(&mut interpreter, "'ok"), ["ok"]);
    }
}

#[test]
fn strings_read_as_self_evaluating_character_lists() {
    let mut interpreter = Interpreter::default();
    eval_bootstrap(&mut interpreter).unwrap();
    assert_eq!(
        results(
            &mut interpreter,
            r#""abc" "a\"b" (string "abc") (sym "name") (nom (sym "name"))"#
        ),
        [
            "(\\a \\b \\c)",
            "(\\a \\\" \\b)",
            "t",
            "name",
            "(\\n \\a \\m \\e)"
        ]
    );
    assert!(read("\"unfinished").unwrap_err().incomplete);
}

#[test]
fn pair_mutation_preserves_identity_and_supports_cycles() {
    let mut interpreter = Interpreter::default();
    eval_bootstrap(&mut interpreter).unwrap();
    assert_eq!(
        results(
            &mut interpreter,
            "(let p (join 'a 'b) (do (xar p 'c) p)) (let p (join 'a 'b) (do (xdr p 'c) p)) (let p (join 'a nil) (do (xdr p p) (car (cdr p)))) (xar (join 'old 'tail) 'new)"
        ),
        ["(c . b)", "(a . c)", "a", "new"]
    );
    for source in ["(xar nil 'x)", "(xdr 'atom nil)"] {
        assert!(interpreter.eval_source(source).is_err(), "{source}");
        assert_eq!(results(&mut interpreter, "'ok"), ["ok"]);
    }
}

#[test]
fn dynamic_errors_escape_through_eif_and_after_always_cleans_up() {
    let mut interpreter = Interpreter::default();
    eval_bootstrap(&mut interpreter).unwrap();
    assert_eq!(
        results(
            &mut interpreter,
            "(eif problem 'value 'failed problem) (eif problem (err 'boom) problem 'unused) (onerr 'caught (err 'boom)) (onerr 'caught (car 'atom)) (onerr 'caught missing) (onerr 'caught ((fn (x) x))) (safe (err 'boom)) (safe 'ok) (dyn x 'dynamic x) (dyn scope 'dynamic scope) (dyn err (fn (x) 'old) (do (set err (fn (x) 'new)) (car 'atom))) (dyn err (fn (e) x) (dyn x 'inner (car 'atom))) (ccc (fn (k) (k 'escaped) 'later)) (ccc (fn (k) 'normal)) (let p (join nil nil) (do (safe (after (err 'boom) (xar p 'clean))) (car p))) (ccc (fn (outer) (ccc (fn (inner) (after (outer 'body) (inner 'cleanup))))))"
        ),
        [
            "value", "boom", "caught", "caught", "caught", "caught", "nil", "ok", "dynamic",
            "dynamic", "new", "inner", "escaped", "normal", "clean", "cleanup"
        ]
    );
    assert!(interpreter.eval_source("(err 'outside)").is_err());
    assert_eq!(results(&mut interpreter, "'after-error"), ["after-error"]);

    interpreter
        .eval_source("(def forever (x) (forever x))")
        .unwrap();
    let error = interpreter
        .eval_source("(onerr 'caught (forever nil))")
        .unwrap_err();
    assert!(error.0.contains("limit reached"), "{error}");
}

#[test]
fn scope_lookup_prefers_dynamic_lexical_and_global_bindings_before_environment() {
    let mut interpreter = Interpreter::default();
    assert_eq!(
        results(&mut interpreter, "scope ((fn (x) scope) 'value)"),
        ["nil", "((x . value))"]
    );
    assert_eq!(
        results(
            &mut interpreter,
            "(set scope 'global) scope ((fn (scope) scope) 'lexical) (dyn scope 'dynamic ((fn (scope) scope) 'lexical))"
        ),
        ["global", "global", "lexical", "dynamic"]
    );
}

#[test]
fn dynamic_scope_does_not_traverse_a_lower_priority_cyclic_environment() {
    let mut interpreter = Interpreter::default();
    eval_bootstrap(&mut interpreter).unwrap();
    assert_eq!(
        results(
            &mut interpreter,
            "(let env (join (join 'z 'value) nil) (do (xdr env env) ((list 'lit 'clo env nil (list 'dyn 'scope (list 'quote 'dynamic) 'scope)))))"
        ),
        ["dynamic"]
    );
    assert_eq!(
        results(&mut interpreter, "'after-cyclic-scope"),
        ["after-cyclic-scope"]
    );
}

#[test]
fn after_reserves_bounded_fuel_for_cleanup_after_step_exhaustion() {
    let mut interpreter = Interpreter::default();
    eval_bootstrap(&mut interpreter).unwrap();
    interpreter
        .eval_source(
            "(def burn (xs) (if xs (do (burn (cdr xs)) (burn (cdr xs))) nil))
             (set cleanup-pair (join nil nil))",
        )
        .unwrap();
    let input = format!(
        "(after (burn '({})) (xar cleanup-pair 'clean))",
        vec!["n"; 24].join(" ")
    );
    let error = interpreter.eval_source(&input).unwrap_err();
    assert!(error.0.contains("evaluation step limit"), "{error}");
    assert_eq!(results(&mut interpreter, "(car cleanup-pair)"), ["clean"]);
}

#[test]
fn dynamic_places_mutate_the_innermost_binding() {
    let mut interpreter = Interpreter::default();
    eval_bootstrap(&mut interpreter).unwrap();
    assert_eq!(
        results(
            &mut interpreter,
            "(dyn x 'before (where x)) (dyn dynamic-target 'before (do (set dynamic-target 'after) dynamic-target)) (bind dynamic-target 'before (do (set dynamic-target 'after) dynamic-target)) (dyn dynamic-target 'outer (list (dyn dynamic-target 'inner (do (set dynamic-target 'changed) dynamic-target)) dynamic-target))"
        ),
        ["((x . before) d)", "after", "after", "(changed outer)"]
    );
    assert!(interpreter.eval_source("dynamic-target").is_err());
    assert_eq!(
        results(&mut interpreter, "'after-dynamic-place"),
        ["after-dynamic-place"]
    );
}

#[test]
fn conditional_is_variadic_and_short_circuits() {
    let mut interpreter = Interpreter::default();
    assert_eq!(
        results(
            &mut interpreter,
            "(if) (if 'a) (if nil 'a nil 'b 'else) (if t 'yes missing) (if nil missing) (if 'nil 'yes 'no)"
        ),
        ["nil", "a", "else", "yes", "nil", "no"]
    );
}

#[test]
fn lexical_closures_can_call_global_definitions_and_recurse() {
    let mut interpreter = Interpreter::default();
    assert_eq!(
        results(&mut interpreter, "(((fn (x) (fn (y) (join x y))) 'a) 'b)"),
        ["(a . b)"]
    );
    interpreter.eval_source("(def first (x) (second x)) (def second (x) x) (def last (xs) (if (cdr xs) (last (cdr xs)) (car xs)))").unwrap();
    assert_eq!(
        results(
            &mut interpreter,
            "(first 'ok) (last '(a b c)) (apply car '((a b)))"
        ),
        ["ok", "c", "a"]
    );
}

#[test]
fn bootstrap_fallbacks_yield_to_ordinary_function_bindings() {
    let mut interpreter = Interpreter::default();
    assert_eq!(
        results(
            &mut interpreter,
            "((fn (set) (set '(a b))) car) ((fn (mac) (mac '(a b))) cdr)"
        ),
        ["a", "(b)"]
    );
}

#[test]
fn bootstrap_macros_receive_raw_forms_and_expand_in_the_callers_scope() {
    let mut interpreter = Interpreter::default();
    interpreter
        .eval_source("(mac first (a b) a) (mac delay (expr) `(fn () ,expr))")
        .unwrap();
    assert_eq!(
        results(
            &mut interpreter,
            "(first 'ok missing) (((fn (x) (delay x)) 'captured))"
        ),
        ["ok", "captured"]
    );
}

#[test]
fn set_accepts_multiple_name_value_pairs() {
    let mut interpreter = Interpreter::default();
    eval_bootstrap(&mut interpreter).unwrap();
    assert_eq!(
        results(
            &mut interpreter,
            "(set left 'a right left) (list left right) (set) (set lone) lone"
        ),
        ["a", "(a a)", "nil", "t", "t"]
    );
    assert!(interpreter.eval_source("(set nil 'x)").is_err());
    assert_eq!(results(&mut interpreter, "'ok"), ["ok"]);
}

#[test]
fn where_exposes_mutable_lexical_global_and_pair_locations() {
    let mut interpreter = Interpreter::default();
    eval_bootstrap(&mut interpreter).unwrap();
    assert_eq!(
        results(
            &mut interpreter,
            "(set target 'before) (let local 'before (do (let (cell loc) (where local) ((case loc a xar d xdr) cell 'after)) local)) (do (let (cell loc) (where target) ((case loc a xar d xdr) cell 'after)) target) (let p (join 'a 'b) (do (let (cell loc) (where (car p)) ((case loc a xar d xdr) cell 'changed)) p)) (let p (join 'a 'b) (do (let car cdr (set (car p) 'changed)) p)) (do (let (cell loc) (where created t) ((case loc a xar d xdr) cell 'new)) created)"
        ),
        [
            "before",
            "after",
            "after",
            "(changed . b)",
            "(a . changed)",
            "new"
        ]
    );
    interpreter
        .eval_source("(mac head (p) (list 'car p))")
        .unwrap();
    assert_eq!(
        results(
            &mut interpreter,
            "(let p (list 'old) (do (set (head p) 'new) p))"
        ),
        ["(new)"]
    );
    for source in [
        "(where missing)",
        "(where (car 'atom))",
        "(where (list 'a))",
    ] {
        assert!(interpreter.eval_source(source).is_err(), "{source}");
        assert_eq!(results(&mut interpreter, "'ok"), ["ok"]);
    }
}

#[test]
fn global_lookup_follows_the_live_first_binding() {
    let mut interpreter = Interpreter::default();
    eval_bootstrap(&mut interpreter).unwrap();
    assert_eq!(
        results(
            &mut interpreter,
            "(set foo 'old) (do (xdr globe (join (join 'foo 'new) (cdr globe))) (list foo (bel 'foo))) (set foo 'updated) (list foo (bel 'foo))"
        ),
        ["old", "(new new)", "updated", "(updated updated)"]
    );
}

#[test]
fn global_insertion_repairs_a_replaced_live_list_node() {
    let mut interpreter = Interpreter::default();
    eval_bootstrap(&mut interpreter).unwrap();
    assert_eq!(
        results(
            &mut interpreter,
            "(let p (drop 9 globe) (do (xdr p (join (car (cdr p)) (cdr (cdr p)))) 'copied)) (set fresh 'new) fresh"
        ),
        ["copied", "new", "new"]
    );
}

#[test]
fn host_backquote_handles_nesting_splicing_and_dotted_tails() {
    let mut interpreter = Interpreter::default();
    assert_eq!(
        results(
            &mut interpreter,
            "((fn (x xs) `(a ,x ,@xs . z)) 'b '(c d)) ((fn (x) `(outer `(inner ,x) ,x)) 'value) ((fn (xs) `(head . ,@xs)) '(tail)) ((fn (xs ys) `(,@xs . ,@ys)) '(a b) '(tail))"
        ),
        [
            "(a b c d . z)",
            "(outer (bquote (inner (comma x))) value)",
            "(head . tail)",
            "(a b . tail)"
        ]
    );
    interpreter.eval_source("(set marker 'initial)").unwrap();
    assert_eq!(
        results(
            &mut interpreter,
            "`(,(if (set marker 'head) 'h) . ,@(if (set marker 'tail) '(d))) marker"
        ),
        ["(h . d)", "tail"]
    );
    interpreter.eval_source("(set xs '(a b))").unwrap();
    assert_eq!(
        results(
            &mut interpreter,
            "`(,@xs . ,@(if (xar xs 'changed) '(tail)))"
        ),
        ["(changed b . tail)"]
    );
    interpreter.eval_source("(set xs '(a b))").unwrap();
    assert_eq!(
        results(&mut interpreter, "`(,@xs ,(if (xar xs 'changed) 'tail))"),
        ["(changed b tail)"]
    );
    for source in [
        "`,@'(a)",
        "`(a ,@'b)",
        "`(a ,@(join 'b 'c))",
        "`(a . ,@nil)",
        "`(a . ,@'(b c))",
    ] {
        assert!(interpreter.eval_source(source).is_err(), "{source}");
        assert_eq!(results(&mut interpreter, "'ok"), ["ok"]);
    }
}

#[test]
fn ordinary_multi_form_sources_install_backquote_in_source_order() {
    let mut interpreter = Interpreter::default();
    eval_bootstrap(&mut interpreter).unwrap();
    interpreter
        .eval_source("(mac probe () (bquote foo))")
        .unwrap();
    let values = interpreter
        .eval_source("(mac bquote (x) (list 'quote (list 'quote 'custom))) (bquote foo) (probe)")
        .unwrap();
    assert_eq!(
        values[values.len() - 2..]
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        ["(quote custom)", "custom"]
    );

    let mut interpreter = Interpreter::default();
    eval_bootstrap(&mut interpreter).unwrap();
    assert!(
        interpreter
            .eval_source("(mac bquote (x) (list 'quote 'custom)) missing")
            .is_err()
    );
    assert_eq!(results(&mut interpreter, "(bquote foo)"), ["custom"]);

    let mut interpreter = Interpreter::default();
    eval_bootstrap(&mut interpreter).unwrap();
    interpreter
        .eval_source(
            "(mac bquote (x) (if (id x 'outer) (list 'bquote 'inner) (list 'quote 'custom)))",
        )
        .unwrap();
    assert_eq!(
        results(&mut interpreter, "(bquote outer) (bquote inner)"),
        ["custom", "custom"]
    );
}

#[test]
fn reader_errors_distinguish_incomplete_and_reject_unsupported_syntax() {
    for source in ["(", "[", "'", "`", ",", ",@", "(a .", "\\"] {
        assert!(read(source).unwrap_err().incomplete);
    }
    for source in [
        ")",
        "]",
        "(. a)",
        "(a . b c)",
        "1/0",
        "a.",
        "a!!b",
        "a.!b",
        "|atom",
        "x|",
        "x|atom|id",
    ] {
        assert!(!read(source).unwrap_err().incomplete, "{source}");
    }
    assert_eq!(
        read("; comment\n'(a ; middle\n b)").unwrap()[0].to_string(),
        "(quote (a b))"
    );
    assert_eq!(
        read("`(a ,b ,@c)").unwrap()[0].to_string(),
        "(bquote (a (comma b) (comma-at c)))"
    );
    assert_eq!(
        read("[id _ (car xs)]").unwrap()[0].to_string(),
        "(fn (_) (id _ (car xs)))"
    );
    assert_eq!(
        read("car:cddr ~no ~~atom s|source")
            .unwrap()
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        [
            "(compose car cddr)",
            "(compose no no)",
            "(compose no (compose no atom))",
            "(t s source)"
        ]
    );
    assert_eq!(
        read("0 1 -2 1/2")
            .unwrap()
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        [
            "(lit num (+ nil (t)) (+ nil (t)))",
            "(lit num (+ (t) (t)) (+ nil (t)))",
            "(lit num (- (t t) (t)) (+ nil (t)))",
            "(lit num (+ (t) (t t)) (+ nil (t)))"
        ]
    );
    assert_eq!(
        read("templates.name isa!arr .first foo!bar")
            .unwrap()
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        [
            "(templates name)",
            "(isa (quote arr))",
            "(upon first)",
            "(foo (quote bar))"
        ]
    );
    assert_eq!(
        read("1.2.3").unwrap()[0].to_string(),
        read("(1 2 3)").unwrap()[0].to_string()
    );

    for source in [
        format!("{}a", "~".repeat(129)),
        format!("a{}", ":a".repeat(1_025)),
        format!("a{}", ".a".repeat(1_025)),
    ] {
        let error = read(&source).unwrap_err();
        assert!(!error.incomplete, "{source}");
        assert!(error.message.contains("abbreviation exceeds"), "{error}");
    }

    let expanded_numbers = format!("'({})", vec!["4096"; 100].join(" "));
    let error = read(&expanded_numbers).unwrap_err();
    assert!(error.message.contains("reader allocation limit"), "{error}");
    assert!(read("4096").is_ok());

    let large_string = format!("\"{}\"", "a".repeat(60_000));
    let error = read(&large_string).unwrap_err();
    assert!(error.message.contains("reader allocation limit"), "{error}");
    assert!(read("\"small\"").is_ok());
}

#[test]
fn numeric_headed_intrasymbols_parse_before_numeric_literals() {
    let mut interpreter = Interpreter::default();
    eval_bootstrap(&mut interpreter).unwrap();
    assert_eq!(
        results(
            &mut interpreter,
            "(let xs '(a b) 1.xs) (= 1.5 3/2) (= .5 1/2) (= 1.5/0.5 3)"
        ),
        ["a", "t", "t", "t"]
    );
}

#[test]
fn errors_recover_and_resource_limits_leave_interpreter_usable() {
    let mut interpreter = Interpreter::default();
    for source in [
        "missing",
        "(car 'symbol)",
        "(join 'a 'b 'c)",
        "((fn (x) x))",
        "(fn (x x) x)",
        "(quote)",
        "(def)",
        "('x)",
    ] {
        assert!(interpreter.eval_source(source).is_err(), "{source}");
        assert_eq!(results(&mut interpreter, "'ok"), ["ok"]);
    }
    interpreter
        .eval_source("(def forever (x) (forever x))")
        .unwrap();
    assert!(
        interpreter
            .eval_source("(forever nil)")
            .unwrap_err()
            .0
            .contains("limit")
    );
    assert_eq!(results(&mut interpreter, "'ok"), ["ok"]);
    assert!(!read(&"(".repeat(200)).unwrap_err().incomplete);
}
