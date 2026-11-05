//! One immediate transaction protects the complete old-to-declared conversion.
use super::*;
use rusqlite::{OptionalExtension, Transaction, TransactionBehavior};
use serde_json::Map;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Stage {
    Initialized,
    Imported(usize),
    StateCreated,
    LegacyRetired,
    IndexesInstalled,
}

pub(super) fn convert(
    db: &mut Connection,
    declarations: &Declarations,
) -> CwpdbResult<MigrationReport> {
    convert_with_checkpoint(db, declarations, |_| Ok(()))
}

// Private fault injection is not a caller-supplied transaction or SQL API.
pub(super) fn convert_with_checkpoint(
    db: &mut Connection,
    declarations: &Declarations,
    mut checkpoint: impl FnMut(Stage) -> CwpdbResult<()>,
) -> CwpdbResult<MigrationReport> {
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let graph_tables = table_count(&tx, &["nodes", "edges", "ref_bindings"])?;
    let legacy_tables = table_count(&tx, &["queue", "identity", "counters"])?;
    if graph_tables != 0 && graph_tables != 3 {
        return Err("incomplete graph schema in retained outbox".into());
    }
    if legacy_tables != 0 && legacy_tables != 3 {
        return Err("incomplete legacy outbox schema".into());
    }
    if graph_tables == 0 {
        // Reject unknown source databases instead of treating them as fresh.
        let unknown: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table'
             AND name NOT LIKE 'sqlite_%' AND name NOT IN ('queue','identity','counters'))",
            [],
            |r| r.get(0),
        )?;
        if unknown {
            return Err("unrecognized retained outbox schema".into());
        }
        cwpdb::initialize(&tx)?;
    } else {
        cwpdb::upgrade(&tx)?;
    }
    checkpoint(Stage::Initialized)?;
    match cwpdb::ref_read::get_ref(&tx, STATE_REF) {
        Ok(_) => {
            let state = declarations
                .state
                .get_in_connection(&tx, STATE_DEFINITION, STATE_REF)?;
            if legacy_tables != 0 {
                return Err("completed outbox migration still has legacy tables".into());
            }
            let high_water = state.value.properties["record"]["high_water"]
                .as_i64()
                .ok_or("invalid outbox high-water")?;
            super::reads::install(&tx)?;
            super::accounting::initialize(&tx, declarations)?;
            checkpoint(Stage::IndexesInstalled)?;
            tx.commit()?;
            return Ok(MigrationReport {
                imported: 0,
                high_water,
                repeated: true,
            });
        }
        Err(cwpdb::ref_read::RefReadError::NotFound) => {}
        Err(error) => return Err(error.into()),
    }
    if graph_tables == 3 && legacy_tables == 0 {
        return Err("retained outbox graph is missing migration state".into());
    }
    // Missing state in a populated graph cannot silently reset identity/counters.
    let populated: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM nodes) OR EXISTS(SELECT 1 FROM edges)",
        [],
        |r| r.get(0),
    )?;
    if populated {
        return Err("retained outbox graph is missing migration state".into());
    }
    let (installation_id, counters, high_water, imported) = if legacy_tables == 3 {
        import_legacy(&tx, declarations, &mut checkpoint)?
    } else {
        (uuid::Uuid::new_v4().to_string(), json!({"dropped":0}), 0, 0)
    };
    declarations.state.create_in_transaction(&tx, STATE_DEFINITION, STATE_REF,
        &json!({"record":{"installation_id":installation_id,"counters":counters,"high_water":high_water,"migration_version":1}}))?;
    checkpoint(Stage::StateCreated)?;
    super::reads::install(&tx)?;
    super::accounting::initialize(&tx, declarations)?;
    checkpoint(Stage::IndexesInstalled)?;
    if legacy_tables == 3 {
        tx.execute_batch("DROP TABLE queue; DROP TABLE counters; DROP TABLE identity;")?;
    }
    checkpoint(Stage::LegacyRetired)?;
    tx.commit()?;
    Ok(MigrationReport {
        imported,
        high_water,
        repeated: false,
    })
}

fn table_count(tx: &Transaction<'_>, names: &[&str; 3]) -> CwpdbResult<usize> {
    let value: i64 = tx.query_row(
        "SELECT count(*) FROM sqlite_master WHERE type='table' AND name IN (?1,?2,?3)",
        names,
        |r| r.get(0),
    )?;
    Ok(usize::try_from(value)?)
}

fn import_legacy(
    tx: &Transaction<'_>,
    declarations: &Declarations,
    checkpoint: &mut impl FnMut(Stage) -> CwpdbResult<()>,
) -> CwpdbResult<(String, Value, i64, usize)> {
    // Reads and schema checks are inside the same startup write transaction.
    validate_legacy_schema(tx)?;
    let identities: Vec<String> = tx
        .prepare("SELECT value FROM identity LIMIT 2")?
        .query_map([], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    if identities.len() != 1 {
        return Err("legacy outbox must retain exactly one installation identity".into());
    }
    let mut counters = Map::new();
    {
        let mut stmt = tx.prepare("SELECT key,value FROM counters LIMIT 65")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
        for row in rows {
            let (key, value) = row?;
            if counters.insert(key, json!(value)).is_some() {
                return Err("duplicate retained counter".into());
            }
        }
    }
    if counters.len() > 64 || !counters.contains_key("dropped") {
        return Err("invalid legacy outbox counters".into());
    }
    let count: i64 = tx.query_row(
        "SELECT count(*) FROM (SELECT id FROM queue LIMIT 10001)",
        [],
        |r| r.get(0),
    )?;
    if count > MAX_QUEUE_EVENTS as i64 {
        return Err("retained outbox exceeds migration work limits; source preserved".into());
    }
    let (bytes, maximum): (i64, i64) = tx.query_row(
        "SELECT coalesce(sum(length(CAST(event AS BLOB))),0),coalesce(max(id),0) FROM queue",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    if bytes > MAX_QUEUE_BYTES as i64 {
        return Err("retained outbox exceeds migration work limits; source preserved".into());
    }
    // AUTOINCREMENT remembers deleted admissions. Absence of sqlite_sequence is
    // an incompatible source, not permission to reuse old integer IDs.
    let sequence: Option<i64> = tx
        .query_row(
            "SELECT seq FROM sqlite_sequence WHERE name='queue'",
            [],
            |r| r.get(0),
        )
        .optional()?;
    let high_water = maximum.max(sequence.unwrap_or(0)).max(0);
    // Reuse copied legacy pages inside the same transaction instead of
    // doubling the database quota. Rollback-journal recovery restores every
    // source row if a later write, DDL or commit fails. Never mutate a table
    // while holding its scan: each iteration is a fresh primary-key seek.
    for index in 0..usize::try_from(count)? {
        let (sequence, destination, received, event, quarantined): (i64, String, i64, String, i64) =
            tx.query_row(
                "SELECT id,destination,received,event,quarantined FROM queue ORDER BY id LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )?;
        if !matches!(quarantined, 0 | 1) {
            return Err("invalid retained quarantine state".into());
        }
        let value = event_value(sequence, received, destination, event, quarantined == 1);
        declarations.event.create_in_transaction(
            tx,
            EVENT_DEFINITION,
            &event_ref(sequence),
            &value,
        )?;
        if tx.execute("DELETE FROM queue WHERE id=?1", [sequence])? != 1 {
            return Err("retained outbox changed during conversion".into());
        }
        checkpoint(Stage::Imported(index + 1))?;
    }
    Ok((
        identities[0].clone(),
        Value::Object(counters),
        high_water,
        usize::try_from(count)?,
    ))
}

// Preserve unknown retained fields rather than treating a newer source as this
// exact legacy format and dropping its tables after copying only known columns.
type LegacyColumn = (&'static str, &'static str, i64, i64);
fn validate_legacy_schema(tx: &Transaction<'_>) -> CwpdbResult<()> {
    let layouts: &[(&str, &[LegacyColumn])] = &[
        (
            "queue",
            &[
                ("id", "INTEGER", 0, 1),
                ("destination", "TEXT", 1, 0),
                ("received", "INTEGER", 1, 0),
                ("event", "TEXT", 1, 0),
                ("quarantined", "INTEGER", 1, 0),
            ],
        ),
        ("identity", &[("value", "TEXT", 0, 1)]),
        (
            "counters",
            &[("key", "TEXT", 0, 1), ("value", "INTEGER", 1, 0)],
        ),
    ];
    for (table, expected) in layouts {
        let mut statement = tx.prepare("SELECT name,upper(type),\"notnull\",pk FROM pragma_table_info(?1) ORDER BY cid LIMIT 7")?;
        let actual: Vec<(String, String, i64, i64)> = statement
            .query_map([table], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })?
            .collect::<Result<_, _>>()?;
        let expected: Vec<_> = expected
            .iter()
            .map(|(name, kind, required, pk)| (name.to_string(), kind.to_string(), *required, *pk))
            .collect();
        if actual != expected {
            return Err("unsupported retained outbox table layout; source preserved".into());
        }
    }
    let queue_sql: String = tx.query_row(
        "SELECT sql FROM sqlite_master WHERE type='table' AND name='queue'",
        [],
        |r| r.get(0),
    )?;
    if !queue_sql.to_ascii_uppercase().contains("AUTOINCREMENT") {
        return Err("unsupported retained outbox admission identity".into());
    }
    Ok(())
}
