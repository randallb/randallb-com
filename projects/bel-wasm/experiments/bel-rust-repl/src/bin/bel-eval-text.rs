//! Evaluate Bel files on the bootstrap host and print the final character tree.
//! This is generic batch/output support, with no compiler or Wasm semantics.
use std::{
    env, fs,
    io::{self, Write},
    process::ExitCode,
};

use bel_rust_repl::{Interpreter, Value, eval_bootstrap, read};

fn batch_limits(
    paths: &mut Vec<std::ffi::OsString>,
) -> Result<(usize, Option<usize>), Box<dyn std::error::Error>> {
    // Explicit batch-only allowance; the default and REPL bounds stay unchanged.
    let max_bytes = if paths.first().is_some_and(|path| path == "--max-text-bytes") {
        if paths.len() < 3 {
            return Err("usage: --max-text-bytes BYTES [--call ...] FILE ...".into());
        }
        let value = paths[1]
            .to_str()
            .ok_or("invalid byte limit")?
            .parse::<usize>()?;
        if !(64 * 1024..=16 * 1024 * 1024).contains(&value) {
            return Err("text byte limit must be between 65536 and 16777216".into());
        }
        paths.drain(..2);
        value
    } else {
        64 * 1024
    };
    let depth_limit = if paths.first().is_some_and(|path| path == "--max-eval-depth") {
        if paths.len() < 3 {
            return Err("usage: --max-eval-depth DEPTH [--call ...] FILE ...".into());
        }
        let value = paths[1]
            .to_str()
            .ok_or("invalid depth limit")?
            .parse::<usize>()?;
        paths.drain(..2);
        Some(value)
    } else {
        None
    };
    Ok((max_bytes, depth_limit))
}

fn evaluate() -> Result<String, Box<dyn std::error::Error>> {
    let mut paths: Vec<_> = env::args_os().skip(1).collect();
    let (max_bytes, depth_limit) = batch_limits(&mut paths)?;
    let call = if paths.first().is_some_and(|path| path == "--call") {
        if paths.len() < 4 {
            return Err("usage: bel-eval-text --call FUNCTION DATA FILE [FILE ...]".into());
        }
        let arguments: Vec<_> = paths.drain(..3).collect();
        Some((
            arguments[1]
                .clone()
                .into_string()
                .map_err(|_| "invalid function name")?,
            arguments[2].clone(),
        ))
    } else {
        None
    };
    if paths.is_empty() {
        return Err("usage: bel-eval-text FILE [FILE ...]".into());
    }
    let mut interpreter = Interpreter::default();
    if let Some(limit) = depth_limit {
        interpreter.set_batch_eval_depth_limit(limit)?;
    }
    eval_bootstrap(&mut interpreter)?;
    let mut result = Value::Nil;
    for path in paths {
        let source = fs::read_to_string(&path)?;
        let values = interpreter
            .eval_source(&source)
            .map_err(|error| format!("{}: {error}", path.to_string_lossy()))?;
        result = values
            .into_iter()
            .last()
            .ok_or("expected a final expression")?;
    }
    if let Some((name, path)) = call {
        // Parse the complete data file without evaluating or interpolating it.
        // The called Bel function receives the list of all source forms.
        let forms = read(&fs::read_to_string(&path)?)?;
        let quoted = Value::list([Value::symbol("quote"), Value::list(forms)]);
        result = interpreter
            .eval_forms(vec![Value::list([Value::symbol(&name), quoted])])?
            .pop()
            .ok_or("expected a call result")?;
    }
    // A tree of characters avoids forcing Bel to concatenate long lists under
    // the bootstrap evaluator's depth limit. Traverse iteratively and bound
    // total work as well as output, including cyclic or highly shared pairs.
    let mut text = String::new();
    let mut pending = vec![result];
    let mut steps = 0;
    while let Some(value) = pending.pop() {
        steps += 1;
        if steps > max_bytes * 4 {
            return Err("text traversal limit reached".into());
        }
        match value {
            Value::Nil => {}
            Value::Char(character) => {
                if text.len() + character.len_utf8() > max_bytes {
                    let message = if max_bytes == 64 * 1024 {
                        "text output limit reached (64 KiB)".to_string()
                    } else {
                        format!("text output limit reached ({max_bytes} bytes)")
                    };
                    return Err(message.into());
                }
                text.push(character);
            }
            Value::Pair(pair) => {
                let (head, tail) = pair.borrow().clone();
                pending.push(tail);
                pending.push(head);
            }
            Value::Symbol(_) => return Err("final result must be a character tree".into()),
        }
    }
    Ok(text)
}

fn main() -> ExitCode {
    // Only the explicit depth option needs a larger native stack.
    let result = if env::args_os().any(|arg| arg == "--max-eval-depth") {
        match std::thread::Builder::new()
            .stack_size(64 * 1024 * 1024)
            .spawn(|| evaluate().map_err(|error| error.to_string()))
        {
            Ok(worker) => worker
                .join()
                .unwrap_or_else(|_| Err("batch worker failed".into())),
            Err(error) => Err(error.to_string()),
        }
    } else {
        evaluate().map_err(|error| error.to_string())
    };
    match result {
        Ok(text) => match io::stdout().lock().write_all(text.as_bytes()) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("error: {error}");
                ExitCode::FAILURE
            }
        },
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}
