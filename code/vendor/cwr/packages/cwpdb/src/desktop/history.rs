//! Local usage operations; native handle, privacy and physical quotas stay host-owned.
use super::{OperationError, OperationErrorCode};
use crate::{declared::Catalog, usage::UsageLedger, NativeStore};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
#[derive(Debug, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    Initialize,
    Append {
        records: Vec<Value>,
        request_id: String,
    },
    Page {
        event: Option<String>,
        control: Option<String>,
        session: Option<String>,
        before: Option<i64>,
    },
    Raw {
        uuid: String,
    },
    Account,
    Expire {
        cutoff: i64,
        after: Option<i64>,
    },
    Evict {
        limit: usize,
    },
    Clear,
}
#[derive(Debug, Serialize)]
pub struct Response {
    pub result: Value,
}
// Page and raw enforce expiration in the owning transaction.
pub fn is_write(request: &Request) -> bool {
    !matches!(request, Request::Account)
}
pub fn execute(
    store: &mut NativeStore,
    declarations: &Catalog,
    request: Request,
) -> Result<Response, OperationError> {
    let ledger = UsageLedger::new(declarations).map_err(|_| OperationError {
        code: OperationErrorCode::StorageFailure,
    })?;
    let writing = is_write(&request);
    let work = |tx: &rusqlite::Transaction<'_>| {
        let result = match request {
            Request::Initialize => {
                ledger.initialize_in_transaction(tx)?;
                json!({"clearing":ledger.clearing_in_transaction(tx)?})
            }
            Request::Append {
                records,
                request_id,
            } => serde_json::to_value(ledger.append_once_in_transaction(
                tx,
                &request_id,
                &records,
            )?)?,
            Request::Page {
                event,
                control,
                session,
                before,
            } => ledger.inspect_page_in_transaction(
                tx,
                [event.as_deref(), control.as_deref(), session.as_deref()],
                before,
            )?,
            Request::Raw { uuid } => json!(ledger.raw_in_transaction(tx, &uuid)?),
            Request::Account => {
                let totals = ledger.account_in_transaction(tx)?;
                json!({"rows":totals.rows,"bytes":totals.bytes,"complete":true})
            }
            Request::Expire { cutoff, after } => {
                serde_json::to_value(ledger.expire_batch_in_transaction(tx, cutoff, after)?)?
            }
            Request::Evict { limit } => {
                serde_json::to_value(ledger.evict_oldest_batch_in_transaction(tx, limit)?)?
            }
            Request::Clear => serde_json::to_value(ledger.clear_batch_in_transaction(tx)?)?,
        };
        Ok(Response { result })
    };
    let result = if writing {
        store.graph_transaction(work)
    } else {
        store.graph_read_transaction(work)
    };
    result.map_err(super::domain_errors::map)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn execute_fixture(
        store: &mut NativeStore,
        request: Request,
    ) -> Result<Response, OperationError> {
        execute(store, &super::super::test_catalog().unwrap(), request)
    }
    #[test]
    fn module_append_replay_clear_and_reopen_preserve_high_water() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("usage");
        let mut store = NativeStore::open(&path).unwrap();
        execute_fixture(&mut store, Request::Initialize).unwrap();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        let event = json!({"uuid":"00000000-0000-0000-0000-000000000001","event":"navigation","sdkTime":"2026-10-22T00:00:00Z","json":"{}","bytes":2,"control":null,"receiptMs":now,"hostSession":"host","renderer":"window","appVersion":"test"});
        let request = json!({"operation":"append","records":[event],"request_id":"append-request"});
        let first = execute_fixture(&mut store, serde_json::from_value(request.clone()).unwrap())
            .unwrap()
            .result;
        assert_eq!(
            execute_fixture(&mut store, serde_json::from_value(request.clone()).unwrap())
                .unwrap()
                .result,
            first
        );
        assert_eq!(
            execute_fixture(&mut store, Request::Account)
                .unwrap()
                .result,
            json!({"rows":1,"bytes":2,"complete":true})
        );
        assert_eq!(
            execute_fixture(&mut store, Request::Clear).unwrap().result["complete"],
            true
        );
        drop(store);
        let mut store = NativeStore::open(&path).unwrap();
        execute_fixture(&mut store, Request::Initialize).unwrap();
        assert_eq!(
            execute_fixture(&mut store, serde_json::from_value(request).unwrap())
                .unwrap()
                .result["outcome"],
            "committed_result_expired"
        );
        assert_eq!(
            execute_fixture(&mut store, Request::Account)
                .unwrap()
                .result["rows"],
            0
        );
    }
}
