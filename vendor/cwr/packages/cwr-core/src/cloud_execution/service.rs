//! Native execution service for a host's already-open normal CWR runtime.
use super::{
    CloudAssignment, CloudCallback, CloudCoordinator, CloudDispatch, CloudEvents, CloudInterruption,
};
use crate::{v2026_09::local::LocalRuntime, CwrRef, CwrResult};
use cwpdb_store::{
    invocation::{Assignment, Authorizer, Context, EventKind, Record},
    CwpdbResult,
};
use reqwest::{blocking::Client, Url};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    sync::{Mutex, MutexGuard},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CloudExecutorEndpoint {
    pub executor_ref: String,
    pub url: String,
}

/// Supplied by trusted host configuration, never by a coworker action.
pub struct CloudServiceConfig {
    pub workspace_ref: String,
    pub coordinator_actor_ref: String,
    pub coordinator_token: String,
    pub assignment_signing_key: Vec<u8>,
    pub executors: Vec<CloudExecutorEndpoint>,
    pub assignment_lifetime_ms: u64,
    pub model: Option<CloudModelGrant>,
}

pub struct CloudHttpResponse {
    pub status: u16,
    pub body: Value,
}
impl CloudHttpResponse {
    fn ok(body: Value) -> Self {
        Self { status: 200, body }
    }
}

struct LocalAuthority {
    runtime: LocalRuntime,
    context: Context,
    require_active: bool,
}
impl Authorizer for LocalAuthority {
    fn authorize(&self, context: &Context, invocation: &str, _write: bool) -> CwpdbResult<()> {
        if context != &self.context {
            return Err("Cloud workspace/caller mismatch".into());
        }
        self.runtime
            .cloud_execution_record(invocation, self.require_active)?;
        Ok(())
    }
}

pub struct CloudHttpService {
    placement: std::sync::atomic::AtomicUsize,
    runtime: LocalRuntime,
    config: CloudServiceConfig,
    runtime_ref: String,
    storage_context: Context,
    coordinator: Mutex<CloudCoordinator<super::service_store::Store, LocalAuthority>>,
    http: Client,
    event_polls: Mutex<std::collections::BTreeSet<String>>,
}

pub(super) fn trusted_origin(value: &str) -> CwrResult<Url> {
    let url = Url::parse(value)?;
    let local = matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"));
    // Fixed reserved origin intercepted by the authored Cloudflare placement.
    // This config is supplied by the host, never by an actor/action parameter.
    let private_binding =
        url.host_str() == Some("executors.internal") && url.port_or_known_default() == Some(80);
    if (url.scheme() != "https" && !(url.scheme() == "http" && (local || private_binding)))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
    {
        return Err("Executor requires HTTPS or loopback HTTP origin".into());
    }
    Ok(url)
}
pub(super) fn equal_secret(left: &str, right: &str) -> bool {
    left.len() == right.len()
        && left
            .bytes()
            .zip(right.bytes())
            .fold(0u8, |different, (a, b)| different | (a ^ b))
            == 0
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

impl CloudHttpService {
    pub fn new<S: super::service_store::CloudStorage + 'static>(
        runtime: LocalRuntime,
        storage: S,
        config: CloudServiceConfig,
    ) -> CwrResult<Self> {
        let caller_ref = config.coordinator_actor_ref.clone();
        Self::new_with_storage_caller(runtime, storage, config, caller_ref)
    }
    /// The host principal authenticating private storage is distinct from the
    /// actual actor configured in the CWR graph. Both are host-selected facts.
    pub fn new_with_storage_caller<S: super::service_store::CloudStorage + 'static>(
        runtime: LocalRuntime,
        storage: S,
        config: CloudServiceConfig,
        storage_caller_ref: String,
    ) -> CwrResult<Self> {
        CwrRef::parse(&storage_caller_ref)?;
        CwrRef::parse(&config.workspace_ref)?;
        CwrRef::parse(&config.coordinator_actor_ref)?;
        if config.coordinator_token.len() < 32
            || !(1_000..=900_000).contains(&config.assignment_lifetime_ms)
            || config.executors.is_empty()
        {
            return Err("Cloud service requires private credentials, executors and a 1..900 second assignment limit".into());
        }
        if runtime.catalog()?.acting_actor.actor_ref != config.coordinator_actor_ref {
            return Err("Cloud coordinator actor differs from actual runtime host".into());
        }
        if let Some(model) = &config.model {
            let endpoint = Url::parse(&model.endpoint)?;
            if endpoint.scheme() != "https"
                && !(endpoint.scheme() == "http"
                    && matches!(endpoint.host_str(), Some("127.0.0.1" | "localhost")))
            {
                return Err("Authorized model route requires HTTPS or loopback HTTP".into());
            }
            if !endpoint.username().is_empty()
                || endpoint.password().is_some()
                || endpoint.query().is_some()
                || endpoint.fragment().is_some()
                || model.model.is_empty()
                || !(1..=64).contains(&model.request_budget)
                || !(1..=65536).contains(&model.max_output_tokens)
            {
                return Err("Explicit valid model route and budget required".into());
            }
        }
        let mut refs = std::collections::BTreeSet::new();
        for executor in &config.executors {
            CwrRef::parse(&executor.executor_ref)?;
            trusted_origin(&executor.url)?;
            if !refs.insert(&executor.executor_ref) {
                return Err("Duplicate executor identity".into());
            }
        }
        let storage_context = Context {
            workspace_ref: config.workspace_ref.clone(),
            caller_ref: storage_caller_ref,
        };
        let authority = LocalAuthority {
            runtime: runtime.clone(),
            context: storage_context.clone(),
            require_active: false,
        };
        let publication_authority = LocalAuthority {
            runtime: runtime.clone(),
            context: storage_context.clone(),
            require_active: true,
        };
        let coordinator = CloudCoordinator::new(
            super::service_store::Store(Box::new(storage)),
            authority,
            config.assignment_signing_key.clone(),
        )?
        .with_publication_authority(publication_authority);
        let runtime_ref = runtime.graph()?.runtime_ref;
        Ok(Self {
            placement: std::sync::atomic::AtomicUsize::new(0),
            runtime,
            config,
            runtime_ref,
            storage_context,
            coordinator: Mutex::new(coordinator),
            event_polls: Mutex::new(std::collections::BTreeSet::new()),
            http: Client::builder()
                .timeout(Duration::from_secs(20))
                .redirect(reqwest::redirect::Policy::none())
                .build()?,
        })
    }
    fn lock(
        &self,
    ) -> CwrResult<MutexGuard<'_, CloudCoordinator<super::service_store::Store, LocalAuthority>>>
    {
        self.coordinator
            .lock()
            .map_err(|_| "Cloud journal lock poisoned".into())
    }
    fn context(&self) -> Context {
        self.storage_context.clone()
    }
    fn invocation_ref(&self, id: &str) -> CwrResult<String> {
        if id.is_empty()
            || id.len() > 128
            || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        {
            return Err("Invalid assignment identity".into());
        }
        Ok(format!("{}/invocations/{id}", self.runtime_ref))
    }
    fn credentials(
        &self,
        id: &str,
        authorization: &str,
        active: bool,
    ) -> CwrResult<(String, Record, CloudAssignment)> {
        let reference = self.invocation_ref(id)?;
        self.runtime.cloud_execution_record(&reference, active)?;
        let mut coordinator = self.lock()?;
        let record = coordinator.record(&self.context(), &reference)?;
        let assignment = record
            .assignment
            .as_ref()
            .ok_or("Invocation is unassigned")?;
        if assignment.assignment_ref != format!("{}/assignments/{id}", self.runtime_ref) {
            return Err("Assignment identity mismatch".into());
        }
        let token = coordinator.assignment_token(&self.context(), &reference)?;
        if !equal_secret(authorization, &format!("Bearer {token}")) {
            return Err("Invalid assignment credential".into());
        }
        coordinator.verify(&record, &token, now())?;
        let wire = CloudAssignment {
            assignment_id: id.into(),
            invocation_ref: reference.clone(),
            executor_ref: assignment.executor_ref.clone(),
            generation: assignment.generation,
            expires_at_unix_ms: assignment.expires_at_unix_ms,
            token,
        };
        Ok((reference, record, wire))
    }
    fn executor_url(&self, assignment: &CloudAssignment, operation: &str) -> CwrResult<Url> {
        let executor = self
            .config
            .executors
            .iter()
            .find(|e| e.executor_ref == assignment.executor_ref)
            .ok_or("Retained executor is not configured")?;
        let mut url = trusted_origin(&executor.url)?;
        url.path_segments_mut()
            .map_err(|_| "Invalid executor URL")?
            .extend([
                "internal",
                "executors",
                &assignment.assignment_id,
                operation,
            ]);
        Ok(url)
    }
    fn executor(
        &self,
        assignment: &CloudAssignment,
        operation: &str,
        body: Option<Value>,
        after: Option<u64>,
    ) -> CwrResult<Value> {
        let mut url = self.executor_url(assignment, operation)?;
        if let Some(after) = after {
            url.query_pairs_mut()
                .append_pair("after", &after.to_string());
        }
        let request = if let Some(body) = body {
            self.http.post(url).json(&body)
        } else {
            self.http.get(url)
        };
        let response = request
            .bearer_auth(&assignment.token)
            .header("x-cwr-assignment-id", &assignment.assignment_id)
            .send()?;
        if !response.status().is_success() {
            return Err(format!(
                "Executor operation {operation} failed ({})",
                response.status().as_u16()
            )
            .into());
        }
        use std::io::Read;
        let mut bytes = Vec::new();
        response.take(4 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
        if bytes.len() > 4 * 1024 * 1024 {
            return Err("Executor output exceeds service limit".into());
        }
        Ok(serde_json::from_slice(&bytes)?)
    }
    fn stop(&self, reference: &str, cancelled: bool, reason: &str) -> CwrResult<()> {
        let mut coordinator = self.lock()?;
        let record = coordinator.record(&self.context(), reference)?;
        if record.terminal.is_none() {
            coordinator.stop(
                self.context(),
                reference.into(),
                record.revision,
                super::coordinator::internal_request_id(
                    if cancelled { "cancel" } else { "interrupt" },
                    reference,
                ),
                CloudInterruption {
                    cancelled,
                    uncertain_effects: json!([reason]),
                },
                now(),
            )?;
        }
        Ok(())
    }
    /// Private HTTP router used by cwp-runtime's authenticated loopback listener.
    pub fn handle(
        &self,
        method: &str,
        path: &str,
        authorization: &str,
        body: Value,
    ) -> CwrResult<CloudHttpResponse> {
        if method == "POST" && path == "/v1/dispatch" {
            if !equal_secret(
                authorization,
                &format!("Bearer {}", self.config.coordinator_token),
            ) {
                return Ok(CloudHttpResponse {
                    status: 401,
                    body: json!({"error":"unauthorized"}),
                });
            }
            return Ok(CloudHttpResponse::ok(serde_json::to_value(
                self.dispatch(serde_json::from_value(body)?)?,
            )?));
        }
        if method == "POST" && path.starts_with("/internal/cloud/") {
            return self.internal(path, authorization, body);
        }
        let url = Url::parse(&format!("http://native{path}"))?;
        let segments: Vec<_> = url.path_segments().ok_or("Invalid service path")?.collect();
        if segments.len() != 4 || segments[0] != "v1" || segments[1] != "assignments" {
            return Ok(CloudHttpResponse {
                status: 404,
                body: json!({"error":"not_found"}),
            });
        }
        let (id, operation) = (segments[2], segments[3]);
        if method == "GET" && operation == "events" {
            let after = url
                .query_pairs()
                .find(|(key, _)| key == "after")
                .map(|(_, value)| value.parse::<u64>())
                .transpose()?
                .unwrap_or(0);
            return Ok(CloudHttpResponse::ok(serde_json::to_value(self.events(
                id,
                authorization,
                after,
            )?)?));
        }
        let (reference, _, assignment) =
            self.credentials(id, authorization, operation == "input")?;
        if method == "POST" && operation == "input" {
            return self.input(&reference, &assignment, body);
        }
        if method == "POST" && matches!(operation, "cancel" | "interrupt") {
            self.stop(
                &reference,
                operation == "cancel",
                "Coordinator ended execution; reconcile uncertain effects",
            )?;
            let _ = self.executor(&assignment, "cancel", Some(json!({})), None);
            let _ = self.executor(&assignment, "destroy", Some(json!({})), None);
            return Ok(CloudHttpResponse::ok(json!({"accepted":true})));
        }
        Ok(CloudHttpResponse {
            status: 404,
            body: json!({"error":"not_found"}),
        })
    }
}

include!("service_input.rs");
include!("service_internal.rs");

include!("service_dispatch.rs");

include!("service_model.rs");
