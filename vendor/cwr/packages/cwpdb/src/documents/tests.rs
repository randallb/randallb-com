use super::*;
use crate::NativeStore;
use serde_json::json;
struct Allow;
impl Authorizer for Allow {
    fn authorize(&self, _: &Context, _: &str, _: bool) -> CwpdbResult<()> {
        Ok(())
    }
}
struct Check;
impl Validator for Check {
    fn validate(
        &self,
        _: &str,
        _: Option<&Map<String, Value>>,
        next: &Map<String, Value>,
    ) -> CwpdbResult<()> {
        if next.get("reject") == Some(&Value::Bool(true)) {
            Err("invalid schema value".into())
        } else {
            Ok(())
        }
    }
}
fn request(id: &str, revision: Option<String>, operation: Operation) -> Mutation {
    Mutation {
        context: Context {
            workspace_ref: "cwp://test/workspaces/one".into(),
            caller_ref: "cwp://test/actors/one".into(),
        },
        reference: "cwp://test/documents/one".into(),
        request_id: id.into(),
        expected_revision: revision,
        operation,
    }
}
fn create() -> Mutation {
    request(
        "create",
        None,
        Operation::Create {
            schema_ref: "cwp://test/schemas/family".into(),
            value: json!({"record":{"name":"family","old":true}})
                .as_object()
                .unwrap()
                .clone(),
        },
    )
}
fn record(outcome: Outcome) -> Record {
    match outcome {
        Outcome::Committed {
            record: Some(record),
            ..
        } => record,
        _ => panic!("record expected"),
    }
}
#[test]
fn conditional_replace_is_atomic_replayable_and_reopens() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("store.db");
    let mut store = NativeStore::open(&path).unwrap();
    let first = record(store.mutate_document(&create(), &Allow, &Check).unwrap());
    let replace = request(
        "replace",
        Some(first.revision.clone()),
        Operation::Replace {
            value: json!({"record":{"name":"corrected"}})
                .as_object()
                .unwrap()
                .clone(),
        },
    );
    let changed = record(store.mutate_document(&replace, &Allow, &Check).unwrap());
    assert_ne!(changed.revision, first.revision);
    assert!(changed.value["record"].get("old").is_none());
    assert_eq!(
        record(store.mutate_document(&replace, &Allow, &Check).unwrap()),
        changed
    );
    let stale = request(
        "stale",
        Some(first.revision),
        Operation::Replace {
            value: json!({"wrong":true}).as_object().unwrap().clone(),
        },
    );
    assert!(store
        .mutate_document(&stale, &Allow, &Check)
        .unwrap_err()
        .to_string()
        .contains("revision_conflict"));
    drop(store);
    let mut store = NativeStore::open(&path).unwrap();
    assert_eq!(
        store
            .get_document(&replace.context, &replace.reference, &Allow)
            .unwrap(),
        Some(changed)
    );
}
#[test]
fn deletion_erases_receipts_without_resurrection() {
    let mut store = NativeStore::open_in_memory().unwrap();
    let create = create();
    let first = record(store.mutate_document(&create, &Allow, &Check).unwrap());
    let delete = request("delete", Some(first.revision), Operation::Delete);
    store.mutate_document(&delete, &Allow, &Check).unwrap();
    assert_eq!(
        store.mutate_document(&create, &Allow, &Check).unwrap(),
        Outcome::CommittedResultExpired
    );
    let mut again = create.clone();
    again.request_id = "recreate".into();
    assert!(store
        .mutate_document(&again, &Allow, &Check)
        .unwrap_err()
        .to_string()
        .contains("retired"));
    assert!(store
        .get_document(&create.context, &create.reference, &Allow)
        .unwrap()
        .is_none());
    assert!(matches!(
        store.mutate_document(&delete, &Allow, &Check).unwrap(),
        Outcome::Committed { record: None, .. }
    ));
}
#[test]
fn validation_rolls_back_receipt_and_value_and_foreign_scope_cannot_read() {
    let mut store = NativeStore::open_in_memory().unwrap();
    let create = create();
    let first = record(store.mutate_document(&create, &Allow, &Check).unwrap());
    let mut bad = request(
        "fixable",
        Some(first.revision.clone()),
        Operation::Replace {
            value: json!({"reject":true}).as_object().unwrap().clone(),
        },
    );
    assert!(store.mutate_document(&bad, &Allow, &Check).is_err());
    bad.operation = Operation::Replace {
        value: json!({"corrected":true}).as_object().unwrap().clone(),
    };
    let good = record(store.mutate_document(&bad, &Allow, &Check).unwrap());
    assert_ne!(good.revision, first.revision);
    let mut context = create.context;
    context.workspace_ref = "cwp://test/workspaces/other".into();
    assert!(store
        .get_document(&context, &create.reference, &Allow)
        .unwrap()
        .is_none());
}
