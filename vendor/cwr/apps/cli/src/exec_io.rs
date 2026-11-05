use std::{
    error::Error,
    ffi::{OsStr, OsString},
    io::{self, Read, Write},
};

use serde_json::Value;

pub(super) fn write_json_record(record: &Value) -> Result<(), Box<dyn Error + Send + Sync>> {
    let stdout = io::stdout();
    let mut stdout = stdout.lock();
    serde_json::to_writer(&mut stdout, record)?;
    stdout.write_all(b"\n")?;
    stdout.flush()?;
    Ok(())
}

pub(super) fn read_exec_source(message: OsString) -> Result<String, Box<dyn Error + Send + Sync>> {
    if message != OsStr::new("-") {
        return message
            .into_string()
            .map_err(|_| "cwr exec message must be valid UTF-8".into());
    }

    let mut bytes = Vec::new();
    io::stdin()
        .read_to_end(&mut bytes)
        .map_err(|error| format!("cwr exec could not read standard input: {error}"))?;
    let source_text =
        String::from_utf8(bytes).map_err(|_| "cwr exec standard input must be valid UTF-8")?;
    Ok(source_text
        .strip_suffix("\r\n")
        .or_else(|| source_text.strip_suffix('\n'))
        .unwrap_or(&source_text)
        .to_owned())
}
