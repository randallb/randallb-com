//! Line-delimited fixture driver for native/hosted conformance. Fictional data only.
use cwpdb::remote::{DeclaredTransport, RemoteStore};
use cwpdb::{
    documents::{self, DocumentStorage},
    invocation::{self, InvocationStorage},
    CwpdbResult, NativeStore,
};
use serde_json::{json, Value};
use std::io::{self, BufRead, Write};
trait FixtureStore: DocumentStorage + InvocationStorage {}
impl<T: DocumentStorage + InvocationStorage> FixtureStore for T {}
struct FixtureTransport {
    endpoint: String,
}
impl DeclaredTransport for FixtureTransport {
    fn call(&mut self, body: Value) -> CwpdbResult<Value> {
        let mut child = std::process::Command::new("/usr/bin/curl")
            .args([
                "--silent",
                "--show-error",
                "--fail-with-body",
                "--noproxy",
                "*",
                "--max-time",
                "10",
                "-H",
                "Authorization: Bearer fictional-local-cwpdb-conformance-only",
                "-H",
                "Content-Type: application/json",
                "--data-binary",
                "@-",
                &self.endpoint,
            ])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()?;
        child
            .stdin
            .take()
            .ok_or("missing fixture request pipe")?
            .write_all(&serde_json::to_vec(&body)?)?;
        let output = child.wait_with_output()?;
        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).into_owned().into());
        }
        let envelope: Value = serde_json::from_slice(&output.stdout)?;
        if envelope["ok"] != true {
            return Err(envelope["error"]
                .as_str()
                .unwrap_or("storage fixture failed")
                .to_string()
                .into());
        }
        Ok(envelope["result"].clone())
    }
}
struct Authority;
impl invocation::Authorizer for Authority {
    fn authorize(&self, _: &invocation::Context, _: &str, _: bool) -> CwpdbResult<()> {
        Ok(())
    }
}
impl documents::Validator for Authority {
    fn validate(
        &self,
        _: &str,
        _: Option<&serde_json::Map<String, Value>>,
        _: &serde_json::Map<String, Value>,
    ) -> CwpdbResult<()> {
        Ok(())
    }
}
fn dispatch(store: &mut dyn FixtureStore, body: Value) -> CwpdbResult<Value> {
    let op = body["operation"].as_str().ok_or("operation required")?;
    match op {
        "mutate_document" => Ok(serde_json::to_value(store.mutate_document(
            &serde_json::from_value(body["request"].clone())?,
            &Authority,
            &Authority,
        )?)?),
        "reconcile_document" => Ok(serde_json::to_value(store.reconcile_document(
            &serde_json::from_value(body["request"].clone())?,
            &Authority,
        )?)?),
        "get_document" => Ok(serde_json::to_value(store.get_document(
            &serde_json::from_value(body["context"].clone())?,
            body["reference"].as_str().ok_or("reference required")?,
            &Authority,
        )?)?),
        "mutate_invocation" => Ok(serde_json::to_value(store.mutate(
            &serde_json::from_value(body["invocation_request"].clone())?,
            &Authority,
            body["now_unix_ms"].as_u64().ok_or("now required")?,
        )?)?),
        "invocation_receipt" => Ok(serde_json::to_value(
            store.receipt(
                &serde_json::from_value(body["context"].clone())?,
                body["invocation_ref"]
                    .as_str()
                    .ok_or("invocation_ref required")?,
                body["request_id"].as_str().ok_or("request_id required")?,
                &Authority,
            )?,
        )?),
        "get_invocation" => Ok(serde_json::to_value(
            store.invocation(
                &serde_json::from_value(body["context"].clone())?,
                body["invocation_ref"]
                    .as_str()
                    .ok_or("invocation_ref required")?,
                &Authority,
            )?,
        )?),
        "invocation_events" => Ok(serde_json::to_value(
            store.events(
                &serde_json::from_value(body["context"].clone())?,
                body["invocation_ref"]
                    .as_str()
                    .ok_or("invocation_ref required")?,
                body["after"].as_u64(),
                body["limit"].as_u64().ok_or("limit required")? as usize,
                &Authority,
            )?,
        )?),
        _ => Err("unsupported operation".into()),
    }
}
fn main() -> CwpdbResult<()> {
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    let mut store: Box<dyn FixtureStore> = if arguments.first().map(String::as_str)
        == Some("--remote")
    {
        let endpoint = arguments
            .get(1)
            .ok_or("fixture loopback endpoint required")?
            .clone();
        if !endpoint.starts_with("http://127.0.0.1:") || endpoint[17..].contains(['?', '#', '@']) {
            return Err("fixture transport requires loopback endpoint".into());
        }
        Box::new(RemoteStore::new(FixtureTransport { endpoint }))
    } else {
        Box::new(NativeStore::open(
            arguments.first().ok_or("fixture database path required")?,
        )?)
    };
    for line in io::stdin().lock().lines() {
        let result = line
            .map_err(|e| e.into())
            .and_then(|line| serde_json::from_str(&line).map_err(|e| e.into()))
            .and_then(|value| dispatch(store.as_mut(), value));
        let output = match result {
            Ok(value) => json!({"ok":true,"result":value}),
            Err(error) => json!({"ok":false,"error":error.to_string()}),
        };
        println!("{output}");
    }
    Ok(())
}
