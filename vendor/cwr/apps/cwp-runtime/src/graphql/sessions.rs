//! Local-operator bootstrap credentials issue actor-bound, connection-owned clients.
use super::{boundary, constant_time_equal, schema::ReadService, HttpState};
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    Json,
};
use cwr::{v2026_09::upstream::UpstreamEvent, CwrResult};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use tokio::sync::{mpsc, watch};

pub(super) struct Sessions {
    pub base: ReadService,
    clients: Mutex<BTreeMap<String, Arc<ClientSession>>>,
    pub changes: watch::Sender<()>,
}

pub(super) struct ClientSession {
    pub id: String,
    pub service: ReadService,
    closed: AtomicBool,
    attached: AtomicBool,
    listening: AtomicBool,
    lifecycle: Mutex<()>,
    last_used: Mutex<Instant>,
    pub shutdown: watch::Sender<bool>,
    pub events: mpsc::Sender<UpstreamEvent>,
    receiver: Mutex<Option<mpsc::Receiver<UpstreamEvent>>>,
    changes: watch::Sender<()>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct OpenSession {
    actor_ref: Option<String>,
    ingress_integration_ref: Option<String>,
    upstream_provider_session_ref: Option<String>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Credential {
    id: String,
    actor_ref: String,
    token: String,
}

impl Sessions {
    pub fn new(base: ReadService, changes: watch::Sender<()>) -> Self {
        Self {
            base,
            clients: Mutex::new(BTreeMap::new()),
            changes,
        }
    }
    fn prune(&self) {
        let removed = {
            let mut clients = self.clients.lock().unwrap();
            let expired = clients
                .iter()
                .filter(|(_, s)| {
                    !s.alive()
                        || (!s.attached.load(Ordering::SeqCst)
                            && s.last_used.lock().unwrap().elapsed() > Duration::from_secs(60))
                })
                .map(|(key, _)| key.clone())
                .collect::<Vec<_>>();
            expired
                .into_iter()
                .filter_map(|key| clients.remove(&key))
                .collect::<Vec<_>>()
        };
        for session in removed {
            session.close();
        }
    }
    pub fn authenticate(&self, headers: &HeaderMap) -> Result<Arc<ClientSession>, StatusCode> {
        self.prune();
        let supplied = headers
            .get("authorization")
            .and_then(|h| h.to_str().ok())
            .and_then(|h| h.strip_prefix("Bearer "))
            .ok_or(StatusCode::UNAUTHORIZED)?;
        let clients = self.clients.lock().unwrap();
        let session = clients
            .iter()
            .find(|(token, _)| constant_time_equal(token.as_bytes(), supplied.as_bytes()))
            .map(|(_, s)| s.clone())
            .ok_or(StatusCode::UNAUTHORIZED)?;
        session
            .ensure_open()
            .map_err(|_| StatusCode::UNAUTHORIZED)?;
        *session.last_used.lock().unwrap() = Instant::now();
        Ok(session)
    }
    fn issue(&self, actor: &str, ingress: String) -> Result<Credential, StatusCode> {
        self.issue_scoped(actor, ingress, None)
    }
    fn issue_scoped(
        &self,
        actor: &str,
        ingress: String,
        provider: Option<&str>,
    ) -> Result<Credential, StatusCode> {
        self.base
            .runtime
            .catalog_as(actor)
            .map_err(|_| StatusCode::FORBIDDEN)?;
        self.prune();
        let mut clients = self.clients.lock().unwrap();
        if clients.len() >= 128 {
            return Err(StatusCode::TOO_MANY_REQUESTS);
        }
        let token = format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        let id = format!(
            "{}/client-sessions/{}",
            self.base.runtime_ref,
            uuid::Uuid::new_v4().simple()
        );
        let mut service = self.base.clone();
        service.actor_ref = actor.into();
        service.ingress = ingress;
        service.owner = Some(match provider {
            Some(provider) => self
                .base
                .runtime
                .upstream_provider_scope(provider)
                .map_err(|_| StatusCode::FORBIDDEN)?,
            None => id.clone(),
        });
        let (events, receiver) = mpsc::channel(64);
        let session = Arc::new(ClientSession {
            id: id.clone(),
            service,
            closed: AtomicBool::new(false),
            attached: AtomicBool::new(false),
            listening: AtomicBool::new(false),
            lifecycle: Mutex::new(()),
            last_used: Mutex::new(Instant::now()),
            shutdown: watch::channel(false).0,
            events,
            receiver: Mutex::new(Some(receiver)),
            changes: self.changes.clone(),
        });
        clients.insert(token.clone(), session);
        Ok(Credential {
            id,
            actor_ref: actor.into(),
            token,
        })
    }
}
impl ClientSession {
    pub fn alive(&self) -> bool {
        !self.closed.load(Ordering::SeqCst)
    }
    pub fn ensure_open(&self) -> CwrResult<()> {
        if !self.alive() {
            return Err(std::io::Error::other("client session is closed").into());
        }
        Ok(())
    }
    pub fn attach(&self) -> CwrResult<()> {
        self.ensure_open()?;
        if self.attached.swap(true, Ordering::SeqCst) {
            return Err(
                std::io::Error::other("session already owns a WebSocket connection").into(),
            );
        }
        Ok(())
    }
    pub fn listen(&self) -> CwrResult<mpsc::Receiver<UpstreamEvent>> {
        self.ensure_open()?;
        if !self.attached.load(Ordering::SeqCst) {
            return Err(std::io::Error::other("terminal delivery requires a WebSocket").into());
        }
        let receiver = self
            .receiver
            .lock()
            .unwrap()
            .take()
            .ok_or_else(|| std::io::Error::other("terminal events already subscribed"))?;
        self.listening.store(true, Ordering::SeqCst);
        Ok(receiver)
    }
    pub fn terminal_operation<T>(&self, operation: impl FnOnce() -> CwrResult<T>) -> CwrResult<T> {
        let _lifecycle = self.lifecycle.lock().unwrap();
        self.ensure_open()?;
        if !self.listening.load(Ordering::SeqCst) {
            return Err(std::io::Error::other(
                "subscribe to terminalEvents before registering or enabling a provider",
            )
            .into());
        }
        let value = operation()?;
        self.changes.send_replace(());
        Ok(value)
    }
    pub fn notify_changes(&self) {
        self.changes.send_replace(());
    }
    pub fn close(&self) {
        let _lifecycle = self.lifecycle.lock().unwrap();
        if !self.closed.swap(true, Ordering::SeqCst) {
            self.shutdown.send_replace(true);
            let _ = self.service.runtime.disconnect_upstream_provider(&self.id);
            self.changes.send_replace(());
        }
    }
}
impl Drop for ClientSession {
    fn drop(&mut self) {
        self.close();
    }
}
/// The guard belongs to the WebSocket or terminal delivery stream, never to one read.
pub(super) struct SessionGuard(pub Arc<ClientSession>);
impl Drop for SessionGuard {
    fn drop(&mut self) {
        self.0.close();
    }
}

pub(super) async fn open(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Json(input): Json<OpenSession>,
) -> Result<Json<Credential>, StatusCode> {
    boundary(&state, &headers)?;
    let expected = format!("Bearer {}", state.endpoint.token);
    if !headers
        .get("authorization")
        .is_some_and(|h| constant_time_equal(h.as_bytes(), expected.as_bytes()))
    {
        return Err(StatusCode::UNAUTHORIZED);
    }
    // This credential is held by the trusted local OS operator, who already controls
    // actor selection through the native bootstrap. An actor ref without it grants nothing.
    let actor = input
        .actor_ref
        .unwrap_or_else(|| state.endpoint.actor_ref.clone());
    let ingress = input
        .ingress_integration_ref
        .unwrap_or_else(|| state.sessions.base.ingress.clone());
    let reference = cwr::CwrRef::parse(&ingress).map_err(|_| StatusCode::BAD_REQUEST)?;
    if ingress.starts_with("~@") || reference.fragment().is_some() {
        return Err(StatusCode::BAD_REQUEST);
    }
    tokio::task::spawn_blocking(move || match input.upstream_provider_session_ref {
        Some(provider) => state
            .sessions
            .issue_scoped(&actor, ingress, Some(&provider)),
        None => state.sessions.issue(&actor, ingress),
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map(Json)
}
pub(super) async fn close(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Result<StatusCode, StatusCode> {
    let session = super::authorize(&state, &headers)?;
    tokio::task::spawn_blocking(move || session.close())
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(StatusCode::NO_CONTENT)
}
