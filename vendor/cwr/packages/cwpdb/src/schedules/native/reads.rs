use super::*;
pub(super) fn installed(
    db: &Connection,
    catalog: &Catalog,
    runtime_root_ref: &str,
    scope: Option<&str>,
) -> CwpdbResult<Vec<Versioned<Installed>>> {
    catalog.validate_reference(runtime_root_ref)?;
    let owners = if let Some(scope) = scope {
        vec![owner_binding(scope)?]
    } else {
        links(db, "schedule_owner", &format!("{runtime_root_ref}/cwpdb"))?
    };
    let mut result = Vec::new();
    for owner_ref in owners {
        for reference in links(db, "installed_schedule", &owner_ref)? {
            let record: Versioned<Installed> = read(db, catalog, SCHEDULE, &reference)?
                .ok_or("installed schedule record missing")?;
            owner(&record.value, scope)?;
            if owner_binding(&record.value.owner_context_ref)? != owner_ref {
                return Err("installed owner relationship mismatch".into());
            }
            result.push(record);
        }
    }
    Ok(result)
}
pub(super) fn occurrences(
    db: &Connection,
    catalog: &Catalog,
    source: &str,
    selection: OccurrenceSelection,
) -> CwpdbResult<Vec<Versioned<Occurrence>>> {
    let sql = occurrence_sql(selection)?;
    let mut statement = db.prepare_cached(&sql)?;
    let mut rows = match selection {
        OccurrenceSelection::Recent(limit) => {
            statement.query(rusqlite::params![source, limit as i64])?
        }
        OccurrenceSelection::UnarchivedTerminal => statement.query([source])?,
    };
    let mut result = Vec::new();
    while let Some(row) = rows.next()? {
        let target: String = row.get(0)?;
        let schema: String = row.get(1)?;
        let position: i64 = row.get(2)?;
        let props: String = row.get(3)?;
        let edge_ref: String = row.get(4)?;
        if schema != crate::EDGE_SCHEMA_REF
            || position != 0
            || serde_json::from_str::<Value>(&props)? != json!({"record":{}})
            || edge_ref
                != crate::unordered::desktop_ref("schedule_occurrence", &json!([source, &target]))
        {
            return Err("schedule occurrence relationship is invalid".into());
        }
        let record: Versioned<Occurrence> =
            read(db, catalog, OCCURRENCE, &target)?.ok_or("schedule occurrence is unavailable")?;
        if record.value.schedule_ref != source {
            return Err("schedule occurrence owner mismatch".into());
        }
        result.push(record);
        if result.len() > 20000 {
            return Err("schedule index work limit exceeded".into());
        }
    }
    Ok(result)
}

pub(super) fn occurrence_sql(selection: OccurrenceSelection) -> CwpdbResult<String> {
    let(index,tail)=match selection{OccurrenceSelection::Recent(limit)=>{if limit==0||limit>100{return Err("recent run limit must be1to100".into());}("nodes_schedule_due","ORDER BY json_extract(n.props,'$.record.due_unix_ms') DESC,n.ref_id DESC LIMIT ?2")},OccurrenceSelection::UnarchivedTerminal=>("nodes_schedule_terminal","AND substr(json_extract(n.props,'$.record.outcome'),1,9)='terminal_' AND json_extract(n.props,'$.record.archived')=0 LIMIT 20001")};
    Ok(format!("SELECT t.canonical_ref,s.canonical_ref,e.position,json(e.props),r.canonical_ref FROM nodes n INDEXED BY {index} CROSS JOIN edges e INDEXED BY edges_in ON e.target_id=n.ref_id AND e.kind='schedule_occurrence' AND e.source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) JOIN ref_bindings t ON t.ref_id=e.target_id JOIN ref_bindings s ON s.ref_id=e.schema_id JOIN ref_bindings r ON r.ref_id=e.ref_id WHERE n.kind='schedule_occurrence' AND json_extract(n.props,'$.record.schedule_ref')=?1 {tail}"))
}
