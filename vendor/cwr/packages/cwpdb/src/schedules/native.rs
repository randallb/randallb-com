use super::*;
use crate::{
    ref_read::RefRecord,
    unordered::{Cardinality, Contract},
};
use rusqlite::{Connection, Transaction};
use serde::{de::DeserializeOwned, Serialize};
use serde_json::json;
#[cfg(feature = "test-support")]
mod fixtures;
mod mutations;
mod reads;
fn contract(kind: &'static str) -> Contract {
    let (source_kind, target_kind, singleton) = match kind {
        "schedule_owner" => ("runtime", "schedule_owner", false),
        "installed_schedule" => ("schedule_owner", "installed_schedule", false),
        "schedule_occurrence" => ("installed_schedule", "schedule_occurrence", false),
        "schedule_invocation" => ("schedule_occurrence", "invocation", true),
        "schedule_conversation" => ("schedule_occurrence", "conversation", true),
        _ => unreachable!("finite schedule relationship"),
    };
    Contract {
        kind,
        schema: crate::EDGE_SCHEMA_REF,
        source_kind,
        target_kind,
        external_target: false,
        source_cardinality: if singleton {
            Cardinality::One
        } else {
            Cardinality::Many
        },
        target_cardinality: Cardinality::One,
    }
}
fn links(db: &Connection, kind: &'static str, source: &str) -> CwpdbResult<Vec<String>> {
    Ok(
        crate::unordered::many_outgoing(db, contract(kind), source, 20000)?
            .into_iter()
            .map(|edge| edge.target)
            .collect(),
    )
}
fn link(tx: &Transaction<'_>, kind: &'static str, source: &str, target: &str) -> CwpdbResult<()> {
    crate::unordered::link(tx, contract(kind), source, target)?;
    Ok(())
}
fn read<T: DeserializeOwned>(
    db: &Connection,
    catalog: &Catalog,
    definition: &str,
    reference: &str,
) -> CwpdbResult<Option<Versioned<T>>> {
    if crate::node_props(db, reference)?.is_none() {
        return Ok(None);
    }
    let record = catalog
        .entity(definition)?
        .get_in_connection(db, definition, reference)?;
    Ok(Some(Versioned {
        value: serde_json::from_value(record.value.properties["record"].clone())?,
        revision: record.revision,
    }))
}
fn write<T: Serialize + DeserializeOwned>(
    tx: &Transaction<'_>,
    catalog: &Catalog,
    definition: &str,
    reference: &str,
    expected: Option<&RefRevision>,
    value: &T,
) -> CwpdbResult<Versioned<T>> {
    let binding = catalog.entity(definition)?;
    let props = json!({"record":value});
    let record: RefRecord = if crate::node_props(tx, reference)?.is_some() {
        let current = binding.get_in_connection(tx, definition, reference)?;
        if expected != Some(&current.revision) {
            return Err("schedule revision conflict; reload owning state".into());
        }
        binding.replace_in_transaction(tx, definition, reference, &current.revision, &props)?
    } else {
        if expected.is_some() {
            return Err("schedule record disappeared".into());
        }
        binding.create_in_transaction(tx, definition, reference, &props)?
    };
    Ok(Versioned {
        value: serde_json::from_value(record.value.properties["record"].clone())?,
        revision: record.revision,
    })
}
fn owner(schedule: &Installed, expected: Option<&str>) -> CwpdbResult<()> {
    if expected.is_some_and(|owner| schedule.owner_context_ref != owner) {
        return Err("schedule is unavailable in this context".into());
    }
    Ok(())
}
fn expected(actual: &RefRevision, expected: &RefRevision) -> CwpdbResult<()> {
    if actual != expected {
        return Err("schedule revision conflict; reload owning state".into());
    }
    Ok(())
}
impl ScheduleStorage for NativeStore {
    #[cfg(feature = "test-support")]
    fn seed_and_verify_history_fixture(
        &mut self,
        schedule: &Installed,
        count: usize,
    ) -> CwpdbResult<String> {
        self.graph_transaction(|tx| fixtures::seed_and_verify(tx, schedule, count))
    }

    fn installed(
        &mut self,
        catalog: &Catalog,
        runtime_root_ref: &str,
        owner: Option<&str>,
    ) -> CwpdbResult<Vec<Versioned<Installed>>> {
        self.graph_read_transaction(|tx| {
            response_budget(reads::installed(tx, catalog, runtime_root_ref, owner)?)
        })
    }
    fn get_schedule(
        &mut self,
        catalog: &Catalog,
        reference: &str,
        scope: Option<&str>,
    ) -> CwpdbResult<Option<Versioned<Installed>>> {
        self.graph_read_transaction(|tx| {
            let record = read::<Installed>(tx, catalog, SCHEDULE, reference)?;
            if let Some(record) = &record {
                owner(&record.value, scope)?;
            }
            Ok(record)
        })
    }
    fn get_occurrence(
        &mut self,
        catalog: &Catalog,
        reference: &str,
    ) -> CwpdbResult<Option<Versioned<Occurrence>>> {
        self.graph_read_transaction(|tx| read(tx, catalog, OCCURRENCE, reference))
    }
    fn occurrences(
        &mut self,
        catalog: &Catalog,
        schedule_ref: &str,
        selection: OccurrenceSelection,
    ) -> CwpdbResult<Vec<Versioned<Occurrence>>> {
        self.graph_read_transaction(|tx| {
            response_budget(reads::occurrences(tx, catalog, schedule_ref, selection)?)
        })
    }
    fn archive_candidates(
        &mut self,
        catalog: &Catalog,
        runtime_root_ref: &str,
    ) -> CwpdbResult<Vec<(String, Versioned<Occurrence>)>> {
        self.graph_read_transaction(|tx| {
            let mut result = Vec::new();
            for schedule in reads::installed(tx, catalog, runtime_root_ref, None)? {
                for occurrence in reads::occurrences(
                    tx,
                    catalog,
                    &schedule.value.schedule_ref,
                    OccurrenceSelection::UnarchivedTerminal,
                )? {
                    if !occurrence.value.outcome.starts_with("terminal_")
                        || occurrence.value.archived
                    {
                        return Err("schedule archive candidate mismatch".into());
                    }
                    result.push((schedule.value.author_actor_ref.clone(), occurrence));
                }
            }
            response_budget(result)
        })
    }
    fn install(
        &mut self,
        catalog: &Catalog,
        request: &Install,
    ) -> CwpdbResult<Versioned<Installed>> {
        self.graph_transaction(|tx| mutations::install(tx, catalog, request))
    }
    fn pause(
        &mut self,
        catalog: &Catalog,
        scope: &str,
        reference: &str,
        revision: &RefRevision,
        paused: bool,
        next_due: u64,
    ) -> CwpdbResult<Versioned<Installed>> {
        time(next_due)?;
        request_budget(&(scope, reference, revision, paused, next_due))?;
        self.graph_transaction(|tx| {
            let mut current: Versioned<Installed> = read(tx, catalog, SCHEDULE, reference)?
                .ok_or("schedule is unavailable in this context")?;
            owner(&current.value, Some(scope))?;
            expected(&current.revision, revision)?;
            if current.value.paused == paused {
                return Ok(current);
            }
            current.value.paused = paused;
            if !paused {
                current.value.next_due_unix_ms = next_due;
            }
            write(
                tx,
                catalog,
                SCHEDULE,
                reference,
                Some(revision),
                &current.value,
            )
        })
    }
    fn availability(
        &mut self,
        catalog: &Catalog,
        scope: &str,
        changes: &[Availability],
    ) -> CwpdbResult<()> {
        request_budget(&changes)?;
        self.graph_transaction(|tx| {
            for change in changes {
                time(change.next_due_unix_ms)?;
                if !["ready", "disabled", "needs_setup", "removed"]
                    .contains(&change.availability.as_str())
                {
                    return Err("unsupported installed schedule availability".into());
                }
                let mut current: Versioned<Installed> =
                    read(tx, catalog, SCHEDULE, &change.schedule_ref)?.ok_or("missing schedule")?;
                owner(&current.value, Some(scope))?;
                expected(&current.revision, &change.expected)?;
                if current.value.availability != change.availability {
                    current.value.availability = change.availability.clone();
                    if change.availability == "ready" {
                        current.value.next_due_unix_ms = change.next_due_unix_ms;
                    }
                    write(
                        tx,
                        catalog,
                        SCHEDULE,
                        &change.schedule_ref,
                        Some(&change.expected),
                        &current.value,
                    )?;
                }
            }
            Ok(())
        })
    }
    fn advance(
        &mut self,
        catalog: &Catalog,
        request: &Advance,
    ) -> CwpdbResult<Versioned<Installed>> {
        self.graph_transaction(|tx| mutations::advance(tx, catalog, request))
    }
    fn reserve(
        &mut self,
        catalog: &Catalog,
        request: &Reservation,
    ) -> CwpdbResult<Versioned<Installed>> {
        self.graph_transaction(|tx| mutations::reserve(tx, catalog, request))
    }
    fn settle(
        &mut self,
        catalog: &Catalog,
        request: &Settle,
    ) -> CwpdbResult<Versioned<Occurrence>> {
        self.graph_transaction(|tx| mutations::settle(tx, catalog, request))
    }
}
