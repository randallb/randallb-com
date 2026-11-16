//! Installed schedules v1: the owner-selected native collection read contract.
use super::schema::{blocking, ReadService};
use async_graphql::{Result, SimpleObject, ID};
use cwr::v2026_09::local::{InstalledSchedule, ScheduleOccurrence};

#[derive(SimpleObject)]
pub(super) struct ScheduleDefinitionV1 {
    declaration_ref: ID,
    revision: String,
    anchor_unix_ms: f64,
    interval_ms: f64,
}

#[derive(SimpleObject)]
pub(super) struct InstalledScheduleV1 {
    id: ID,
    schedule_ref: ID,
    owner_context_ref: ID,
    author_actor_ref: ID,
    definition: ScheduleDefinitionV1,
    availability: String,
    paused: bool,
    next_due_unix_ms: f64,
    active_occurrence_ref: Option<ID>,
    last_admission_error: Option<String>,
}

impl From<InstalledSchedule> for InstalledScheduleV1 {
    fn from(value: InstalledSchedule) -> Self {
        Self {
            id: ID(value.schedule_ref.clone()),
            schedule_ref: ID(value.schedule_ref),
            owner_context_ref: ID(value.owner_context_ref),
            author_actor_ref: ID(value.author_actor_ref),
            definition: ScheduleDefinitionV1 {
                declaration_ref: ID(value.definition.declaration_ref),
                revision: value.definition.revision,
                anchor_unix_ms: value.definition.anchor_unix_ms as f64,
                interval_ms: value.definition.interval_ms as f64,
            },
            availability: value.availability,
            paused: value.paused,
            next_due_unix_ms: value.next_due_unix_ms as f64,
            active_occurrence_ref: value.active_occurrence_ref.map(ID),
            last_admission_error: value.last_admission_error,
        }
    }
}

#[derive(SimpleObject)]
pub(super) struct ScheduleOccurrenceV1 {
    id: ID,
    occurrence_ref: ID,
    schedule_ref: ID,
    due_unix_ms: f64,
    outcome: String,
    invocation_ref: Option<ID>,
    conversation_ref: Option<ID>,
    archived: bool,
}

impl From<ScheduleOccurrence> for ScheduleOccurrenceV1 {
    fn from(value: ScheduleOccurrence) -> Self {
        Self {
            id: ID(value.occurrence_ref.clone()),
            occurrence_ref: ID(value.occurrence_ref),
            schedule_ref: ID(value.schedule_ref),
            due_unix_ms: value.due_unix_ms as f64,
            outcome: value.outcome,
            invocation_ref: value.invocation_ref.map(ID),
            conversation_ref: value.conversation_ref.map(ID),
            archived: value.archived,
        }
    }
}

pub(super) async fn list(service: ReadService, owner: ID) -> Result<Vec<InstalledScheduleV1>> {
    blocking(service.reads.clone(), service.budget(), move || {
        let schedules = service
            .runtime
            .host_schedules_module_as(owner.as_str(), &service.actor_ref)?
            .list()?;
        Ok(schedules.into_iter().map(Into::into).collect())
    })
    .await
}

pub(super) async fn runs(
    service: ReadService,
    owner: ID,
    schedule: ID,
    first: i32,
) -> Result<Vec<ScheduleOccurrenceV1>> {
    if !(1..=100).contains(&first) {
        return Err("first must be 1 to 100".into());
    }
    blocking(service.reads.clone(), service.budget(), move || {
        let runs = service
            .runtime
            .host_schedules_module_as(owner.as_str(), &service.actor_ref)?
            .recent_runs(schedule.as_str(), first as usize)?;
        Ok(runs.into_iter().map(Into::into).collect())
    })
    .await
}
