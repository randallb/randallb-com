use super::*;
pub(super) fn install(
    tx: &Transaction<'_>,
    catalog: &Catalog,
    request: &Install,
) -> CwpdbResult<Versioned<Installed>> {
    request_budget(request)?;
    time(request.next_due_unix_ms)?;
    if request.owner_context_ref.trim().is_empty() || request.owner_context_ref.len() > 2048 {
        return Err("installed owner context is required".into());
    }
    let declaration = request.definition["declaration_ref"]
        .as_str()
        .ok_or("schedule declaration ref required")?;
    let reference = identity(
        "installed-schedules",
        (&request.owner_context_ref, declaration),
    )?;
    let old: Option<Versioned<Installed>> = read(tx, catalog, SCHEDULE, &reference)?;
    let mut installed = old
        .as_ref()
        .map(|old| old.value.clone())
        .unwrap_or_else(|| Installed {
            schedule_ref: reference.clone(),
            owner_context_ref: request.owner_context_ref.clone(),
            author_actor_ref: request.author_actor_ref.clone(),
            definition: request.definition.clone(),
            availability: "ready".into(),
            paused: false,
            next_due_unix_ms: request.next_due_unix_ms,
            active_occurrence_ref: None,
            last_admission_error: None,
        });
    owner(&installed, Some(&request.owner_context_ref))?;
    if installed.definition != request.definition {
        if installed.definition["revision"] == request.definition["revision"] {
            return Err("schedule revision was reused with different content".into());
        }
        installed.definition = request.definition.clone();
        if installed.active_occurrence_ref.is_none() {
            installed.next_due_unix_ms = request.next_due_unix_ms;
        }
    }
    let owner_ref = owner_binding(&request.owner_context_ref)?;
    let current: Option<Versioned<Value>> = read(tx, catalog, OWNER, &owner_ref)?;
    write(
        tx,
        catalog,
        OWNER,
        &owner_ref,
        current.as_ref().map(|old| &old.revision),
        &json!({"owner_context_ref":request.owner_context_ref}),
    )?;
    let result = write(
        tx,
        catalog,
        SCHEDULE,
        &reference,
        old.as_ref().map(|old| &old.revision),
        &installed,
    )?;
    crate::claim_identity(tx, &reference, "node:installed_schedule", &owner_ref)?;
    link(
        tx,
        "schedule_owner",
        &format!("{}/cwpdb", request.runtime_root_ref),
        &owner_ref,
    )?;
    link(tx, "installed_schedule", &owner_ref, &reference)?;
    Ok(result)
}
pub(super) fn advance(
    tx: &Transaction<'_>,
    catalog: &Catalog,
    request: &Advance,
) -> CwpdbResult<Versioned<Installed>> {
    request_budget(request)?;
    time(request.next_due_unix_ms)?;
    let mut current: Versioned<Installed> =
        read(tx, catalog, SCHEDULE, &request.schedule_ref)?.ok_or("missing schedule")?;
    expected(&current.revision, &request.expected)?;
    if let Some(active) = &request.active_occurrence_ref {
        let occurrence: Versioned<Occurrence> =
            read(tx, catalog, OCCURRENCE, active)?.ok_or("active schedule occurrence missing")?;
        if occurrence.value.schedule_ref != request.schedule_ref {
            return Err("active occurrence mismatch".into());
        }
    }
    current.value.next_due_unix_ms = request.next_due_unix_ms;
    current.value.active_occurrence_ref = request.active_occurrence_ref.clone();
    current.value.last_admission_error = request.last_admission_error.clone();
    write(
        tx,
        catalog,
        SCHEDULE,
        &request.schedule_ref,
        Some(&request.expected),
        &current.value,
    )
}
pub(super) fn reserve(
    tx: &Transaction<'_>,
    catalog: &Catalog,
    request: &Reservation,
) -> CwpdbResult<Versioned<Installed>> {
    request_budget(request)?;
    time(request.next_due_unix_ms)?;
    let current: Versioned<Installed> =
        read(tx, catalog, SCHEDULE, &request.schedule_ref)?.ok_or("missing schedule")?;
    expected(&current.revision, &request.expected)?;
    let occurrence = &request.occurrence;
    time(occurrence.due_unix_ms)?;
    if occurrence.schedule_ref != request.schedule_ref
        || occurrence.definition != current.value.definition
        || occurrence.occurrence_ref
            != identity(
                "schedule-occurrences",
                (
                    &request.schedule_ref,
                    &occurrence.definition["revision"],
                    occurrence.due_unix_ms,
                ),
            )?
    {
        return Err("schedule occurrence identity mismatch".into());
    }
    if occurrence.invocation_ref.is_some()
        || occurrence.conversation_ref.is_some()
        || occurrence.archived
    {
        return Err("new occurrence cannot attach execution or archival".into());
    }
    let old: Option<Versioned<Occurrence>> =
        read(tx, catalog, OCCURRENCE, &occurrence.occurrence_ref)?;
    if old.as_ref().is_some_and(|old| old.value != *occurrence) {
        return Err("schedule occurrence identity was reused".into());
    }
    if old.is_none() && occurrence.outcome.starts_with("terminal_") {
        terminal_capacity(tx, catalog, &request.schedule_ref)?;
    }
    write(
        tx,
        catalog,
        OCCURRENCE,
        &occurrence.occurrence_ref,
        old.as_ref().map(|old| &old.revision),
        occurrence,
    )?;
    crate::claim_identity(
        tx,
        &occurrence.occurrence_ref,
        "node:schedule_occurrence",
        &request.schedule_ref,
    )?;
    link(
        tx,
        "schedule_occurrence",
        &request.schedule_ref,
        &occurrence.occurrence_ref,
    )?;
    advance(
        tx,
        catalog,
        &Advance {
            schedule_ref: request.schedule_ref.clone(),
            expected: request.expected.clone(),
            next_due_unix_ms: request.next_due_unix_ms,
            active_occurrence_ref: request.active_occurrence_ref.clone(),
            last_admission_error: current.value.last_admission_error,
        },
    )
}
pub(super) fn settle(
    tx: &Transaction<'_>,
    catalog: &Catalog,
    request: &Settle,
) -> CwpdbResult<Versioned<Occurrence>> {
    request_budget(request)?;
    let mut current: Versioned<Occurrence> =
        read(tx, catalog, OCCURRENCE, &request.occurrence_ref)?.ok_or("missing occurrence")?;
    expected(&current.revision, &request.expected)?;
    if current.value.invocation_ref.is_some()
        && current.value.invocation_ref != request.invocation_ref
        || current.value.conversation_ref.is_some()
            && current.value.conversation_ref != request.conversation_ref
    {
        return Err("retained schedule execution identity changed".into());
    }
    if request.invocation_ref.is_some() != request.conversation_ref.is_some() {
        return Err("schedule execution requires invocation and conversation".into());
    }
    if current.value.archived && !request.archived
        || current.value.outcome.starts_with("terminal_")
            && !request.outcome.starts_with("terminal_")
        || request.archived && !request.outcome.starts_with("terminal_")
    {
        return Err("schedule terminal/archival state cannot regress".into());
    }
    if !current.value.outcome.starts_with("terminal_")
        && request.outcome.starts_with("terminal_")
        && !request.archived
    {
        terminal_capacity(tx, catalog, &current.value.schedule_ref)?;
    }
    current.value.outcome = request.outcome.clone();
    current.value.invocation_ref = request.invocation_ref.clone();
    current.value.conversation_ref = request.conversation_ref.clone();
    current.value.archived = request.archived;
    let result = write(
        tx,
        catalog,
        OCCURRENCE,
        &request.occurrence_ref,
        Some(&request.expected),
        &current.value,
    )?;
    if let (Some(invocation), Some(conversation)) =
        (&request.invocation_ref, &request.conversation_ref)
    {
        link(
            tx,
            "schedule_invocation",
            &request.occurrence_ref,
            invocation,
        )?;
        link(
            tx,
            "schedule_conversation",
            &request.occurrence_ref,
            conversation,
        )?;
    }
    Ok(result)
}

fn terminal_capacity(db: &Connection, catalog: &Catalog, reference: &str) -> CwpdbResult<()> {
    if reads::occurrences(
        db,
        catalog,
        reference,
        OccurrenceSelection::UnarchivedTerminal,
    )?
    .len()
        >= 20000
    {
        return Err("schedule index work limit exceeded".into());
    }
    Ok(())
}
