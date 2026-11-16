use super::*;
use rusqlite::{params, Connection};
pub(super) fn validate_formats(db: &Connection) -> CwpdbResult<()> {
    let tables: i64 = db.query_row(
        "SELECT count(*) FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'",
        [],
        |row| row.get(0),
    )?;
    if tables != 3 {
        return Err("WLP DB has an unsupported storage format.".into());
    }

    let mut statement = db.prepare_cached(
        "SELECT n.kind,s.canonical_ref FROM nodes n
         LEFT JOIN ref_bindings s ON s.ref_id=n.schema_id
         WHERE n.kind IN (?1,?2,?3) OR EXISTS (
           SELECT 1 FROM edges e WHERE e.target_id=n.ref_id AND e.kind IN (?4,?5))",
    )?;
    for row in statement.query_map(
        params![
            WORKSPACE_KIND,
            ENTRY_KIND,
            CLEANUP_KIND,
            MEMBERSHIP_KIND,
            CLEANUP_EDGE_KIND
        ],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
    )? {
        let (kind, schema) = row?;
        let expected = match kind.as_str() {
            WORKSPACE_KIND => WORKSPACE_SCHEMA,
            ENTRY_KIND => ENTRY_SCHEMA,
            CLEANUP_KIND => CLEANUP_SCHEMA,
            _ => return Err("WLP DB has an unsupported environment record kind.".into()),
        };
        if schema != expected {
            return Err("WLP DB has an unsupported environment record schema.".into());
        }
    }
    let mut edges = db.prepare_cached(
        "SELECT r.canonical_ref,e.kind,s.canonical_ref,src.canonical_ref,dst.canonical_ref,
                source.kind,target.kind
         FROM edges e
         LEFT JOIN ref_bindings r ON r.ref_id=e.ref_id
         LEFT JOIN ref_bindings s ON s.ref_id=e.schema_id
         LEFT JOIN ref_bindings src ON src.ref_id=e.source_id
         LEFT JOIN ref_bindings dst ON dst.ref_id=e.target_id
         LEFT JOIN nodes source ON source.ref_id=e.source_id
         LEFT JOIN nodes target ON target.ref_id=e.target_id
         WHERE e.kind IN (?1,?2)",
    )?;
    for row in edges.query_map(params![MEMBERSHIP_KIND, CLEANUP_EDGE_KIND], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, String>(5)?,
            row.get::<_, String>(6)?,
        ))
    })? {
        let (reference, kind, schema, source, target, source_kind, target_kind) = row?;
        let expected_target = if kind == MEMBERSHIP_KIND {
            ENTRY_KIND
        } else {
            CLEANUP_KIND
        };
        if schema != crate::EDGE_SCHEMA_REF
            || reference != crate::derived_ref(&kind, &[&source, &target])
            || source_kind != WORKSPACE_KIND
            || target_kind != expected_target
        {
            return Err("WLP DB has an unsupported environment relationship.".into());
        }
    }
    Ok(())
}

pub(super) fn validate_entry(record: &EntryValue) -> CwpdbResult<()> {
    let mut chars = record.name.chars();
    if record.name.len() > 128
        || !chars
            .next()
            .is_some_and(|c| c == '_' || c.is_ascii_alphabetic())
        || !chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
    {
        return Err("Invalid environment entry name.".into());
    }
    match record.kind.as_str() {
        "variable" if record.value.is_some() && record.credential_id.is_none() => {
            let value = record.value.as_deref().unwrap();
            if value.len() > 16384 || value.contains('\0') {
                return Err("Invalid environment variable value.".into());
            }
        }
        "secret" if record.value.is_none() && record.credential_id.is_some() => {
            let id = record.credential_id.as_deref().unwrap();
            if id.len() != 36
                || !id.bytes().enumerate().all(|(i, b)| {
                    if [8, 13, 18, 23].contains(&i) {
                        b == b'-'
                    } else {
                        b.is_ascii_digit() || (b'a'..=b'f').contains(&b)
                    }
                })
            {
                return Err("Invalid environment credential identity.".into());
            }
        }
        _ => return Err("Environment entry has an unknown format.".into()),
    }
    Ok(())
}
