//! Invocation-scoped content-free projection. Storage owns the index plan.
use crate::{CwpdbResult, NativeStore, QueryRequest};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub runtime_ref: String,
    pub invocation_ref: String,
    pub limit: usize,
}
#[derive(Serialize)]
pub struct Page {
    pub observations: Vec<serde_json::Map<String, Value>>,
    pub truncated: bool,
}
impl NativeStore {
    pub fn invocation_performance(&mut self, request: &Request) -> CwpdbResult<Page> {
        if request.limit == 0
            || request.limit > 1000
            || request.runtime_ref.is_empty()
            || request.invocation_ref.is_empty()
        {
            return Err("invalid invocation performance request".into());
        }
        let result = self.query(&QueryRequest {
            sql: include_str!("projection.sql").into(),
            params: vec![json!(request.invocation_ref), json!(request.runtime_ref)],
            max_rows: Some(request.limit),
        })?;
        Ok(Page {
            observations: result
                .rows
                .into_iter()
                .map(|row| result.columns.iter().cloned().zip(row).collect())
                .collect(),
            truncated: result.truncated,
        })
    }
}
