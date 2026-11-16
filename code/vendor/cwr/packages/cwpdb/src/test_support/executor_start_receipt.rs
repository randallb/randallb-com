//! Expire only an existing executor-start reservation's disposable receipt proof.
use crate::{invocation::Context, CwpdbResult, NativeStore};
use rusqlite::params;

/// Keep the durable request marker, journal cursor and start event, while
/// erasing the payload that proves the exact authorized worker boot.
pub fn expire_start_proof(
    store: &mut NativeStore,
    context: &Context,
    invocation: &str,
) -> CwpdbResult<()> {
    store.graph_transaction(|tx| {
        let journal = crate::derived_ref(
            "invocation_execution_journal_v1",
            &[&context.workspace_ref, invocation],
        );
        let record = crate::node_props(tx, &journal)?.ok_or("start fixture journal missing")?;
        if record["workspace_ref"] != context.workspace_ref
            || record["invocation_ref"] != invocation
        {
            return Err("start fixture scope mismatch".into());
        }
        let event = crate::derived_ref("invocation_execution_event_v1", &[&journal, "0"]);
        let value = crate::node_props(tx, &event)?.ok_or("start fixture event missing")?;
        let missing_boot = value["payload"]["worker_boot_id"]
            .as_str()
            .is_none_or(str::is_empty);
        if value["sequence"] != 0
            || value["kind"] != "progress"
            || value["payload"]["lifecycle"] != "executor_start_reserved"
            || missing_boot
        {
            return Err("fixture requires an existing executor-start reservation".into());
        }
        let mut statement = tx.prepare(
            "SELECT r.canonical_ref,json(e.props) FROM edges e
             JOIN ref_bindings r ON r.ref_id=e.ref_id
             WHERE e.target_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1)
               AND e.kind='sequence_mutation_receipt_v1' LIMIT 2",
        )?;
        let proofs = statement
            .query_map(params![event], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let [(reference, properties)] = proofs.as_slice() else {
            return Err("fixture requires exactly one retained start proof".into());
        };
        let properties: serde_json::Value = serde_json::from_str(properties)?;
        if properties["result"]["request"]["context"] != serde_json::to_value(context)?
            || properties["result"]["request"]["invocation_ref"] != invocation
        {
            return Err("start receipt fixture owner mismatch".into());
        }
        crate::remove_disposable(tx, reference)?;
        Ok(())
    })
}
