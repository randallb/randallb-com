//! Deterministic evaluator and published-helper comparisons (test-only).
use crate::{Error, Interpreter, Value, eval_bootstrap, read};

#[derive(Clone, Copy, Debug)]
enum Engine {
    Host,
    Published,
}

struct Fixture {
    name: &'static str,
    setup: &'static str,
    corpus: &'static str,
}

fn fields(value: &Value) -> Vec<Value> {
    let mut value = value.clone();
    let mut fields = Vec::new();
    while let Value::Pair(pair) = value {
        let (head, tail) = pair.borrow().clone();
        fields.push(head);
        value = tail;
    }
    assert!(value.is_nil(), "fixture rows must be proper lists");
    fields
}

fn quoted(value: Value) -> Value {
    Value::list([Value::symbol("quote"), value])
}

fn evaluate(interpreter: &mut Interpreter, expression: Value) -> Result<Value, Error> {
    Ok(interpreter.eval_forms(vec![expression])?.pop().unwrap())
}

fn reference(
    interpreter: &mut Interpreter,
    expression: Value,
    recipe: Option<Value>,
) -> Result<Value, Error> {
    let Some(recipe) = recipe else {
        return evaluate(
            interpreter,
            Value::list([Value::symbol("bel"), quoted(expression)]),
        );
    };
    let recipe = fields(&recipe);
    assert_eq!(
        recipe.len(),
        2,
        "expected (source expression) or (expand expression)"
    );
    let value = evaluate(interpreter, recipe[1].clone())?;
    match recipe[0].name() {
        Some("source") => Ok(value),
        Some("expand") => evaluate(interpreter, value),
        _ => panic!("unknown reference recipe"),
    }
}

fn observations(engine: Engine, fixture: &Fixture) -> Vec<String> {
    // Neither mutable syntax nor globals may leak between the two executions.
    let steps = read(fixture.corpus).expect("valid corpus syntax");
    assert!(!steps.is_empty(), "empty corpus");
    let mut interpreter = Interpreter::default();
    if matches!(engine, Engine::Published) {
        interpreter.test_eval_depth_limit = Some(4_096);
    }
    eval_bootstrap(&mut interpreter).expect("published bootstrap must load");
    // Setup is deliberately shared host infrastructure, like the bootstrap.
    interpreter
        .eval_source(fixture.setup)
        .expect("fixture setup");
    steps
        .iter()
        .enumerate()
        .map(|(index, step)| {
            let row = fields(step);
            assert!(
                matches!(row.len(), 2 | 3),
                "expected (expression [reference-recipe] result)"
            );
            let expression = row[0].clone();
            let expected = row
                .last()
                .unwrap()
                .render()
                .expect("finite expected result");
            let context = format!(
                "{}, {engine:?}, step {}: {}",
                fixture.name,
                index + 1,
                expression.render().expect("finite expression")
            );
            let value = match engine {
                Engine::Host => evaluate(&mut interpreter, expression),
                Engine::Published => reference(
                    &mut interpreter,
                    expression,
                    (row.len() == 3).then(|| row[1].clone()),
                ),
            }
            .unwrap_or_else(|error| panic!("{context}: unexpected evaluation error: {error}"));
            let actual = value
                .render()
                .unwrap_or_else(|error| panic!("{context}: {error}"));
            assert_eq!(actual, expected, "{context}");
            actual
        })
        .collect()
}

fn compare(fixture: Fixture) {
    let host = observations(Engine::Host, &fixture);
    // Only the reference receives a larger stack/depth allowance. The host under
    // test runs on the ordinary test thread with production resource limits.
    let published = std::thread::Builder::new()
        .name(format!("published-{}", fixture.name))
        .stack_size(64 * 1024 * 1024)
        .spawn(move || observations(Engine::Published, &fixture))
        .expect("start published evaluator")
        .join()
        .unwrap_or_else(|panic| std::panic::resume_unwind(panic));
    assert_eq!(host, published, "evaluator observations differ");
}

macro_rules! comparison {
    ($name:ident, $setup:expr) => {
        #[test]
        fn $name() {
            compare(Fixture {
                name: stringify!($name),
                setup: $setup,
                corpus: include_str!(concat!("../tests/comparison/", stringify!($name), ".bel")),
            });
        }
    };
}

comparison!(values_and_calls, "(set saved '(left right))");
comparison!(parameters, "");
comparison!(dynamic_scope, "(set scope-value 'global)");
comparison!(table_mutation, "(set tab (table '((key . before))))");
comparison!(backquote, "(set marker 'start) (set xs '(a b))");
comparison!(errors_and_cleanup, "(set cleanup-marker '(before))");

#[test]
#[should_panic(expected = "Published, step 1")]
fn comparison_really_uses_the_published_evaluator() {
    compare(Fixture {
        name: "route-probe",
        setup: "(set bel (fn (e) 'wrong))",
        corpus: "('expected expected)",
    });
}

#[test]
#[should_panic(expected = "Host, step 1")]
fn comparison_rejects_a_shared_wrong_result() {
    compare(Fixture {
        name: "wrong-expectation",
        setup: "",
        corpus: "('actual expected)",
    });
}

#[test]
fn backquote_preserves_template_identity_on_both_routes() {
    for engine in [Engine::Host, Engine::Published] {
        let mut interpreter = Interpreter::default();
        eval_bootstrap(&mut interpreter).unwrap();
        let template = read("(a b)").unwrap().pop().unwrap();
        let expression = Value::list([Value::symbol("bquote"), template.clone()]);
        let result = match engine {
            Engine::Host => evaluate(&mut interpreter, expression).unwrap(),
            Engine::Published => {
                let expander = interpreter
                    .eval_source("(caddr bquote)")
                    .unwrap()
                    .pop()
                    .unwrap();
                let expansion = evaluate(
                    &mut interpreter,
                    Value::list([expander, quoted(template.clone())]),
                )
                .unwrap();
                evaluate(&mut interpreter, expansion).unwrap()
            }
        };
        assert!(
            result.identical(&template),
            "{engine:?}: unchanged template was copied"
        );
    }
}
