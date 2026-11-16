use std::{
    env,
    error::Error,
    io::{self, BufRead, Read, Write},
    os::unix::net::UnixStream,
    path::Path,
    time::Duration,
};

use serde_json::{json, Value};

const MAX_BYTES: usize = 1024 * 1024;
const ACTIONS: [&str; 3] = ["receive--message", "deliver--message", "handle--request"];

pub fn run(args: &[std::ffi::OsString]) -> Result<(), Box<dyn Error + Send + Sync>> {
    forward(args).map_err(|_| "Workloop SMS action unavailable".into())
}

fn forward(args: &[std::ffi::OsString]) -> Result<(), Box<dyn Error + Send + Sync>> {
    let [action] = args else {
        return Err("invalid action arguments".into());
    };
    let action = action.to_str().ok_or("invalid action name")?;
    if !ACTIONS.contains(&action) {
        return Err("invalid action name".into());
    }
    let invocation_ref = env::var("CWR_INVOCATION_REF")?;
    if !invocation_ref.starts_with("cwp://") {
        return Err("invalid invocation reference".into());
    }
    let socket = env::var_os("WORKLOOP_SMS_BRIDGE_SOCKET").ok_or("missing bridge socket")?;
    if !Path::new(&socket).is_absolute() {
        return Err("invalid bridge socket".into());
    }

    let mut bytes = Vec::new();
    io::stdin()
        .take((MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_BYTES {
        return Err("action input too large".into());
    }
    let input: Value = serde_json::from_slice(&bytes)?;
    if !input.is_object() {
        return Err("invalid action input".into());
    }
    let packet = json!({
        "version": 1,
        "action_name": action,
        "invocation_ref": invocation_ref,
        "input": input,
    });
    let mut encoded = serde_json::to_vec(&packet)?;
    if encoded.len() > MAX_BYTES {
        return Err("action packet too large".into());
    }
    encoded.push(b'\n');

    let mut connection = UnixStream::connect(socket)?;
    let timeout = if action == "deliver--message" {
        Duration::from_secs(55 * 60)
    } else {
        Duration::from_secs(60)
    };
    connection.set_read_timeout(Some(timeout))?;
    connection.set_write_timeout(Some(timeout))?;
    connection.write_all(&encoded)?;
    let mut response = Vec::new();
    io::BufReader::new(connection)
        .take((MAX_BYTES + 1) as u64)
        .read_until(b'\n', &mut response)?;
    if response.len() > MAX_BYTES || response.last() != Some(&b'\n') {
        return Err("invalid bridge response".into());
    }
    let envelope: Value = serde_json::from_slice(&response)?;
    if envelope.get("ok") != Some(&Value::Bool(true)) {
        return Err("bridge rejected action".into());
    }
    serde_json::to_writer(io::stdout(), envelope.get("result").unwrap_or(&Value::Null))?;
    Ok(())
}
