#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScheduleWorkerState {
    Uninitialized,
    Initialized,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScheduleFault {
    #[default]
    None,
    AfterReservation,
    AfterAdmission,
    AfterLink,
    BeforeArchive,
    AfterArchive,
}

/// Committed changes the native host must publish after a timer pass.
#[derive(Default)]
pub struct ScheduleTickResult {
    pub admissions: Vec<LocalAdmission>,
    pub archived_conversation_refs: Vec<String>,
}

const SCHEDULE_INGRESS: &str = "cwp://org.coworkerprotocol/cwr/ingress/schedules";
const ARCHIVE_RECOVERY_INTERVAL_MS: u64 = 30_000;
const ARCHIVE_RETRY_INTERVAL_MS: u64 = 1_000;

#[derive(Clone)]
struct PendingScheduleArchive {
    actor: String,
    occurrence: ScheduleOccurrence,
    retry_after_unix_ms: u64,
}

impl LocalRuntime {
    /// Runtime timer consumer. It never depends on a mounted product view.
    pub fn tick_schedules(&self, now: u64) -> CwrResult<Vec<LocalAdmission>> {
        self.tick_schedules_with_updates(now)
            .map(|result| result.admissions)
    }

    pub fn tick_schedules_with_updates(&self, now: u64) -> CwrResult<ScheduleTickResult> {
        self.tick_schedules_reporting(now, ScheduleFault::None)
    }

    #[doc(hidden)]
    pub fn tick_schedules_with_fault(
        &self,
        now: u64,
        fault: ScheduleFault,
    ) -> CwrResult<Vec<LocalAdmission>> {
        self.tick_schedules_reporting(now, fault)
            .map(|result| result.admissions)
    }

    fn tick_schedules_reporting(
        &self,
        now: u64,
        fault: ScheduleFault,
    ) -> CwrResult<ScheduleTickResult> {
        let mut state = self.lock()?;
        if state.shutting_down
            || state
                .admission
                .context_policy
                .as_ref()
                .is_none_or(|p| p.suspended)
        {
            return Ok(ScheduleTickResult::default());
        }
        let mut store = schedule_store(&state)?;
        let schedules = store.installed(None)?;
        let boot = state.schedule_worker_state == ScheduleWorkerState::Uninitialized;
        state.schedule_worker_state = ScheduleWorkerState::Initialized;
        let mut admitted = vec![];
        for mut schedule in schedules {
            let context = self.installed_action_context_as(
                &schedule.owner_context_ref,
                &schedule.author_actor_ref,
                SCHEDULE_INGRESS,
            )?;
            // First settle/recover the retained active occurrence. Archival is
            // performed later, outside the runtime mutex, through its service.
            if let Some(reference) = schedule.active_occurrence_ref.clone() {
                let mut occurrence = store
                    .occurrence(&reference)?
                    .ok_or_else(|| invalid_error("active schedule occurrence missing"))?;
                if occurrence.schedule_ref != schedule.schedule_ref {
                    return invalid("active occurrence mismatch");
                }
                let receipt = state
                    .admission
                    .admission(SCHEDULE_INGRESS, &reference)
                    .cloned();
                if occurrence.invocation_ref.is_none()
                    && receipt.is_none()
                    && (schedule.paused
                        || schedule.availability != "ready"
                        || now
                            >= occurrence
                                .due_unix_ms
                                .saturating_add(occurrence.definition.interval_ms))
                {
                    occurrence.outcome = if schedule.paused {
                        "skipped_paused"
                    } else if schedule.availability != "ready" {
                        "skipped_unavailable"
                    } else {
                        "skipped_offline"
                    }
                    .into();
                    schedule.active_occurrence_ref = None;
                } else if occurrence.invocation_ref.is_none() {
                    if let Some(admission) = admit_pending_schedule(
                        &context,
                        &mut state,
                        &mut schedule,
                        &mut occurrence,
                        fault,
                    )? {
                        admitted.push(admission);
                    }
                }
                if let Some(invocation) = occurrence.invocation_ref.as_deref() {
                    let record = state
                        .admission
                        .invocation_record(invocation)
                        .ok_or_else(|| invalid_error("schedule invocation missing"))?;
                    if record.state.is_terminal() && record.state != ActionInvocationState::Stopping
                    {
                        occurrence.outcome = format!("terminal_{:?}", record.state).to_lowercase();
                        schedule.active_occurrence_ref = None;
                    }
                }
                store.settle(&occurrence)?;
                if occurrence.outcome.starts_with("terminal_") && !occurrence.archived {
                    state.pending_schedule_archives.insert(
                        reference.clone(),
                        PendingScheduleArchive {
                            actor: schedule.author_actor_ref.clone(),
                            occurrence: occurrence.clone(),
                            retry_after_unix_ms: now,
                        },
                    );
                }
                if fault == ScheduleFault::AfterLink {
                    return invalid("injected after occurrence link");
                }
                if schedule.active_occurrence_ref.is_none() {
                    schedule.next_due_unix_ms = next_schedule_tick(&schedule.definition, now)?;
                }
                store.advance(&schedule)?;
            }
            if boot && schedule.active_occurrence_ref.is_none() && schedule.next_due_unix_ms <= now
            {
                schedule.next_due_unix_ms = next_schedule_tick(&schedule.definition, now)?;
                store.advance(&schedule)?;
            }
            if schedule.paused
                || schedule.availability != "ready"
                || schedule.next_due_unix_ms > now
            {
                continue;
            }
            let due = schedule.next_due_unix_ms;
            let reference = schedule_identity(
                "schedule-occurrences",
                (&schedule.schedule_ref, &schedule.definition.revision, due),
            )?;
            let outcome = if schedule.active_occurrence_ref.is_some() {
                "skipped_overlap"
            } else if now >= due.saturating_add(schedule.definition.interval_ms) {
                "skipped_offline"
            } else {
                "pending"
            };
            let occurrence = ScheduleOccurrence {
                occurrence_ref: reference.clone(),
                schedule_ref: schedule.schedule_ref.clone(),
                definition: schedule.definition.clone(),
                due_unix_ms: due,
                outcome: outcome.into(),
                invocation_ref: None,
                conversation_ref: None,
                archived: false,
            };
            if outcome == "pending" {
                schedule.active_occurrence_ref = Some(reference.clone());
            }
            // Occurrence reservation and schedule pointer are one owning graph
            // transaction. Action admission commits independently with the same ID.
            schedule.next_due_unix_ms = next_schedule_tick(&schedule.definition, now)?;
            store.reserve(&schedule, &occurrence)?;
            if fault == ScheduleFault::AfterReservation {
                return invalid("injected after reservation");
            }
            // Pending admission is consumed on the next worker pass. This keeps
            // the crash window explicit and never queues a second invocation.
        }
        self.finish_schedule_tick(state, boot, now, fault, admitted)
    }

    fn finish_schedule_tick(
        &self,
        mut state: MutexGuard<'_, LocalRuntimeState>,
        boot: bool,
        now: u64,
        fault: ScheduleFault,
        admitted: Vec<LocalAdmission>,
    ) -> CwrResult<ScheduleTickResult> {
        // Advance immediately before the attempt. Earlier loop errors leave
        // the boot or recovery sweep due, while scan errors retry after 1 s.
        let scan_archives = boot || now >= state.next_schedule_archive_scan_unix_ms;
        if scan_archives {
            state.next_schedule_archive_scan_unix_ms =
                now.saturating_add(ARCHIVE_RECOVERY_INTERVAL_MS);
        }
        let mut pending_archives = BTreeMap::new();
        state
            .pending_schedule_archives
            .retain(|reference, candidate| {
                if candidate.retry_after_unix_ms <= now {
                    pending_archives.insert(reference.clone(), candidate.clone());
                    false
                } else {
                    true
                }
            });
        drop(state);
        let archived_conversation_refs =
            self.archive_terminal_schedule_runs(pending_archives, scan_archives, fault, now)?;
        Ok(ScheduleTickResult {
            admissions: admitted,
            archived_conversation_refs,
        })
    }

    fn scan_schedule_archive_candidates(&self) -> CwrResult<Vec<(String, ScheduleOccurrence)>> {
        let mut state = self.lock()?;
        state.schedule_archive_history_scan_count =
            state.schedule_archive_history_scan_count.saturating_add(1);
        schedule_store(&state)?.archive_candidates()
    }

    fn archive_terminal_schedule_runs(
        &self,
        mut pending: BTreeMap<String, PendingScheduleArchive>,
        scan_history: bool,
        fault: ScheduleFault,
        now: u64,
    ) -> CwrResult<Vec<String>> {
        // New terminal occurrences go straight to archival. A bounded full
        // scan recovers records committed before a crash or transient failure.
        if scan_history {
            match self.scan_schedule_archive_candidates() {
                Ok(candidates) => {
                    for (actor, occurrence) in candidates {
                        pending.insert(
                            occurrence.occurrence_ref.clone(),
                            PendingScheduleArchive {
                                actor,
                                occurrence,
                                retry_after_unix_ms: now,
                            },
                        );
                    }
                }
                Err(error) => {
                    let retry_after = archive_error_retry_after(now, fault);
                    let mut state = self.lock()?;
                    state.next_schedule_archive_scan_unix_ms =
                        now.saturating_add(ARCHIVE_RETRY_INTERVAL_MS);
                    for candidate in pending.values_mut() {
                        candidate.retry_after_unix_ms = retry_after;
                    }
                    state.pending_schedule_archives.extend(pending);
                    return Err(error);
                }
            }
        }
        if pending.is_empty() {
            return Ok(vec![]);
        }
        let mut retry_on_error = pending.clone();
        let attempt = (|| -> CwrResult<(Vec<String>, Vec<PendingScheduleArchive>)> {
            if fault == ScheduleFault::BeforeArchive {
                return invalid("injected before terminal archival");
            }
            let mut archived = vec![];
            let mut retry = vec![];
            for candidate in pending.into_values() {
                let PendingScheduleArchive {
                    actor,
                    mut occurrence,
                    ..
                } = candidate;
                let Some(conversation) = occurrence.conversation_ref.as_deref() else {
                    continue;
                };
                let service = self.bundled_conversation_module_as(SCHEDULE_INGRESS, &actor)?;
                if service.set_archived(conversation, true).is_err()
                    || !self.conversation(conversation)?.is_some_and(|c| c.archived)
                {
                    retry.push(PendingScheduleArchive {
                        actor,
                        occurrence,
                        retry_after_unix_ms: now.saturating_add(ARCHIVE_RETRY_INTERVAL_MS),
                    });
                    continue;
                }
                if fault == ScheduleFault::AfterArchive {
                    return invalid("injected after conversation archival");
                }
                archived.push(conversation.to_owned());
                occurrence.archived = true;
                let state = self.lock()?;
                let mut store = schedule_store(&state)?;
                store
                    .occurrence(&occurrence.occurrence_ref)?
                    .ok_or_else(|| invalid_error("missing archived schedule occurrence"))?;
                store.settle(&occurrence)?;
            }
            Ok((archived, retry))
        })();
        match attempt {
            Ok((archived, retry)) => {
                if !retry.is_empty() {
                    let mut state = self.lock()?;
                    for candidate in retry {
                        state
                            .pending_schedule_archives
                            .insert(candidate.occurrence.occurrence_ref.clone(), candidate);
                    }
                }
                Ok(archived)
            }
            Err(error) => {
                let retry_after = archive_error_retry_after(now, fault);
                for candidate in retry_on_error.values_mut() {
                    candidate.retry_after_unix_ms = retry_after;
                }
                self.lock()?
                    .pending_schedule_archives
                    .extend(retry_on_error);
                Err(error)
            }
        }
    }
}

fn archive_error_retry_after(now: u64, fault: ScheduleFault) -> u64 {
    if fault == ScheduleFault::None {
        now.saturating_add(ARCHIVE_RETRY_INTERVAL_MS)
    } else {
        // Fault injection exercises immediate same-runtime reconciliation.
        now
    }
}

fn admit_pending_schedule(
    context: &LocalInstalledActionContext<'_>,
    state: &mut LocalRuntimeState,
    schedule: &mut InstalledSchedule,
    occurrence: &mut ScheduleOccurrence,
    fault: ScheduleFault,
) -> CwrResult<Option<LocalAdmission>> {
    let reference = &occurrence.occurrence_ref;
    match context.admit_locked(
        state,
        reference,
        &occurrence.definition.target,
        occurrence.definition.input.clone(),
    ) {
        Ok(admission) => {
            if fault == ScheduleFault::AfterAdmission {
                return invalid("injected after admission");
            }
            occurrence.invocation_ref = admission.record.invocation_ref.clone();
            occurrence.conversation_ref = Some(admission.record.conversation_ref.clone());
            occurrence.outcome = "admitted".into();
            schedule.last_admission_error = None;
            Ok(Some(admission))
        }
        Err(error) => {
            // Preserve uncertain commits until receipt reconciliation.
            if !(error.is::<crate::ConfigurationError>() || error.is::<ConversationContextError>())
                || state
                    .admission
                    .admission(SCHEDULE_INGRESS, reference)
                    .is_some()
            {
                return invalid("schedule admission outcome requires reconciliation");
            }
            occurrence.outcome = "admission_failed".into();
            schedule.active_occurrence_ref = None;
            schedule.last_admission_error =
                Some("Exact target, installed context or current authority unavailable".into());
            Ok(None)
        }
    }
}
