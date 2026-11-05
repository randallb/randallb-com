// CWR compiles trusted declarations and decodes domain records. Every persistence
// decision is a complete named operation owned by cwpdb; no graph handle escapes.
use crate::cwpdb::ref_read::RefRevision;
use crate::cwpdb::schedules::{self as storage, ScheduleStorage};
fn schedule_catalog() -> CwrResult<&'static crate::cwpdb::declared::Catalog> {
    use std::sync::{Arc, OnceLock};
    static CATALOG: OnceLock<crate::cwpdb::declared::Catalog> = OnceLock::new();
    if CATALOG.get().is_none() {
        let entities = Arc::new(crate::entities::EntityCatalog::from_toml_documents([
            include_str!(
                "../../../../resources/base/schedules/definitions/installed-schedule.toml"
            ),
            include_str!(
                "../../../../resources/base/schedules/definitions/schedule-occurrence.toml"
            ),
            include_str!("../../../../resources/base/schedules/definitions/schedule-owner.toml"),
        ])?);
        let contracts = entities
            .types()
            .into_iter()
            .map(|definition| entities.contract(definition))
            .collect::<CwrResult<Vec<_>>>()?;
        let _ = CATALOG.set(crate::cwpdb::declared::Catalog::new(contracts, entities)?);
    }
    Ok(CATALOG.get().expect("validated schedule declarations"))
}
struct ScheduleRepository {
    store: Box<dyn ScheduleStorage>,
    root: String,
    revisions: BTreeMap<String, RefRevision>,
}
fn schedule_store(state: &LocalRuntimeState) -> CwrResult<ScheduleRepository> {
    let selected = std::env::var_os("CWR_CONVERSATION_STORAGE_URL");
    let required = std::env::var("CWR_REQUIRE_REMOTE_STATE")
        .ok()
        .is_some_and(|value| value == "1" || value == "true");
    let store: Box<dyn ScheduleStorage> = if let Some(endpoint) = selected {
        let token = std::env::var("CWR_CONVERSATION_STORAGE_TOKEN")
            .map_err(|_| invalid_error("hosted schedules require the trusted storage token"))?;
        let workspace_ref = std::env::var("CWR_CONVERSATION_WORKSPACE_REF")
            .map_err(|_| invalid_error("hosted schedules require the workspace binding"))?;
        let caller_ref = std::env::var("CWR_CONVERSATION_CALLER_REF")
            .map_err(|_| invalid_error("hosted schedules require the trusted caller binding"))?;
        CwrRef::parse(&workspace_ref)?;
        CwrRef::parse(&caller_ref)?;
        Box::new(storage::RemoteSchedules::new(
            crate::cloud_execution::HttpDeclaredTransport::new(
                endpoint
                    .to_str()
                    .ok_or_else(|| invalid_error("invalid hosted schedule endpoint"))?,
                token,
            )?,
            crate::cwpdb::invocation::Context {
                workspace_ref,
                caller_ref,
            },
        ))
    } else {
        if required {
            return Err(invalid_error(
                "hosted schedules refuse native state fallback",
            ));
        }
        let (path, _) = state.admission.read_store_identity();
        Box::new(crate::cwpdb::NativeStore::open(path)?)
    };
    Ok(ScheduleRepository {
        store,
        root: state.admission.runtime_root_ref().into(),
        revisions: BTreeMap::new(),
    })
}
fn convert<T: serde::de::DeserializeOwned>(record: impl Serialize) -> CwrResult<T> {
    Ok(serde_json::from_value(serde_json::to_value(record)?)?)
}
impl ScheduleRepository {
    #[cfg(test)]
    fn seed_and_verify_history_fixture(
        &mut self,
        schedule: &InstalledSchedule,
        count: usize,
    ) -> CwrResult<String> {
        self.store
            .seed_and_verify_history_fixture(&convert(schedule)?, count)
    }

    fn schedule_value(
        &mut self,
        record: storage::Versioned<storage::Installed>,
    ) -> CwrResult<InstalledSchedule> {
        self.revisions
            .insert(record.value.schedule_ref.clone(), record.revision);
        convert(record.value)
    }
    fn occurrence_value(
        &mut self,
        record: storage::Versioned<storage::Occurrence>,
    ) -> CwrResult<ScheduleOccurrence> {
        self.revisions
            .insert(record.value.occurrence_ref.clone(), record.revision);
        convert(record.value)
    }
    fn expected(&self, reference: &str) -> CwrResult<RefRevision> {
        self.revisions
            .get(reference)
            .cloned()
            .ok_or_else(|| invalid_error("schedule operation requires a fresh owning read"))
    }
    fn installed(&mut self, owner: Option<&str>) -> CwrResult<Vec<InstalledSchedule>> {
        let records = self
            .store
            .installed(schedule_catalog()?, &self.root, owner)?;
        records
            .into_iter()
            .map(|record| self.schedule_value(record))
            .collect()
    }
    fn get(
        &mut self,
        reference: &str,
        owner: Option<&str>,
    ) -> CwrResult<Option<InstalledSchedule>> {
        self.store
            .get_schedule(schedule_catalog()?, reference, owner)?
            .map(|record| self.schedule_value(record))
            .transpose()
    }
    fn occurrence(&mut self, reference: &str) -> CwrResult<Option<ScheduleOccurrence>> {
        self.store
            .get_occurrence(schedule_catalog()?, reference)?
            .map(|record| self.occurrence_value(record))
            .transpose()
    }
    fn occurrences(
        &mut self,
        reference: &str,
        selection: storage::OccurrenceSelection,
    ) -> CwrResult<Vec<ScheduleOccurrence>> {
        let records = self
            .store
            .occurrences(schedule_catalog()?, reference, selection)?;
        records
            .into_iter()
            .map(|record| self.occurrence_value(record))
            .collect()
    }
    fn archive_candidates(&mut self) -> CwrResult<Vec<(String, ScheduleOccurrence)>> {
        let records = self
            .store
            .archive_candidates(schedule_catalog()?, &self.root)?;
        records
            .into_iter()
            .map(|(actor, record)| Ok((actor, self.occurrence_value(record)?)))
            .collect()
    }
    fn install(
        &mut self,
        owner: &str,
        actor: &str,
        definition: ScheduleDeclaration,
        next_due: u64,
    ) -> CwrResult<InstalledSchedule> {
        let record = self.store.install(
            schedule_catalog()?,
            &storage::Install {
                runtime_root_ref: self.root.clone(),
                owner_context_ref: owner.into(),
                author_actor_ref: actor.into(),
                definition: serde_json::to_value(definition)?,
                next_due_unix_ms: next_due,
            },
        )?;
        self.schedule_value(record)
    }
    fn pause(
        &mut self,
        owner: &str,
        schedule: &InstalledSchedule,
        paused: bool,
        next_due: u64,
    ) -> CwrResult<InstalledSchedule> {
        let record = self.store.pause(
            schedule_catalog()?,
            owner,
            &schedule.schedule_ref,
            &self.expected(&schedule.schedule_ref)?,
            paused,
            next_due,
        )?;
        self.schedule_value(record)
    }
    fn availability(
        &mut self,
        owner: &str,
        changes: Vec<(InstalledSchedule, String, u64)>,
    ) -> CwrResult<()> {
        let changes = changes
            .into_iter()
            .map(|(schedule, availability, next_due)| {
                Ok(storage::Availability {
                    expected: self.expected(&schedule.schedule_ref)?,
                    schedule_ref: schedule.schedule_ref,
                    availability,
                    next_due_unix_ms: next_due,
                })
            })
            .collect::<CwrResult<Vec<_>>>()?;
        self.store
            .availability(schedule_catalog()?, owner, &changes)
    }
    fn advance(&mut self, schedule: &InstalledSchedule) -> CwrResult<()> {
        let record = self.store.advance(
            schedule_catalog()?,
            &storage::Advance {
                schedule_ref: schedule.schedule_ref.clone(),
                expected: self.expected(&schedule.schedule_ref)?,
                next_due_unix_ms: schedule.next_due_unix_ms,
                active_occurrence_ref: schedule.active_occurrence_ref.clone(),
                last_admission_error: schedule.last_admission_error.clone(),
            },
        )?;
        self.schedule_value(record)?;
        Ok(())
    }
    fn reserve(
        &mut self,
        schedule: &InstalledSchedule,
        occurrence: &ScheduleOccurrence,
    ) -> CwrResult<()> {
        let record = self.store.reserve(
            schedule_catalog()?,
            &storage::Reservation {
                schedule_ref: schedule.schedule_ref.clone(),
                expected: self.expected(&schedule.schedule_ref)?,
                occurrence: convert(occurrence)?,
                next_due_unix_ms: schedule.next_due_unix_ms,
                active_occurrence_ref: schedule.active_occurrence_ref.clone(),
            },
        )?;
        self.schedule_value(record)?;
        Ok(())
    }
    fn settle(&mut self, occurrence: &ScheduleOccurrence) -> CwrResult<()> {
        let record = self.store.settle(
            schedule_catalog()?,
            &storage::Settle {
                occurrence_ref: occurrence.occurrence_ref.clone(),
                expected: self.expected(&occurrence.occurrence_ref)?,
                outcome: occurrence.outcome.clone(),
                invocation_ref: occurrence.invocation_ref.clone(),
                conversation_ref: occurrence.conversation_ref.clone(),
                archived: occurrence.archived,
            },
        )?;
        self.occurrence_value(record)?;
        Ok(())
    }
}
