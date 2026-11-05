use super::*;
use serde_json::json;
use std::{
    ffi::{CStr, CString},
    ptr,
};

fn query(store: &mut NativeStore, sql: &str, params: Vec<Value>) -> QueryResponse {
    store
        .query(&QueryRequest {
            sql: sql.into(),
            params,
            max_rows: None,
        })
        .unwrap()
}

#[test]
fn graph_batches_deduplicate_atomically_and_reclaim_physical_identities() {
    let mut store = NativeStore::open_in_memory().unwrap();
    let event = "cwp://example/events/one";
    let insert = |value| GraphOperation::InsertDisposableNode {
        reference: event.into(),
        kind: "usage_event".into(),
        props: json!({"record":{"value":value}}),
    };
    let response = store
        .graph_batch(&GraphBatchRequest {
            operations: vec![insert(1), insert(2)],
        })
        .unwrap();
    assert_eq!(response.changes, vec![1, 0]);
    assert_eq!(
        query(
            &mut store,
            "SELECT json_extract(props,'$.record.value') FROM nodes WHERE kind='usage_event'",
            vec![]
        )
        .rows,
        vec![vec![json!(1)]]
    );
    // A durable collision fails the whole transaction, including earlier writes.
    let durable = "cwp://example/durable";
    store
        .graph_batch(&GraphBatchRequest {
            operations: vec![GraphOperation::PutNode {
                reference: durable.into(),
                kind: "state".into(),
                props: json!({}),
            }],
        })
        .unwrap();
    assert!(store
        .graph_batch(&GraphBatchRequest {
            operations: vec![
                GraphOperation::InsertDisposableNode {
                    reference: "cwp://example/events/rolled-back".into(),
                    kind: "usage_event".into(),
                    props: json!({})
                },
                GraphOperation::InsertDisposableNode {
                    reference: durable.into(),
                    kind: "state".into(),
                    props: json!({})
                },
            ]
        })
        .is_err());
    assert_eq!(query(&mut store, "SELECT COUNT(*) FROM ref_bindings WHERE canonical_ref='cwp://example/events/rolled-back'", vec![]).rows, vec![vec![json!(0)]]);
    store
        .graph_batch(&GraphBatchRequest {
            operations: vec![GraphOperation::RemoveDisposables {
                references: vec![event.into()],
            }],
        })
        .unwrap();
    assert_eq!(
        query(
            &mut store,
            "SELECT COUNT(*) FROM ref_bindings WHERE disposable=1",
            vec![]
        )
        .rows,
        vec![vec![json!(0)]]
    );
    assert_eq!(
        query(
            &mut store,
            "SELECT COUNT(*) FROM nodes WHERE kind='state'",
            vec![]
        )
        .rows,
        vec![vec![json!(1)]]
    );
}

#[test]
fn graph_ffi_requires_response_channel_before_committing() {
    let path = CString::new(":memory:").unwrap();
    let mut handle = ptr::null_mut();
    let mut error = ptr::null_mut();
    // SAFETY: valid C string and writable output pointers.
    let status = unsafe { cwpdb_open(path.as_ptr(), &mut handle, &mut error) };
    assert_eq!(status, FFI_OK);
    let request = CString::new(r#"{"operations":[{"operation":"insert_disposable_node","reference":"cwp://example/event","kind":"event","props":{}}]}"#).unwrap();
    // SAFETY: valid handle and C string; null result is deliberately rejected.
    let status =
        unsafe { cwpdb_graph_batch(handle, request.as_ptr(), ptr::null_mut(), &mut error) };
    assert_eq!(status, FFI_ERROR);
    // SAFETY: only the native boundary allocated the error string.
    unsafe { cwpdb_string_free(error) };
    let query = CString::new(r#"{"sql":"SELECT COUNT(*) FROM nodes"}"#).unwrap();
    let mut response = ptr::null_mut();
    // SAFETY: valid handle, JSON request, and writable outputs.
    let status = unsafe { cwpdb_query(handle, query.as_ptr(), &mut response, &mut error) };
    assert_eq!(status, FFI_OK);
    // SAFETY: the native response is a live NUL-terminated string.
    let result: Value =
        serde_json::from_str(unsafe { CStr::from_ptr(response) }.to_str().unwrap()).unwrap();
    assert_eq!(result["rows"], json!([[0]]));
    // SAFETY: free each native string and close the live handle exactly once.
    unsafe {
        cwpdb_string_free(response);
        cwpdb_close(handle);
    }
}
