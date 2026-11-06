//! Check literate examples against extracted definitions on the Rust host.
use std::{env, fs, process::ExitCode};

use bel_rust_repl::{Interpreter, Value, read};

fn fields(mut value: Value) -> Result<Vec<Value>, Box<dyn std::error::Error>> {
    let mut fields = Vec::new();
    while let Value::Pair(pair) = value {
        let (head, tail) = pair.borrow().clone();
        fields.push(head);
        value = tail;
    }
    if !value.is_nil() || fields.len() != 3 {
        return Err("example must be (name expression expected-value)".into());
    }
    Ok(fields)
}

fn check() -> Result<(), Box<dyn std::error::Error>> {
    let paths: Vec<_> = env::args_os().skip(1).collect();
    if paths.len() != 2 {
        return Err("usage: rbel-check-examples DEFINITIONS EXAMPLES".into());
    }
    let definitions = fs::read_to_string(&paths[0])?;
    let examples = read(&fs::read_to_string(&paths[1])?)?;
    if examples.is_empty() {
        return Err("no examples".into());
    }
    let count = examples.len();
    for row in examples {
        let row = fields(row)?;
        let name = row[0].name().ok_or("example name must be a symbol")?;
        // No full bootstrap: later fragments must not mask missing definitions.
        let mut interpreter = Interpreter::default();
        interpreter
            .eval_source(&definitions)
            .map_err(|error| format!("{name}: definitions: {error}"))?;
        let actual = interpreter
            .eval_forms(vec![row[1].clone()])
            .map_err(|error| format!("{name}: {error}"))?
            .pop()
            .ok_or("example returned no value")?
            .render()
            .map_err(|error| format!("{name}: result: {error}"))?;
        // Expected values are data; never execute them or use unchecked Display.
        let expected = row[2]
            .render()
            .map_err(|error| format!("{name}: expected value: {error}"))?;
        if actual != expected {
            return Err(format!("{name}: expected {expected}, got {actual}").into());
        }
        println!("ok {name}");
    }
    println!("{count} literate examples passed on the Rust host");
    Ok(())
}

fn main() -> ExitCode {
    match check() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}
