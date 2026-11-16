use std::{
    fs,
    io::{self, BufRead, Write},
};

use crate::{BOOTSTRAP_FRAGMENTS, Interpreter, eval_bootstrap, read};

const HELP: &str = "Bel bootstrap experiment: an incrementally self-hosted language core.\n\
Commands: :bootstrap, :load PATH, :cancel, :help, :quit\n\
Use :bootstrap to load the complete unchanged 1,801-line published Bel source.\n";

/// Run a persistent REPL. Ordinary reading, evaluation, and loading errors are recoverable.
pub fn run(input: impl BufRead, mut output: impl Write, mut errors: impl Write) -> io::Result<()> {
    let mut interpreter = Interpreter::default();
    let mut pending = String::new();
    let mut lines = input.lines();
    output.write_all(HELP.as_bytes())?;
    loop {
        output.write_all(if pending.is_empty() {
            b"bel> "
        } else {
            b"...> "
        })?;
        output.flush()?;
        let Some(line) = lines.next() else {
            if !pending.is_empty() {
                writeln!(errors, "error: incomplete expression at end of input")?;
            }
            return Ok(());
        };
        let line = line?;
        let trimmed = line.trim();
        if trimmed.starts_with(':') {
            if command(
                trimmed,
                &mut interpreter,
                &mut pending,
                &mut output,
                &mut errors,
            )? {
                return Ok(());
            }
            continue;
        }
        pending.push_str(&line);
        pending.push('\n');
        match read(&pending) {
            Ok(forms) => {
                pending.clear();
                match interpreter.eval_forms(forms) {
                    Ok(values) => {
                        for value in values {
                            match value.render() {
                                Ok(text) => writeln!(output, "{text}")?,
                                Err(error) => writeln!(errors, "error: {error}")?,
                            }
                        }
                    }
                    Err(error) => writeln!(errors, "error: {error}")?,
                }
            }
            Err(error) if error.incomplete => {}
            Err(error) => {
                pending.clear();
                writeln!(errors, "error: {error}")?;
            }
        }
    }
}

fn command(
    line: &str,
    interpreter: &mut Interpreter,
    pending: &mut String,
    output: &mut impl Write,
    errors: &mut impl Write,
) -> io::Result<bool> {
    match line {
        ":quit" => return Ok(true),
        ":cancel" => {
            pending.clear();
            writeln!(output, "input cancelled")?;
        }
        ":help" => output.write_all(HELP.as_bytes())?,
        ":bootstrap" => {
            if !pending.is_empty() {
                writeln!(
                    errors,
                    "error: finish the current expression or use :cancel before bootstrapping"
                )?;
            } else {
                match eval_bootstrap(interpreter) {
                    Ok(forms) => writeln!(
                        output,
                        "loaded {forms} bootstrap form(s) from {} fragment(s)",
                        BOOTSTRAP_FRAGMENTS.len()
                    )?,
                    Err(error) => writeln!(
                        errors,
                        "error: {error}; earlier bootstrap definitions may have been loaded"
                    )?,
                }
            }
        }
        _ if line == ":load" || line.starts_with(":load ") || line.starts_with(":load\t") => {
            if !pending.is_empty() {
                writeln!(
                    errors,
                    "error: finish the current expression or use :cancel before loading"
                )?;
            } else {
                load(line[5..].trim(), interpreter, output, errors)?;
            }
        }
        _ => writeln!(errors, "error: unknown command; use :help")?,
    }
    Ok(false)
}

fn load(
    path: &str,
    interpreter: &mut Interpreter,
    output: &mut impl Write,
    errors: &mut impl Write,
) -> io::Result<()> {
    if path.is_empty() {
        return writeln!(
            errors,
            "error: usage: :load PATH (relative to the current directory)"
        );
    }
    let source = match fs::read_to_string(path) {
        Ok(source) => source,
        Err(error) => return writeln!(errors, "error: cannot load {path}: {error}"),
    };
    match interpreter.eval_source(&source) {
        Ok(values) => writeln!(output, "loaded {} form(s) from {path}", values.len()),
        Err(error) => writeln!(
            errors,
            "error: {path}: {error}; earlier definitions may have been loaded"
        ),
    }
}
