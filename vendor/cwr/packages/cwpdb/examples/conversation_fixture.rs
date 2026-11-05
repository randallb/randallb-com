//! Local-only line protocol for canonical conversation backend conformance.
#[path = "support/schedules.rs"]
mod schedules;
use cwpdb::conversation::{self, ConversationStorage, NativeConversationStore};
use cwpdb::remote::{DeclaredTransport, RemoteStore};
use cwpdb::{invocation, CwpdbResult};
use serde_json::{json, Value};
use std::io::{self, BufRead, Write};
struct FixtureTransport {
    endpoint: String,
}
impl DeclaredTransport for FixtureTransport {
    fn call_read(
        &mut self,
        body: Value,
        control: &conversation::ReadControl,
    ) -> CwpdbResult<Value> {
        controlled_fixture_read(&self.endpoint, body, control)
    }

    fn call(&mut self, body: Value) -> CwpdbResult<Value> {
        let mut child = std::process::Command::new("/usr/bin/curl")
            .args([
                "--silent",
                "--show-error",
                "--fail-with-body",
                "--noproxy",
                "*",
                "--max-time",
                "60",
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

#[path = "support/controlled_read.rs"]
mod controlled_read;
use controlled_read::controlled_fixture_read;

struct Authority;
impl invocation::Authorizer for Authority {
    fn authorize(&self, _: &invocation::Context, _: &str, _: bool) -> CwpdbResult<()> {
        Ok(())
    }
}
impl cwpdb::documents::Validator for Authority {
    fn validate(
        &self,
        _: &str,
        _: Option<&serde_json::Map<String, Value>>,
        _: &serde_json::Map<String, Value>,
    ) -> CwpdbResult<()> {
        Ok(())
    }
}
fn documents(
    path: &std::path::Path,
    endpoint: &Option<String>,
    body: &Value,
) -> CwpdbResult<Value> {
    use cwpdb::documents::DocumentStorage;
    let mut store: Box<dyn DocumentStorage> = if let Some(endpoint) = endpoint {
        Box::new(RemoteStore::new(FixtureTransport {
            endpoint: endpoint.clone(),
        }))
    } else {
        Box::new(cwpdb::NativeStore::open(path)?)
    };
    match body["operation"].as_str() {
        Some("mutate_document") => Ok(serde_json::to_value(store.mutate_document(
            &serde_json::from_value(body["request"].clone())?,
            &Authority,
            &Authority,
        )?)?),
        Some("get_document") => Ok(serde_json::to_value(store.get_document(
            &serde_json::from_value(body["context"].clone())?,
            body["reference"].as_str().ok_or("reference required")?,
            &Authority,
        )?)?),
        _ => Err("unsupported fixture document operation".into()),
    }
}
fn scope(body: &Value) -> CwpdbResult<conversation::Scope> {
    serde_json::from_value(if body["scope"].is_object() {
        body["scope"].clone()
    } else {
        body["request"]["scope"].clone()
    })
    .map_err(Into::into)
}
fn native(
    path: &std::path::Path,
    scope: conversation::Scope,
) -> CwpdbResult<Box<dyn ConversationStorage>> {
    Ok(Box::new(NativeConversationStore::open(
        path,
        scope,
        conversation::OpenOptions {
            runtime_state_version: "cwp://org.coworkerprotocol/cwpdb/schemas/runtime-state/2026-09"
                .into(),
            workspace_binding: None,
            timeline_rollout: None,
        },
    )?))
}
fn dispatch(store: &mut dyn ConversationStorage, body: &Value) -> CwpdbResult<Value> {
    match body["operation"].as_str().ok_or("operation required")? {
        "conversation_history_status" => Ok(serde_json::to_value(store.history_status(
            &scope(body)?,
            body["reference"].as_str().ok_or("reference required")?,
            &Authority,
        )?)?),
        "conversation_contains_member" => Ok(serde_json::to_value(store.contains_member(
            &scope(body)?,
            body["source_ref"].as_str().ok_or("source required")?,
            serde_json::from_value(body["membership_kind"].clone())?,
            body["target_ref"].as_str().ok_or("target required")?,
            &Authority,
        )?)?),
        "read_conversation_selection" => Ok(serde_json::to_value(store.read_selection(
            &serde_json::from_value(body["request"].clone())?,
            &conversation::ReadControl {
                cancellation: conversation::ReadCancellation::default(),
                deadline: std::time::Instant::now() + std::time::Duration::from_secs(30),
                vm_steps: None,
            },
            &Authority,
        )?)?),
        "conversation_history_read" => Ok(serde_json::to_value(store.history_read(
            &serde_json::from_value(body["request"].clone())?,
            &conversation::ReadControl {
                cancellation: conversation::ReadCancellation::default(),
                deadline: std::time::Instant::now() + std::time::Duration::from_secs(30),
                vm_steps: None,
            },
            &Authority,
        )?)?),
        "conversation_timeline_page" => Ok(serde_json::to_value(store.timeline_page(
            &serde_json::from_value(body["request"].clone())?,
            &conversation::ReadControl {
                cancellation: conversation::ReadCancellation::default(),
                deadline: std::time::Instant::now() + std::time::Duration::from_secs(30),
                vm_steps: None,
            },
            &Authority,
        )?)?),
        "conversation_content_page" => Ok(serde_json::to_value(store.content_page(
            &serde_json::from_value(body["request"].clone())?,
            &Authority,
        )?)?),
        "conversation_completion_page" => Ok(serde_json::to_value(store.completion_page(
            &serde_json::from_value(body["request"].clone())?,
            &Authority,
        )?)?),
        "conversation_resource" => Ok(serde_json::to_value(store.resource(
            &scope(body)?,
            body["reference"].as_str().ok_or("reference required")?,
            &Authority,
        )?)?),
        "conversation_summary" => Ok(serde_json::to_value(store.summary(
            &scope(body)?,
            body["reference"].as_str().ok_or("reference required")?,
            &Authority,
        )?)?),
        "conversation_endpoint_kinds" => Ok(serde_json::to_value(store.endpoint_kinds(
            &scope(body)?,
            body["reference"].as_str().ok_or("reference required")?,
            &Authority,
        )?)?),
        "conversation_identity_in_use" => Ok(serde_json::to_value(store.identity_in_use(
            &scope(body)?,
            body["reference"].as_str().ok_or("reference required")?,
            &Authority,
        )?)?),
        "conversation_retained_order" => Ok(serde_json::to_value(store.retained_order(
            &scope(body)?,
            body["reference"].as_str().ok_or("reference required")?,
            &Authority,
        )?)?),
        "published_progress_conversation" => Ok(serde_json::to_value(
            store.published_progress_conversation(
                &scope(body)?,
                body["reference"].as_str().ok_or("reference required")?,
                &Authority,
            )?,
        )?),
        "read_conversation_graph" => Ok(serde_json::to_value(store.read(
            &serde_json::from_value(body["request"].clone())?,
            &Authority,
        )?)?),
        "get_conversation_record" => Ok(serde_json::to_value(store.get(
            &scope(body)?,
            body["reference"].as_str().ok_or("reference required")?,
            &Authority,
        )?)?),
        "commit_conversation_graph" => Ok(serde_json::to_value(store.commit(
            &serde_json::from_value(body["request"].clone())?,
            &Authority,
        )?)?),
        "reconcile_conversation_graph" => Ok(serde_json::to_value(store.reconcile(
            &serde_json::from_value(body["request"].clone())?,
            &Authority,
        )?)?),
        _ => Err("unsupported conversation fixture operation".into()),
    }
}
fn main() -> CwpdbResult<()> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let remote = args.first().map(String::as_str) == Some("--remote");
    let endpoint = if remote {
        let endpoint = args.get(1).ok_or("loopback endpoint required")?.clone();
        if !endpoint.starts_with("http://127.0.0.1:") || endpoint[17..].contains(['?', '#', '@']) {
            return Err("fixture transport requires loopback endpoint".into());
        }
        Some(endpoint)
    } else {
        None
    };
    let path = std::path::PathBuf::from(args.first().ok_or("fixture path required")?);
    let mut store: Option<Box<dyn ConversationStorage>> = endpoint.clone().map(|endpoint| {
        Box::new(RemoteStore::new(FixtureTransport { endpoint })) as Box<dyn ConversationStorage>
    });
    for line in io::stdin().lock().lines() {
        let result = (|| -> CwpdbResult<Value> {
            let body: Value = serde_json::from_str(&line?)?;
            if body["operation"] == "schedule" {
                return schedules::dispatch(&path, &endpoint, &body);
            }
            if matches!(
                body["operation"].as_str(),
                Some("mutate_document" | "get_document")
            ) {
                return documents(&path, &endpoint, &body);
            }
            if body["operation"] == "fixture_reopen" {
                if !remote {
                    store = Some(native(&path, scope(&body)?)?);
                }
                return Ok(json!(null));
            }
            if !remote && body["operation"] == "read_conversation_selection" {
                let request: conversation::ReadSelectionRequest =
                    serde_json::from_value(body["request"].clone())?;
                let control = conversation::ReadControl {
                    cancellation: conversation::ReadCancellation::default(),
                    deadline: std::time::Instant::now() + std::time::Duration::from_secs(30),
                    vm_steps: None,
                };
                let mut reader = conversation::NativeReadStore::open(
                    &path,
                    &request.scope.runtime_root_ref,
                    &control,
                )?;
                let snapshot = reader.begin_current(control)?;
                return Ok(serde_json::to_value(snapshot.read_selection(&request)?)?);
            }
            if store.is_none() {
                store = Some(native(&path, scope(&body)?)?);
            }
            dispatch(store.as_mut().unwrap().as_mut(), &body)
        })();
        let envelope = match result {
            Ok(value) => json!({"ok":true,"result":value}),
            Err(error) => json!({"ok":false,"error":error.to_string()}),
        };
        println!("{envelope}");
        io::stdout().flush()?;
    }
    Ok(())
}
