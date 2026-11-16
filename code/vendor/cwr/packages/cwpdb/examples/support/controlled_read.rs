use cwpdb::{conversation::ReadControl, CwpdbResult};
use serde_json::Value;
use std::{
    io::{Read, Write},
    process::{Command, Stdio},
    time::Duration,
};
pub fn controlled_fixture_read(
    endpoint: &str,
    body: Value,
    control: &ReadControl,
) -> CwpdbResult<Value> {
    control.check()?;
    let mut child = Command::new("/usr/bin/curl")
        .args([
            "--silent",
            "--show-error",
            "--fail-with-body",
            "--noproxy",
            "*",
            "--max-time",
            &control
                .deadline
                .saturating_duration_since(std::time::Instant::now())
                .as_secs_f64()
                .to_string(),
            "-H",
            "Authorization: Bearer fictional-local-cwpdb-conformance-only",
            "-H",
            "Content-Type: application/json",
            "--data-binary",
            "@-",
            endpoint,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    child
        .stdin
        .take()
        .ok_or("missing fixture request pipe")?
        .write_all(&serde_json::to_vec(&body)?)?;
    let output = child.stdout.take().ok_or("missing fixture response pipe")?;
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        output
            .take(24 * 1024 * 1024 + 65537)
            .read_to_end(&mut bytes)?;
        Ok::<_, std::io::Error>(bytes)
    });
    let status = loop {
        if let Err(error) = control.check() {
            let _ = child.kill();
            let _ = child.wait();
            let _ = reader.join();
            return Err(error);
        }
        if let Some(status) = child.try_wait()? {
            break status;
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    let bytes = reader
        .join()
        .map_err(|_| "fixture response reader failed")??;
    control.check()?;
    if !status.success() {
        return Err("controlled storage fixture refused request".into());
    }
    if bytes.len() > 24 * 1024 * 1024 + 65536 {
        return Err("fixture response exceeds byte budget".into());
    }
    let envelope: Value = serde_json::from_slice(&bytes)?;
    if envelope["ok"] != true {
        return Err(envelope["error"]
            .as_str()
            .unwrap_or("storage fixture failed")
            .to_owned()
            .into());
    }
    Ok(envelope["result"].clone())
}
