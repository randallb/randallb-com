//! CWR-owned consumer API. The resident remains the runtime owner.
mod actors;
mod attachments;
mod budget;
mod contexts;
mod controls;
mod conversation_summary;
mod coworkers;
mod file_upload;
mod files;
mod inspector;
mod inspector_budget;
mod invocation_action;
mod invocations;
mod legacy_history;
mod messages;
mod messaging;
mod mutations;
mod publications;
mod schedules;
mod schema;
mod sessions;
mod socket;
mod subscriptions;
mod terminals;
#[cfg(test)]
mod tests;
mod timing;
mod windows;

use async_graphql::Schema;
use axum::{
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, StatusCode},
    routing::post,
    Json, Router,
};
use cwr::v2026_09::local::LocalAdmission;
use cwr::{
    v2026_09::local::{LocalGraphqlEndpoint, LocalRuntime},
    CwrResult,
};
use mutations::Mutation;
use schema::{Query, ReadService};
use subscriptions::Subscription;
pub(crate) type AdmissionPublisher = Arc<dyn Fn(&LocalAdmission) -> CwrResult<()> + Send + Sync>;
use std::{
    fs,
    future::IntoFuture,
    io::Write,
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    sync::Arc,
    thread,
    time::Duration,
};
use tokio::sync::{oneshot, Semaphore};

const SOCKET_CAPACITY: u32 = 32;

type RuntimeSchema = Schema<Query, Mutation, Subscription>;
#[derive(Clone)]
struct HttpState {
    schema: RuntimeSchema,
    sessions: Arc<sessions::Sessions>,
    endpoint: LocalGraphqlEndpoint,
    activity: crate::idle::Activity,
    permits: Arc<Semaphore>,
    sockets: Arc<Semaphore>,
    shutdown: tokio::sync::watch::Receiver<bool>,
}

pub(crate) struct GraphqlServer {
    endpoint: LocalGraphqlEndpoint,
    path: PathBuf,
    _socket: socket::SocketFile,
    shutdown: tokio::sync::watch::Sender<bool>,
    stop: Option<oneshot::Sender<()>>,
    thread: Option<thread::JoinHandle<()>>,
}

pub(crate) fn sdl() -> String {
    Schema::build(Query, Mutation::default(), Subscription)
        .finish()
        .sdl()
}

pub(crate) fn start(
    runtime: LocalRuntime,
    runtime_ref: String,
    directory: &Path,
    activity: crate::idle::Activity,
    publisher: AdmissionPublisher,
    changes: tokio::sync::watch::Receiver<()>,
) -> CwrResult<GraphqlServer> {
    let actor_ref = runtime.catalog()?.acting_actor.actor_ref;
    let (listener, socket) = socket::SocketFile::bind(directory.join("graphql.sock"))?;
    let endpoint = LocalGraphqlEndpoint {
        version: 2,
        socket_path: socket.path().to_owned(),
        runtime_ref: runtime_ref.clone(),
        actor_ref: actor_ref.clone(),
        token: format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        ),
        pid: std::process::id(),
    };
    let (shutdown, shutdown_rx) = tokio::sync::watch::channel(false);
    let (session_changes, session_observation) = tokio::sync::watch::channel(());
    let sessions = Arc::new(sessions::Sessions::new(
        ReadService {
            configurations: Default::default(),
            action_titles: Default::default(),
            runtime,
            runtime_ref,
            actor_ref,
            owner: None,
            ingress: "cwp://org.coworkerprotocol.local/integrations/graphql".into(),
            publications: Default::default(),
            reads: Arc::new(Semaphore::new(8)),
            budget: None,
            publisher,
        },
        session_changes,
    ));
    let schema = Schema::build(Query, Mutation::default(), Subscription)
        .extension(budget::BudgetExtension)
        .extension(inspector_budget::InspectorResponseBudget)
        .data(subscriptions::Observation {
            changes,
            session_changes: session_observation,
            restarts: activity.restart_notifications(),
            permits: Arc::new(Semaphore::new(64)),
        })
        .limit_depth(64)
        .limit_complexity(1000)
        .limit_recursive_depth(32)
        .finish();
    let state = HttpState {
        schema,
        sessions,
        endpoint: endpoint.clone(),
        activity,
        permits: Arc::new(Semaphore::new(8)),
        sockets: Arc::new(Semaphore::new(SOCKET_CAPACITY as usize)),
        shutdown: shutdown_rx,
    };
    let mut shutdown_wait = state.shutdown.clone();
    let sockets = state.sockets.clone();
    let app = Router::new()
        .route("/graphql", post(execute).get(subscriptions::upgrade))
        .route("/files", post(file_upload::upload))
        .route(
            "/graphql/session",
            post(sessions::open).delete(sessions::close),
        )
        .layer(DefaultBodyLimit::max(15 * 1024 * 1024))
        .layer(axum::middleware::from_fn(
            |request: axum::extract::Request, next: axum::middleware::Next| async move {
                tokio::time::timeout(Duration::from_secs(15), next.run(request))
                    .await
                    .map_err(|_| StatusCode::REQUEST_TIMEOUT)
            },
        ))
        .with_state(state);
    let executor = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    let (stop, stopping) = oneshot::channel();
    let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
    let thread = thread::Builder::new()
        .name("cwr-graphql".into())
        .spawn(move || {
            executor.block_on(async move {
                match tokio::net::UnixListener::from_std(listener) {
                    Ok(listener) => {
                        let _ = ready_tx.send(Ok(()));
                        let serving = axum::serve(listener, app)
                            .with_graceful_shutdown(async { let _ = stopping.await; })
                            .into_future();
                        tokio::pin!(serving);
                        tokio::select! {
                            _ = &mut serving => {},
                            _ = shutdown_wait.changed() => {
                                let _ = tokio::time::timeout(Duration::from_secs(2), &mut serving).await;
                            },
                        }
                        // Axum's graceful HTTP shutdown does not wait for
                        // upgraded WebSockets. Let their shutdown selects drop
                        // the connections before destroying the blocking pool;
                        // otherwise queued reads can emit worker-failed frames.
                        if tokio::time::timeout(
                            Duration::from_secs(2),
                            sockets.acquire_many_owned(SOCKET_CAPACITY),
                        ).await.is_err() {
                            eprintln!("GraphQL WebSocket shutdown exceeded its two-second drain limit");
                        }
                    }
                    Err(error) => {
                        let _ = ready_tx.send(Err(error));
                    }
                }
            });
            executor.shutdown_timeout(Duration::from_secs(1));
        })?;
    ready_rx.recv()??;
    let path = directory.join("graphql.json");
    let mut server = GraphqlServer {
        endpoint,
        path,
        _socket: socket,
        shutdown,
        stop: Some(stop),
        thread: Some(thread),
    };
    server.publish()?;
    Ok(server)
}

impl GraphqlServer {
    fn publish(&mut self) -> CwrResult<()> {
        let temporary = self
            .path
            .with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
        let result = (|| {
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&temporary)?;
            file.write_all(&serde_json::to_vec(&self.endpoint)?)?;
            file.sync_all()?;
            fs::rename(&temporary, &self.path)?;
            Ok(())
        })();
        let _ = fs::remove_file(temporary);
        result
    }
}

impl Drop for GraphqlServer {
    fn drop(&mut self) {
        self.shutdown.send_replace(true);
        if LocalGraphqlEndpoint::read(&self.path)
            .is_ok_and(|value| value.token == self.endpoint.token)
        {
            let _ = fs::remove_file(&self.path);
        }
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

async fn execute(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Json(request): Json<async_graphql::Request>,
) -> Result<Json<async_graphql::Response>, StatusCode> {
    let session = authorize(&state, &headers)?;
    let request = request.data(session.service.clone()).data(session);
    let _permit = state
        .permits
        .clone()
        .try_acquire_owned()
        .map_err(|_| StatusCode::TOO_MANY_REQUESTS)?;
    let _consumer = state
        .activity
        .connect()
        .ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    tokio::time::timeout(Duration::from_secs(15), state.schema.execute(request))
        .await
        .map(Json)
        .map_err(|_| StatusCode::GATEWAY_TIMEOUT)
}

fn authorize(
    state: &HttpState,
    headers: &HeaderMap,
) -> Result<Arc<sessions::ClientSession>, StatusCode> {
    boundary(state, headers)?;
    state.sessions.authenticate(headers)
}
fn boundary(_state: &HttpState, headers: &HeaderMap) -> Result<(), StatusCode> {
    // Native local consumers only. Browser credentials and origin policy come with Desktop migration.
    if headers.contains_key("origin")
        || headers.get("host").and_then(|v| v.to_str().ok()) != Some("localhost")
    {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(())
}

fn constant_time_equal(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0u8, |difference, (a, b)| difference | (a ^ b))
        == 0
}

mod rich_messages;
