//! Optional authenticated execution listener for the same native CWR runtime.
use axum::{
    extract::{DefaultBodyLimit, Request, State},
    http::StatusCode,
    Json, Router,
};
use cwr::{
    cloud_execution::{
        CloudExecutorEndpoint, CloudHttpService, CloudServiceConfig, HttpDeclaredTransport,
    },
    cwpdb::NativeStore,
    v2026_09::local::LocalRuntime,
    CwrResult,
};
use serde_json::{json, Value};
use std::{
    net::{SocketAddr, TcpListener},
    path::Path,
    sync::Arc,
    thread,
};
use tokio::sync::{oneshot, Semaphore};

#[derive(Clone)]
struct HttpState {
    service: Arc<CloudHttpService>,
    permits: Arc<Semaphore>,
}
pub(crate) struct CloudServer {
    stop: Option<oneshot::Sender<()>>,
    thread: Option<thread::JoinHandle<()>>,
}
impl Drop for CloudServer {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
fn required(name: &str) -> CwrResult<String> {
    std::env::var(name)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .ok_or_else(|| format!("Cloud execution requires {name}").into())
}

pub(crate) fn start_from_environment(
    runtime: LocalRuntime,
    state_directory: &Path,
) -> CwrResult<Option<CloudServer>> {
    let Some(address) = std::env::var_os("CWR_CLOUD_HTTP_BIND") else {
        return Ok(None);
    };
    let address: SocketAddr = address
        .to_str()
        .ok_or("Cloud bind address must be UTF-8")?
        .parse()?;
    if !address.ip().is_loopback() || address.port() == 0 {
        return Err("Cloud execution listener requires an explicit private loopback port".into());
    }
    let origin = required("CWR_CLOUD_COORDINATOR_URL")?;
    let expected = format!("http://{address}");
    if origin != expected && origin != format!("{expected}/") {
        return Err("Cloud coordinator origin must match its private listener".into());
    }
    let executors: Vec<CloudExecutorEndpoint> =
        serde_json::from_str(&required("CWR_CLOUD_EXECUTORS")?)?;
    let config = CloudServiceConfig {
        workspace_ref: required("CWR_CLOUD_WORKSPACE_REF")?,
        coordinator_actor_ref: required("CWR_CLOUD_COORDINATOR_ACTOR_REF")?,
        coordinator_token: required("CWR_CLOUD_COORDINATOR_TOKEN")?,
        assignment_signing_key: required("CWR_CLOUD_SIGNING_KEY")?.into_bytes(),
        executors,
        assignment_lifetime_ms: 900_000,
        model: std::env::var("CWR_CLOUD_MODEL_GRANT")
            .ok()
            .map(|value| serde_json::from_str(&value))
            .transpose()?,
    };
    // The invocation assignment/event journal and ordinary conversation state
    // must share the host-selected canonical backend. A disposable host's local
    // directory cannot own recovery state when remote storage is required.
    let service = if let Some(endpoint) = std::env::var_os("CWR_CONVERSATION_STORAGE_URL") {
        if required("CWR_CONVERSATION_WORKSPACE_REF")? != config.workspace_ref {
            return Err("Cloud journal storage scope differs from its trusted host".into());
        }
        let transport = HttpDeclaredTransport::new(
            endpoint.to_str().ok_or("Invalid remote journal endpoint")?,
            required("CWR_CONVERSATION_STORAGE_TOKEN")?,
        )?;
        CloudHttpService::new_with_storage_caller(
            runtime,
            cwr::cwpdb::remote::RemoteStore::new(transport),
            config,
            required("CWR_CONVERSATION_CALLER_REF")?,
        )?
    } else {
        if std::env::var_os("CWR_REQUIRE_REMOTE_STATE").is_some() {
            return Err(
                "Cloud invocation journal requires the configured remote canonical backend".into(),
            );
        }
        CloudHttpService::new(
            runtime,
            NativeStore::open(state_directory.join("cwpdb.sqlite3"))?,
            config,
        )?
    };
    let service = Arc::new(service);
    let listener = TcpListener::bind(address)?;
    Ok(Some(start(listener, service)?))
}

pub(crate) fn start(
    listener: TcpListener,
    service: Arc<CloudHttpService>,
) -> CwrResult<CloudServer> {
    if !listener.local_addr()?.ip().is_loopback() {
        return Err("Cloud listener must remain private".into());
    }
    listener.set_nonblocking(true)?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    let (stop, stopped) = oneshot::channel();
    let thread = thread::spawn(move || {
        runtime.block_on(async move {
            let listener = match tokio::net::TcpListener::from_std(listener) {
                Ok(listener) => listener,
                Err(_) => return,
            };
            let state = HttpState {
                service,
                permits: Arc::new(Semaphore::new(32)),
            };
            let router = Router::new()
                .fallback(handle)
                .layer(DefaultBodyLimit::max(512 * 1024))
                .with_state(state);
            let _ = axum::serve(listener, router)
                .with_graceful_shutdown(async {
                    let _ = stopped.await;
                })
                .await;
        });
    });
    Ok(CloudServer {
        stop: Some(stop),
        thread: Some(thread),
    })
}

async fn handle(State(state): State<HttpState>, request: Request) -> (StatusCode, Json<Value>) {
    let permit = match state.permits.try_acquire_owned() {
        Ok(permit) => permit,
        Err(_) => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"error":"service_busy"})),
            )
        }
    };
    let authorization = match request
        .headers()
        .get("authorization")
        .and_then(|value| value.to_str().ok())
    {
        Some(value) if value.len() <= 4096 => value.to_owned(),
        _ => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({"error":"unauthorized"})),
            )
        }
    };
    let method = request.method().as_str().to_owned();
    let path = request.uri().to_string();
    let bytes = match axum::body::to_bytes(request.into_body(), 512 * 1024).await {
        Ok(bytes) => bytes,
        Err(_) => {
            return (
                StatusCode::PAYLOAD_TOO_LARGE,
                Json(json!({"error":"request_too_large"})),
            )
        }
    };
    let body = if bytes.is_empty() {
        json!({})
    } else {
        match serde_json::from_slice(&bytes) {
            Ok(value) => value,
            Err(_) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({"error":"invalid_json"})),
                )
            }
        }
    };
    let result = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        state.service.handle(&method, &path, &authorization, body)
    })
    .await;
    match result {
        Ok(Ok(response)) => (
            StatusCode::from_u16(response.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            Json(response.body),
        ),
        Ok(Err(error)) => {
            eprintln!("cloud execution request refused: {error}");
            (
                StatusCode::CONFLICT,
                Json(json!({"error":"operation_refused_or_requires_reconciliation"})),
            )
        }
        _ => (
            StatusCode::CONFLICT,
            Json(json!({"error":"operation_refused_or_requires_reconciliation"})),
        ),
    }
}

#[cfg(test)]
mod tests;
