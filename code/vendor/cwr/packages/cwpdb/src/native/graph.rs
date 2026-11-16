//! Typed JSON graph operations backed by the crate's shared transaction API.
use super::{BatchResponse, NativeStore, MAX_BATCH_STATEMENTS};
use crate::CwpdbResult;
use rusqlite::{OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Typed graph writes composed with the same primitives used by Rust callers.
#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum GraphOperation {
    /// Merge one durable node's properties.
    PutNode {
        reference: String,
        kind: String,
        props: Value,
    },
    /// Insert once; duplicate records retain their original properties.
    InsertDisposableNode {
        reference: String,
        kind: String,
        props: Value,
    },
    /// Remove records and reclaim only their unused disposable identities.
    RemoveDisposables { references: Vec<String> },
}

/// A bounded set of typed graph operations, committed atomically.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GraphBatchRequest {
    /// Operations in transaction order (at most 256).
    pub operations: Vec<GraphOperation>,
}

impl NativeStore {
    /// Execute typed graph operations without exposing raw transaction control.
    pub(crate) fn graph_batch(
        &mut self,
        request: &GraphBatchRequest,
    ) -> CwpdbResult<BatchResponse> {
        if request.operations.is_empty() || request.operations.len() > MAX_BATCH_STATEMENTS {
            return Err("cwpDb graph batch requires 1 to 256 operations".into());
        }
        for operation in &request.operations {
            if let GraphOperation::RemoveDisposables { references } = operation {
                if references.len() > MAX_BATCH_STATEMENTS {
                    return Err("cwpDb graph removal exceeds 256 references".into());
                }
            }
        }
        self.graph_transaction(|tx| {
            let changes = request
                .operations
                .iter()
                .map(|operation| apply_operation(tx, operation))
                .collect::<CwpdbResult<Vec<_>>>()?;
            Ok(BatchResponse { changes })
        })
    }
}

fn apply_operation(tx: &Transaction<'_>, operation: &GraphOperation) -> CwpdbResult<u64> {
    match operation {
        GraphOperation::PutNode {
            reference,
            kind,
            props,
        } => {
            crate::put_node(tx, reference, kind, props)?;
            Ok(1)
        }
        GraphOperation::InsertDisposableNode {
            reference,
            kind,
            props,
        } => insert_disposable_node(tx, reference, kind, props),
        GraphOperation::RemoveDisposables { references } => {
            let existing = references
                .iter()
                .map(|reference| crate::find(tx, reference))
                .collect::<CwpdbResult<Vec<_>>>()?
                .into_iter()
                .flatten()
                .collect::<std::collections::BTreeSet<_>>();
            crate::remove_disposables(tx, references.iter().map(String::as_str))?;
            Ok(existing.len() as u64)
        }
    }
}

fn insert_disposable_node(
    tx: &Transaction<'_>,
    reference: &str,
    kind: &str,
    props: &Value,
) -> CwpdbResult<u64> {
    let Some(id) = crate::find(tx, reference)? else {
        crate::put_disposable_node(tx, reference, kind, props)?;
        return Ok(1);
    };
    let disposable: i64 = tx.query_row(
        "SELECT disposable FROM ref_bindings WHERE ref_id=?1",
        [id],
        |row| row.get(0),
    )?;
    if disposable == 0 {
        return Err(format!("{reference} has a durable graph identity").into());
    }
    let compatible: Option<bool> = tx.query_row(
        "SELECT kind=?2 AND schema_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?3) FROM nodes WHERE ref_id=?1",
        rusqlite::params![id, kind, crate::schema_ref(kind)], |row| row.get(0),
    ).optional()?;
    match compatible {
        Some(true) => Ok(0),
        Some(false) => Err(format!("unsupported node kind or schema for {reference}").into()),
        None => {
            crate::put_disposable_node(tx, reference, kind, props)?;
            Ok(1)
        }
    }
}
