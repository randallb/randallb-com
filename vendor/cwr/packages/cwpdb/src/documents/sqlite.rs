use super::*;
use crate::{sequence_receipt, NativeStore};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde_json::json;
use sha2::{Digest, Sha256};
const KIND: &str = "cloud_document_v1";
fn valid_ref(reference: &str) -> CwpdbResult<()> {
    if !reference.starts_with("cwp://") || reference.len() > 4096 || reference.contains('#') {
        return Err("invalid canonical storage reference".into());
    }
    Ok(())
}
fn validate_context(context: &Context, reference: &str) -> CwpdbResult<()> {
    valid_ref(&context.workspace_ref)?;
    valid_ref(&context.caller_ref)?;
    valid_ref(reference)
}
fn token(reference: &str, revision: i64) -> String {
    hex::encode(Sha256::digest(
        json!([reference, revision]).to_string().as_bytes(),
    ))
}
fn read(db: &Connection, context: &Context, reference: &str) -> CwpdbResult<Option<Record>> {
    let row:Option<(String,String,i64)>=db.query_row("SELECT s.canonical_ref,json(json_extract(n.props,'$.record')),n.revision FROM nodes n JOIN ref_bindings r ON r.ref_id=n.ref_id JOIN ref_bindings s ON s.ref_id=n.schema_id WHERE r.canonical_ref=?1 AND r.claim_owner_ref=?2 AND n.kind=?3",params![reference,context.workspace_ref,KIND],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
    row.map(|(schema_ref, value, revision)| {
        Ok(Record {
            reference: reference.into(),
            schema_ref,
            value: serde_json::from_str(&value)?,
            revision: token(reference, revision),
        })
    })
    .transpose()
}
impl DocumentStorage for NativeStore {
    fn reconcile_document(
        &mut self,
        request: &Mutation,
        authorizer: &dyn Authorizer,
    ) -> CwpdbResult<Option<Outcome>> {
        validate_context(&request.context, &request.reference)?;
        let arguments = serde_json::to_value(request)?;
        let scope = crate::derived_ref(
            "declared_mutation_scope",
            &[&request.context.workspace_ref, &request.context.caller_ref],
        );
        self.graph_read(|db| {
            authorizer.authorize(&request.context, &request.reference, true)?;
            match sequence_receipt::check(db, &scope, &request.request_id, &arguments)? {
                sequence_receipt::Replay::NotCommitted => Ok(None),
                sequence_receipt::Replay::CommittedResultExpired => {
                    Ok(Some(Outcome::CommittedResultExpired))
                }
                sequence_receipt::Replay::Retained(value) => {
                    Ok(Some(serde_json::from_value(value)?))
                }
            }
        })
    }
    fn get_document(
        &mut self,
        context: &Context,
        reference: &str,
        authorizer: &dyn Authorizer,
    ) -> CwpdbResult<Option<Record>> {
        validate_context(context, reference)?;
        self.graph_read(|db| {
            authorizer.authorize(context, reference, false)?;
            read(db, context, reference)
        })
    }
    fn mutate_document(
        &mut self,
        request: &Mutation,
        authorizer: &dyn Authorizer,
        validator: &dyn Validator,
    ) -> CwpdbResult<Outcome> {
        validate_context(&request.context, &request.reference)?;
        let arguments = serde_json::to_value(request)?;
        if serde_json::to_vec(&arguments)?.len() > 524_288 {
            return Err("document mutation payload exceeds 512 KiB".into());
        }
        let scope = crate::derived_ref(
            "declared_mutation_scope",
            &[&request.context.workspace_ref, &request.context.caller_ref],
        );
        self.graph_transaction(|tx| {
            authorizer.authorize(&request.context, &request.reference, true)?;
            match sequence_receipt::check(tx, &scope, &request.request_id, &arguments)? {
                sequence_receipt::Replay::Retained(result) => {
                    return Ok(serde_json::from_value(result)?)
                }
                sequence_receipt::Replay::CommittedResultExpired => {
                    return Ok(Outcome::CommittedResultExpired)
                }
                sequence_receipt::Replay::NotCommitted => {}
            }
            let previous = read(tx, &request.context, &request.reference)?;
            let outcome = apply(tx, request, previous, validator)?;
            sequence_receipt::commit(
                tx,
                &scope,
                &request.request_id,
                &arguments,
                &serde_json::to_value(&outcome)?,
                std::slice::from_ref(&request.reference),
            )?;
            Ok(outcome)
        })
    }
}
fn apply(
    tx: &Transaction<'_>,
    request: &Mutation,
    previous: Option<Record>,
    validator: &dyn Validator,
) -> CwpdbResult<Outcome> {
    let (schema_ref, next) = match &request.operation {
        Operation::Create { schema_ref, value } => {
            valid_ref(schema_ref)?;
            if request.expected_revision.is_some() {
                return Err("create cannot name an expected revision".into());
            }
            if previous.is_some() {
                return Err("already_exists".into());
            }
            if crate::unordered::retired(tx, &request.reference)? {
                return Err("ref_retired_or_unavailable".into());
            }
            crate::claim_identity(
                tx,
                &request.reference,
                &format!("node:{KIND}"),
                &request.context.workspace_ref,
            )?;
            (schema_ref.clone(), value.clone())
        }
        operation => {
            let previous = previous.as_ref().ok_or("not_found")?;
            if request.expected_revision.as_ref() != Some(&previous.revision) {
                return Err("revision_conflict".into());
            }
            match operation {
                Operation::Replace { value } => (previous.schema_ref.clone(), value.clone()),
                Operation::Patch { patch } => {
                    let mut merged = Value::Object(previous.value.clone());
                    merge(&mut merged, &Value::Object(patch.clone()));
                    (
                        previous.schema_ref.clone(),
                        merged.as_object().ok_or("invalid object")?.clone(),
                    )
                }
                Operation::Delete => {
                    let id = crate::find(tx, &request.reference)?.ok_or("not_found")?;
                    let relationships:i64=tx.query_row("SELECT count(*) FROM edges WHERE (source_id=?1 OR target_id=?1) AND kind!='sequence_mutation_receipt_v1'",[id],|r|r.get(0))?;
                    if relationships > 0 {
                        return Err("relationships_exist".into());
                    }
                    crate::remove(tx, &request.reference)?;
                    return Ok(Outcome::Committed {
                        record: None,
                        deleted_revision: Some(previous.revision.clone()),
                    });
                }
                Operation::Create { .. } => unreachable!(),
            }
        }
    };
    validator.validate(&schema_ref, previous.as_ref().map(|r| &r.value), &next)?;
    if previous.as_ref().is_none_or(|record| record.value != next) {
        crate::put_node_with_schema(
            tx,
            &request.reference,
            KIND,
            &schema_ref,
            &json!({"record":next}),
        )?;
    }
    Ok(Outcome::Committed {
        record: read(tx, &request.context, &request.reference)?,
        deleted_revision: None,
    })
}
fn merge(target: &mut Value, patch: &Value) {
    let Some(patch) = patch.as_object() else {
        *target = patch.clone();
        return;
    };
    if !target.is_object() {
        *target = json!({});
    }
    let target = target.as_object_mut().unwrap();
    for (key, value) in patch {
        if value.is_null() {
            target.remove(key);
        } else {
            merge(target.entry(key).or_insert(Value::Null), value);
        }
    }
}
