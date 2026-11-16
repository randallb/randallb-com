//! Narrow native fixture preserves the real large-history/query-plan regression.
use super::*;
pub(super) fn seed_and_verify(
    tx: &Transaction<'_>,
    schedule: &Installed,
    count: usize,
) -> CwpdbResult<String> {
    if count == 0 || count > 25000 {
        return Err("schedule history fixture requires1to25000records".into());
    }
    let source_id = crate::bind(tx, &schedule.schedule_ref)?;
    let schema_id = crate::bind(
        tx,
        "cwp://org.coworkerprotocol/cwpdb/schemas/schedule_occurrence/2026-09",
    )?;
    let edge_schema = crate::bind(tx, crate::EDGE_SCHEMA_REF)?;
    let mut newest = String::new();
    for index in 0..count {
        let due = 10000 + index as u64 * 2000;
        let reference = identity(
            "schedule-occurrences",
            (
                &schedule.schedule_ref,
                &schedule.definition["revision"],
                due,
            ),
        )?;
        let terminal = index + 1 == count;
        let occurrence = Occurrence {
            occurrence_ref: reference.clone(),
            schedule_ref: schedule.schedule_ref.clone(),
            definition: schedule.definition.clone(),
            due_unix_ms: due,
            outcome: if terminal {
                "terminal_stopped"
            } else {
                "skipped_offline"
            }
            .into(),
            invocation_ref: None,
            conversation_ref: None,
            archived: !terminal,
        };
        tx.execute(
            "INSERT INTO ref_bindings(canonical_ref) VALUES(?1)",
            [&reference],
        )?;
        let target = tx.last_insert_rowid();
        tx.execute("INSERT INTO nodes(ref_id,kind,schema_id,revision,props) VALUES(?1,'schedule_occurrence',?2,0,jsonb(?3))",rusqlite::params![target,schema_id,serde_json::to_string(&json!({"record":occurrence}))?])?;
        let edge_ref = crate::unordered::desktop_ref(
            "schedule_occurrence",
            &json!([schedule.schedule_ref, reference]),
        );
        tx.execute(
            "INSERT INTO ref_bindings(canonical_ref) VALUES(?1)",
            [&edge_ref],
        )?;
        let edge = tx.last_insert_rowid();
        tx.execute("INSERT INTO edges(ref_id,kind,schema_id,source_id,target_id,position,revision,props) VALUES(?1,'schedule_occurrence',?2,?3,?4,0,0,jsonb('{\"record\":{}}'))",rusqlite::params![edge,edge_schema,source_id,target])?;
        if terminal {
            newest = reference;
        }
    }
    for (selection, index) in [
        (OccurrenceSelection::Recent(1), "nodes_schedule_due"),
        (
            OccurrenceSelection::UnarchivedTerminal,
            "nodes_schedule_terminal",
        ),
    ] {
        let sql = format!("EXPLAIN QUERY PLAN {}", reads::occurrence_sql(selection)?);
        let mut statement = tx.prepare(&sql)?;
        let mut rows = match selection {
            OccurrenceSelection::Recent(limit) => {
                statement.query(rusqlite::params![schedule.schedule_ref, limit as i64])?
            }
            OccurrenceSelection::UnarchivedTerminal => statement.query([&schedule.schedule_ref])?,
        };
        let mut indexed = false;
        while let Some(row) = rows.next()? {
            let step: String = row.get(3)?;
            if step.contains(&format!("SEARCH n USING INDEX {index}")) {
                indexed = true;
            }
            if step.contains("USE TEMP B-TREE") {
                return Err(format!("schedule read sorts retained history: {step}").into());
            }
        }
        if !indexed {
            return Err(format!("schedule read missed owning {index}").into());
        }
    }
    Ok(newest)
}
