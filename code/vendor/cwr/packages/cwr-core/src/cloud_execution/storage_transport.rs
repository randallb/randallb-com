//! Host-selected private named cwpdb transport. No SQL/transaction capability.
use crate::CwrResult;
use cwpdb_store::{remote::DeclaredTransport, CwpdbResult};
use reqwest::{blocking::Client, redirect::Policy, Url};
use serde_json::Value;
use std::{io::Read, time::Duration};
const MAX_BYTES: u64 = 24 * 1024 * 1024;
const MAX_WIRE_BYTES: u64 = MAX_BYTES + 16 * 1024;
pub struct HttpDeclaredTransport {
    client: Client,
    endpoint: Url,
    token: String,
}
impl HttpDeclaredTransport {
    pub fn new(endpoint: &str, token: String) -> CwrResult<Self> {
        let endpoint = Url::parse(endpoint)?;
        let private = endpoint.host_str() == Some("cwpdb.internal")
            && endpoint.port_or_known_default() == Some(80);
        let local = matches!(
            endpoint.host_str(),
            Some("127.0.0.1" | "localhost" | "[::1]")
        );
        if token.len() < 32
            || !endpoint.username().is_empty()
            || endpoint.password().is_some()
            || endpoint.query().is_some()
            || endpoint.fragment().is_some()
            || endpoint.scheme() != "https" && !(endpoint.scheme() == "http" && (private || local))
        {
            return Err(
                "Declared storage requires an authenticated private binding or HTTPS endpoint"
                    .into(),
            );
        }
        Ok(Self {
            client: Client::builder()
                .no_proxy()
                .redirect(Policy::none())
                .timeout(Duration::from_secs(30))
                .build()?,
            endpoint,
            token,
        })
    }
}
impl DeclaredTransport for HttpDeclaredTransport {
    fn call_read(
        &mut self,
        body: Value,
        control: &cwpdb_store::conversation::ReadControl,
    ) -> CwpdbResult<Value> {
        control.check()?;
        let encoded = serde_json::to_vec(&body)?;
        if encoded.len() as u64 > MAX_WIRE_BYTES {
            return Err("declared storage read exceeds 24 MiB".into());
        }
        let endpoint = self.endpoint.clone();
        let token = self.token.clone();
        let control = control.clone();
        // A private runtime thread lets cancellation drop the live HTTP future,
        // including a stalled header/body read, without a detached request.
        std::thread::spawn(move || -> CwpdbResult<Value> {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            let result = runtime.block_on(async {
                let mut operation =
                    Box::pin(read_response(endpoint, token, encoded, control.clone()));
                loop {
                    control.check()?;
                    match tokio::time::timeout(Duration::from_millis(5), operation.as_mut()).await {
                        Ok(result) => {
                            control.check()?;
                            return result;
                        }
                        Err(_) => continue,
                    }
                }
            });
            runtime.shutdown_timeout(Duration::from_millis(10));
            result
        })
        .join()
        .map_err(|_| "declared read transport worker failed")?
    }

    fn call(&mut self, body: Value) -> CwpdbResult<Value> {
        let encoded = serde_json::to_vec(&body)?;
        if encoded.len() as u64 > MAX_WIRE_BYTES {
            return Err("declared storage mutation exceeds 24 MiB".into());
        }
        let response = self
            .client
            .post(self.endpoint.clone())
            .bearer_auth(&self.token)
            .header("content-type", "application/json")
            .body(encoded)
            .send()?;
        if !response.status().is_success() {
            let status = response.status().as_u16();
            let mut refusal = Vec::new();
            response.take(4097).read_to_end(&mut refusal)?;
            if status == 409
                && refusal.len() <= 4096
                && serde_json::from_slice::<Value>(&refusal)
                    .is_ok_and(|value| value["error"]["code"] == "source_authority_changed")
            {
                return Err(Box::new(cwpdb_store::conversation::SourceAuthorityConflict));
            }
            return Err(format!("declared storage operation refused ({status})").into());
        }
        let mut bytes = Vec::new();
        response.take(MAX_WIRE_BYTES + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_WIRE_BYTES {
            return Err("declared storage response exceeds 24 MiB".into());
        }
        decode_value(&bytes)
    }
}

async fn read_response(
    endpoint: Url,
    token: String,
    encoded: Vec<u8>,
    control: cwpdb_store::conversation::ReadControl,
) -> CwpdbResult<Value> {
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(Policy::none())
        .timeout(
            control
                .deadline
                .saturating_duration_since(std::time::Instant::now()),
        )
        .build()?;
    let mut response = client
        .post(endpoint)
        .bearer_auth(token)
        .header("content-type", "application/json")
        .body(encoded)
        .send()
        .await?;
    let status = response.status().as_u16();
    let success = response.status().is_success();
    let cap = if success { MAX_WIRE_BYTES } else { 4096 };
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        control.check()?;
        if bytes.len() as u64 + chunk.len() as u64 > cap {
            return Err(if success {
                "declared storage response exceeds 24 MiB".to_owned()
            } else {
                format!("declared storage operation refused ({status})")
            }
            .into());
        }
        bytes.extend_from_slice(&chunk);
    }
    if !success {
        if status == 409
            && serde_json::from_slice::<Value>(&bytes)
                .is_ok_and(|value| value["error"]["code"] == "source_authority_changed")
        {
            return Err(Box::new(cwpdb_store::conversation::SourceAuthorityConflict)
                as Box<dyn std::error::Error + Send + Sync>);
        }
        return Err(format!("declared storage operation refused ({status})").into());
    }
    decode_value(&bytes)
}

fn decode_value(bytes: &[u8]) -> CwpdbResult<Value> {
    let value: Value = serde_json::from_slice(bytes)?;
    if let Some(ok) = value.get("ok").and_then(Value::as_bool) {
        if !ok {
            if value["error"].as_str() == Some("source authority revision conflict") {
                return Err(Box::new(cwpdb_store::conversation::SourceAuthorityConflict));
            }
            return Err(value["error"]
                .as_str()
                .unwrap_or("declared storage refused operation")
                .to_owned()
                .into());
        }
        let result = value
            .get("result")
            .ok_or("missing declared storage result")?
            .clone();
        if serde_json::to_vec(&result)?.len() as u64 > MAX_BYTES {
            return Err("declared storage result exceeds 24 MiB".into());
        }
        return Ok(result);
    }
    if bytes.len() as u64 > MAX_BYTES {
        return Err("declared storage result exceeds 24 MiB".into());
    }
    Ok(value)
}

#[cfg(test)]
mod read_control_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;

    #[test]
    fn private_refusals_preserve_only_the_typed_source_conflict() {
        for (status, body, conflict) in [
            (
                409,
                r#"{"error":{"code":"source_authority_changed"}}"#,
                true,
            ),
            (
                500,
                r#"{"error":{"code":"source_authority_changed"}}"#,
                false,
            ),
            (409, "private database details", false),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let endpoint = format!("http://{}/declared", listener.local_addr().unwrap());
            let server = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut length = 0;
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    if line == "\r\n" {
                        break;
                    }
                    if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        length = value.trim().parse::<usize>().unwrap();
                    }
                }
                reader.read_exact(&mut vec![0; length]).unwrap();
                write!(stream, "HTTP/1.1 {status} Refused\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            });
            let mut transport = HttpDeclaredTransport::new(
                &endpoint,
                "fictional-storage-token-only-32-characters".into(),
            )
            .unwrap();
            let error = transport
                .call(serde_json::json!({"operation":"fixture"}))
                .unwrap_err();
            server.join().unwrap();
            assert_eq!(
                error.is::<cwpdb_store::conversation::SourceAuthorityConflict>(),
                conflict
            );
            assert!(!error.to_string().contains("private database details"));
        }
    }
}
