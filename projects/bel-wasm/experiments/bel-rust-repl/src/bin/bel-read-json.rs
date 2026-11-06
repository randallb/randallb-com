//! Serialize reader data to JSON without evaluating it or interpreting a language.
use bel_rust_repl::{Value, read};
use std::{env, fs, process::ExitCode};

fn string(text: &str, out: &mut String) {
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c < ' ' => out.push_str(&format!("\\u{:04x}", u32::from(c))),
            c => out.push(c),
        }
    }
    out.push('"');
}

fn value(x: &Value, out: &mut String) {
    match x {
        Value::Nil => out.push_str("null"),
        Value::Symbol(s) => string(s, out),
        Value::Char(c) => {
            out.push_str("{\"char\":");
            string(&c.to_string(), out);
            out.push('}');
        }
        Value::Pair(_) => {
            // Proper lists use arrays; dotted tails use the explicit pair tag.
            let mut tail = x.clone();
            let mut items = Vec::new();
            while let Value::Pair(p) = tail {
                let (a, d) = p.borrow().clone();
                items.push(a);
                tail = d;
            }
            if matches!(tail, Value::Nil) {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    value(item, out);
                }
                out.push(']');
            } else {
                for item in &items {
                    out.push_str("{\"pair\":[");
                    value(item, out);
                    out.push(',');
                }
                value(&tail, out);
                for _ in &items {
                    out.push_str("]}");
                }
            }
        }
    }
}

fn run() -> Result<String, Box<dyn std::error::Error>> {
    let mut args = env::args_os().skip(1);
    let path = args.next().ok_or("usage: bel-read-json SOURCE")?;
    if args.next().is_some() {
        return Err("usage: bel-read-json SOURCE".into());
    }
    let forms = read(&fs::read_to_string(path)?)?;
    let mut out = String::new();
    value(&Value::list(forms), &mut out);
    Ok(out)
}

fn main() -> ExitCode {
    match run() {
        Ok(out) => {
            println!("{out}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}
