//! Exact retained pre-upgrade invocation snapshot fixture.
use crate::CwpdbResult;
use std::path::Path;

pub fn remove_execution_snapshot_fields(path: &Path) -> CwpdbResult<()> {
    let connection = rusqlite::Connection::open(path)?;
    let rows = connection
        .prepare("SELECT r.canonical_ref, CAST(json_extract(n.props,'$.record') AS BLOB) FROM nodes n JOIN ref_bindings r ON r.ref_id=n.ref_id WHERE n.kind='activity'")
        ?
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
        })
        ?
        .collect::<rusqlite::Result<Vec<_>>>()
        ?;
    for (reference, payload) in rows {
        let mut activity: serde_json::Value = serde_json::from_slice(&payload)?;
        let Some(snapshot) = activity
            .pointer_mut("/event/invocation")
            .and_then(serde_json::Value::as_object_mut)
        else {
            continue;
        };
        snapshot.remove("retained_route");
        snapshot.remove("provider_fingerprint");
        connection
            .execute(
                "UPDATE nodes SET props=jsonb_set(props,'$.record',json(?1)) WHERE ref_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?2)",
                rusqlite::params![serde_json::to_vec(&activity)?, reference],
            )
            ?;
    }
    Ok(())
}
