//! Assignment credentials and cwpdb-backed execution retention.
use crate::{
    adapter::{validate_adapter_invocation, AdapterEvent, AdapterInvocation},
    CwrResult,
};
use cwpdb_store::invocation::{
    Assignment, Authorizer, Context, Event, EventKind, InvocationStorage, Mutation, Operation,
    Outcome, Record,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

/// One idempotent executor publication, without bearer credentials.
#[derive(Debug, Clone, PartialEq)]
pub struct CloudCallback {
    pub request_id: String,
    pub sequence: u64,
    pub frame: Value,
}

/// Cancellation/interruption facts retained for effect reconciliation.
#[derive(Debug, Clone, PartialEq)]
pub struct CloudInterruption {
    pub cancelled: bool,
    pub uncertain_effects: Value,
}

/// The caller supplies an already authenticated coordinator context and a current
/// CWR Authorizer. Neither workspace fields nor bearer claims replace authority.
pub struct CloudCoordinator<S, A> {
    pub storage: S,
    pub authorizer: A,
    pub(super) publication_authorizer: Option<A>,
    signing_key: Vec<u8>,
}

impl<S: InvocationStorage, A: Authorizer> CloudCoordinator<S, A> {
    pub fn new(storage: S, authorizer: A, signing_key: Vec<u8>) -> CwrResult<Self> {
        if signing_key.len() < 32 {
            return Err("Cloud assignment signing key requires at least 32 bytes".into());
        }
        Ok(Self {
            storage,
            authorizer,
            publication_authorizer: None,
            signing_key,
        })
    }

    /// Publication/start reservations require active normal CWR authority;
    /// cleanup and retained reads keep the coordinator's base authority.
    pub fn with_publication_authority(mut self, authority: A) -> Self {
        self.publication_authorizer = Some(authority);
        self
    }
    pub(super) fn publication_authority(&self) -> &A {
        self.publication_authorizer
            .as_ref()
            .unwrap_or(&self.authorizer)
    }

    /// Retain the existing host-admitted invocation, not a caller-authored action.
    pub fn admit(
        &mut self,
        context: Context,
        record: &crate::v2026_09::invocation::ActionInvocationRecord,
        invocation: &AdapterInvocation,
        request_id: String,
        now: u64,
    ) -> CwrResult<Outcome> {
        validate_adapter_invocation(invocation)?;
        if record.invocation_ref != invocation.invocation_ref
            || record.action_name != invocation.action_name
            || record.input != invocation.input
            || record.state.is_terminal()
        {
            return Err("Cloud dispatch does not match an active CWR admission".into());
        }
        let conversation_ref = record
            .conversation_ref
            .clone()
            .ok_or("Cloud invocation requires a conversation")?;
        self.storage.mutate(
            &Mutation {
                context,
                invocation_ref: record.invocation_ref.clone(),
                request_id,
                expected_revision: None,
                operation: Operation::Admit {
                    conversation_ref,
                    parent_invocation_ref: record.parent_invocation_ref.clone(),
                    admitted: json!({"cwr_invocation":record,"adapter_invocation":invocation}),
                },
            },
            &self.authorizer,
            now,
        )
    }

    pub fn assign(
        &mut self,
        context: Context,
        invocation_ref: String,
        revision: String,
        assignment: Assignment,
        request_id: String,
        now: u64,
    ) -> CwrResult<Outcome> {
        self.storage.mutate(
            &Mutation {
                context,
                invocation_ref,
                request_id,
                expected_revision: Some(revision),
                operation: Operation::Assign { assignment },
            },
            &self.authorizer,
            now,
        )
    }

    pub fn existing(&mut self, context: &Context, reference: &str) -> CwrResult<Option<Record>> {
        self.storage
            .invocation(context, reference, &self.authorizer)
    }
    pub fn lookup_receipt(
        &mut self,
        context: &Context,
        reference: &str,
        request_id: &str,
    ) -> CwrResult<cwpdb_store::invocation::ReceiptLookup> {
        self.storage
            .receipt(context, reference, request_id, &self.authorizer)
    }
    pub fn retained_events(
        &mut self,
        context: &Context,
        reference: &str,
        after: Option<u64>,
        limit: usize,
    ) -> CwrResult<cwpdb_store::invocation::EventPage> {
        self.storage
            .events(context, reference, after, limit, &self.authorizer)
    }

    pub fn record(&mut self, context: &Context, invocation_ref: &str) -> CwrResult<Record> {
        self.storage
            .invocation(context, invocation_ref, &self.authorizer)?
            .ok_or_else(|| "Unknown cloud invocation".into())
    }

    /// Tokens are derived from every immutable assignment dimension. The reusable
    /// signing key is kept at the trusted coordinator and never serialized.
    pub fn assignment_token(
        &mut self,
        context: &Context,
        invocation_ref: &str,
    ) -> CwrResult<String> {
        let record = self.record(context, invocation_ref)?;
        self.token_for(&record)
    }

    fn token_for(&self, record: &Record) -> CwrResult<String> {
        let assignment = record
            .assignment
            .as_ref()
            .ok_or("Cloud invocation is not assigned")?;
        let scope = serde_json::to_vec(&json!([
            "cwr-cloud-assignment-v1",
            record.workspace_ref,
            record.conversation_ref,
            record.invocation_ref,
            record.parent_invocation_ref,
            assignment
        ]))?;
        Ok(hex::encode(hmac(&self.signing_key, &scope)))
    }

    pub(super) fn verify(&self, record: &Record, token: &str, now: u64) -> CwrResult<()> {
        let expected = self.token_for(record)?;
        let actual = hex::decode(token).map_err(|_| "Invalid cloud assignment credential")?;
        let expected = hex::decode(expected)?;
        let mismatch = actual.len() != expected.len()
            || actual
                .iter()
                .zip(&expected)
                .fold(0u8, |different, (a, b)| different | (a ^ b))
                != 0;
        if mismatch
            || record
                .assignment
                .as_ref()
                .is_none_or(|a| a.expires_at_unix_ms <= now)
        {
            return Err("Expired or foreign cloud assignment credential".into());
        }
        Ok(())
    }

    /// The exact mutation (including its revision) is retried after an uncertain
    /// acknowledgement, so the stored receipt is returned after authorization.
    pub fn publish(
        &mut self,
        context: Context,
        invocation_ref: String,
        token: &str,
        revision: String,
        callback: CloudCallback,
        now: u64,
    ) -> CwrResult<Outcome> {
        self.publication_authority()
            .authorize(&context, &invocation_ref, true)?;
        let CloudCallback {
            request_id,
            sequence,
            frame,
        } = callback;
        let record = self.record(&context, &invocation_ref)?;
        self.verify(&record, token, now)?;
        if frame["invocation_ref"] != invocation_ref {
            return Err("Foreign executor frame".into());
        }
        let parsed: AdapterEvent = serde_json::from_value(frame.clone())?;
        let admitted: AdapterInvocation =
            serde_json::from_value(record.admitted["adapter_invocation"].clone())?;
        crate::adapter::validate_cloud_callback(&admitted, &parsed)?;
        let kind = if matches!(parsed, AdapterEvent::Result { .. }) {
            EventKind::Completed
        } else {
            EventKind::Progress
        };
        let assignment = record
            .assignment
            .ok_or("Cloud invocation is not assigned")?;
        self.storage.mutate(
            &Mutation {
                context,
                invocation_ref,
                request_id,
                expected_revision: Some(revision),
                operation: Operation::Append {
                    assignment_ref: assignment.assignment_ref,
                    executor_ref: assignment.executor_ref,
                    generation: assignment.generation,
                    event: Event {
                        sequence,
                        kind,
                        payload: frame,
                    },
                },
            },
            self.publication_authorizer
                .as_ref()
                .unwrap_or(&self.authorizer),
            now,
        )
    }

    pub fn stop(
        &mut self,
        context: Context,
        invocation_ref: String,
        revision: String,
        request_id: String,
        interruption: CloudInterruption,
        now: u64,
    ) -> CwrResult<Outcome> {
        let CloudInterruption {
            cancelled,
            uncertain_effects,
        } = interruption;
        let kind = if cancelled {
            EventKind::Cancelled
        } else {
            EventKind::Interrupted
        };
        self.storage.mutate(&Mutation {context,invocation_ref,request_id,expected_revision:Some(revision),operation:Operation::Stop {
            kind,reason:json!({"status":if cancelled {499} else {500},"error":{"code":if cancelled {"cancelled"} else {"executor_interrupted"}},
                "uncertain_effects":uncertain_effects,"replay_permitted":false})}},&self.authorizer,now)
    }
}

// RFC 2104 HMAC-SHA256; no raw key or provider credential enters the token.
fn hmac(key: &[u8], message: &[u8]) -> [u8; 32] {
    let key = if key.len() > 64 {
        Sha256::digest(key).to_vec()
    } else {
        key.to_vec()
    };
    let mut inner = [0x36u8; 64];
    let mut outer = [0x5cu8; 64];
    for (index, byte) in key.iter().enumerate() {
        inner[index] ^= byte;
        outer[index] ^= byte;
    }
    let mut digest = Sha256::new();
    digest.update(inner);
    digest.update(message);
    let mut outer_digest = Sha256::new();
    outer_digest.update(outer);
    outer_digest.update(digest.finalize());
    outer_digest.finalize().into()
}

#[cfg(test)]
include!("coordinator_tests.rs");

/// Stable bounded request identity even when canonical runtime refs are long.
/// Tuple encoding preserves operation separation and exact retry identity.
pub(super) fn internal_request_id(operation: &str, identity: &str) -> String {
    format!(
        "cloud:{}",
        hex::encode(Sha256::digest(
            json!(["cwr-cloud-request-v1", operation, identity])
                .to_string()
                .as_bytes()
        ))
    )
}

/// A durable marker without its exact start/boot proof is uncertainty, never
/// permission to start again or to claim an already-started executor.
pub(super) fn executor_start_receipt_available(
    receipt: &cwpdb_store::invocation::ReceiptLookup,
) -> CwrResult<bool> {
    use cwpdb_store::invocation::ReceiptLookup;
    match receipt {
        ReceiptLookup::Retained { .. } => Ok(true),
        ReceiptLookup::NotCommitted => Ok(false),
        ReceiptLookup::CommittedResultExpired => {
            Err("Executor start receipt expired; interruption and reconciliation required".into())
        }
    }
}
