use rusqlite::{Connection, OptionalExtension, Transaction};

use super::CwpdbResult;

/// Canonical persistent graph DDL used to classify a supported runtime store.
pub const PHYSICAL_SCHEMA_SQL: &str = include_str!("schema.sql");

/// Create the physical graph schema inside the caller's transaction.
pub fn initialize(transaction: &Transaction<'_>) -> CwpdbResult<()> {
    transaction.execute_batch(PHYSICAL_SCHEMA_SQL)?;
    migrate_ref_bindings(transaction)?;
    Ok(())
}

/// Upgrade an existing physical graph schema without recreating its tables.
///
/// NativeStore calls this after confirming the graph tables already exist.
/// It keeps all pre-existing identities durable while adding backward-
/// compatible physical columns and triggers.
pub fn upgrade(transaction: &Transaction<'_>) -> CwpdbResult<()> {
    migrate_ref_bindings(transaction)
}

/// Whether the current graph schema is installed, without taking a write lock.
pub fn schema_is_current(db: &Connection) -> CwpdbResult<bool> {
    let count: i64 = db.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE
         (type='table' AND name IN ('nodes','edges','ref_bindings')) OR
         (type='trigger' AND name IN ('ref_bindings_no_canonical_update','ref_bindings_no_ref_id_update','ref_bindings_no_disposable_downgrade','ref_bindings_no_durable_delete','ref_bindings_no_claim_update','ref_bindings_claim_pair_insert')) OR
         (type='index' AND name IN ('ref_bindings_disposable','ref_bindings_claim_owner','nodes_by_schema','nodes_schedule_due','nodes_schedule_terminal','edges_by_schema'))",
        [], |row| row.get(0),
    )?;
    let legacy: i64 = db.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='trigger' AND name IN ('ref_bindings_no_update','ref_bindings_no_delete')",
        [], |row| row.get(0),
    )?;
    let columns: i64 = db.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('ref_bindings') WHERE name IN ('disposable','claim_kind','claim_owner_ref')",
        [],
        |row| row.get(0),
    )?;
    Ok(count == 15 && legacy == 0 && columns == 3)
}

// cwpDb is pre-release, but existing local stores can predate explicit
// disposable identities. Preserve every old binding as durable while adding
// the opt-in column and replacing only the binding-protection triggers.
fn migrate_ref_bindings(transaction: &Transaction<'_>) -> CwpdbResult<()> {
    if schema_is_current(transaction)? {
        return Ok(());
    }
    let has_disposable = transaction
        .prepare_cached("PRAGMA table_info(ref_bindings)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?
        .iter()
        .any(|column| column == "disposable");
    if !has_disposable {
        transaction.execute_batch(
            "ALTER TABLE ref_bindings ADD COLUMN disposable INTEGER NOT NULL DEFAULT 0 CHECK (disposable IN (0, 1));",
        )?;
    }
    let columns = transaction
        .prepare_cached("PRAGMA table_info(ref_bindings)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?;
    if !columns.iter().any(|column| column == "claim_kind") {
        transaction.execute_batch("ALTER TABLE ref_bindings ADD COLUMN claim_kind TEXT;")?;
    }
    if !columns.iter().any(|column| column == "claim_owner_ref") {
        transaction.execute_batch("ALTER TABLE ref_bindings ADD COLUMN claim_owner_ref TEXT;")?;
    }
    transaction.execute_batch(
        "DROP TRIGGER IF EXISTS ref_bindings_no_update;
         DROP TRIGGER IF EXISTS ref_bindings_no_delete;",
    )?;
    let collision: Option<String> = transaction
        .query_row(
            "SELECT canonical_ref FROM ref_bindings
             WHERE ref_id IN (SELECT ref_id FROM nodes INTERSECT SELECT ref_id FROM edges)
             LIMIT 1",
            [],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(reference) = collision {
        return Err(format!("{reference} is both a graph node and relationship").into());
    }
    transaction.execute_batch(
        r#"
            UPDATE ref_bindings
            SET claim_kind='node:' || (SELECT kind FROM nodes WHERE nodes.ref_id=ref_bindings.ref_id),
                claim_owner_ref=NULL
            WHERE claim_kind IS NULL AND ref_id IN (SELECT ref_id FROM nodes);
            UPDATE ref_bindings
            SET claim_kind='edge:' || (SELECT kind FROM edges WHERE edges.ref_id=ref_bindings.ref_id),
                claim_owner_ref=NULL
            WHERE claim_kind IS NULL AND ref_id IN (SELECT ref_id FROM edges);
        "#,
    )?;
    transaction.execute_batch(
        r#"
            DROP TRIGGER IF EXISTS ref_bindings_no_update;
            DROP TRIGGER IF EXISTS ref_bindings_no_delete;
            DROP TRIGGER IF EXISTS ref_bindings_no_canonical_update;
            DROP TRIGGER IF EXISTS ref_bindings_no_ref_id_update;
            DROP TRIGGER IF EXISTS ref_bindings_no_disposable_downgrade;
            DROP TRIGGER IF EXISTS ref_bindings_no_durable_delete;
            DROP TRIGGER IF EXISTS ref_bindings_no_claim_update;
            DROP TRIGGER IF EXISTS ref_bindings_claim_pair_insert;
            CREATE TRIGGER ref_bindings_no_canonical_update BEFORE UPDATE OF canonical_ref ON ref_bindings
            BEGIN SELECT RAISE(ABORT, 'reference bindings are immutable'); END;
            CREATE TRIGGER ref_bindings_no_ref_id_update BEFORE UPDATE OF ref_id ON ref_bindings
            BEGIN SELECT RAISE(ABORT, 'reference bindings are immutable'); END;
            CREATE TRIGGER ref_bindings_no_disposable_downgrade BEFORE UPDATE OF disposable ON ref_bindings
            WHEN OLD.disposable=0 AND NEW.disposable!=0
            BEGIN SELECT RAISE(ABORT, 'durable reference bindings cannot become disposable'); END;
            CREATE TRIGGER ref_bindings_no_durable_delete BEFORE DELETE ON ref_bindings
            WHEN OLD.disposable=0
            BEGIN SELECT RAISE(ABORT, 'reference bindings are immutable'); END;
            CREATE TRIGGER ref_bindings_no_claim_update BEFORE UPDATE OF claim_kind, claim_owner_ref ON ref_bindings
            WHEN (NEW.claim_kind IS NULL AND NEW.claim_owner_ref IS NOT NULL)
              OR (OLD.claim_kind IS NOT NULL AND NEW.claim_kind IS NOT OLD.claim_kind)
              OR (OLD.claim_owner_ref IS NOT NULL AND NEW.claim_owner_ref IS NOT OLD.claim_owner_ref)
            BEGIN SELECT RAISE(ABORT, 'identity claims are immutable'); END;
            CREATE TRIGGER ref_bindings_claim_pair_insert BEFORE INSERT ON ref_bindings
            WHEN NEW.claim_kind IS NULL AND NEW.claim_owner_ref IS NOT NULL
            BEGIN SELECT RAISE(ABORT, 'identity claim owners require a claim kind'); END;
            CREATE INDEX IF NOT EXISTS ref_bindings_disposable ON ref_bindings(disposable, ref_id);
            CREATE INDEX IF NOT EXISTS ref_bindings_claim_owner ON ref_bindings(claim_owner_ref, claim_kind, ref_id);
            CREATE INDEX IF NOT EXISTS nodes_by_schema ON nodes(schema_id, ref_id);
            CREATE INDEX IF NOT EXISTS nodes_schedule_due ON nodes(
                json_extract(props, '$.record.schedule_ref'),
                json_extract(props, '$.record.due_unix_ms') DESC,
                ref_id DESC
            ) WHERE kind='schedule_occurrence';
            CREATE INDEX IF NOT EXISTS nodes_schedule_terminal ON nodes(
                json_extract(props, '$.record.schedule_ref'), ref_id
            ) WHERE kind='schedule_occurrence'
              AND substr(json_extract(props, '$.record.outcome'), 1, 9)='terminal_'
              AND json_extract(props, '$.record.archived')=0;
            CREATE INDEX IF NOT EXISTS edges_by_schema ON edges(schema_id, ref_id);
        "#,
    )?;
    Ok(())
}
