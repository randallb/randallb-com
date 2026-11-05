impl LocalSchedulesModule<'_> {
    fn authorize(&self, state: &LocalRuntimeState) -> CwrResult<()> {
        local_catalog(&state.graph, &self.actor)?;
        if self.host_owner_only {
            require_installed_schedule_owner_in_state(state, &self.owner, &self.actor)?;
        }
        // Missing setup or disabled pack remains inspectable. Membership is a
        // host-authenticated installed-context binding, not target availability.
        Ok(())
    }

    /// Installer-only activation. Release declarations cannot select an owner.
    pub fn install(
        &self,
        definition: ScheduleDeclaration,
        now: u64,
    ) -> CwrResult<InstalledSchedule> {
        validate_schedule_ref(&definition.declaration_ref)?;
        if definition.revision.trim().is_empty() {
            return invalid("schedule revision is required");
        }
        let next_due = next_schedule_tick(&definition, now)?;
        let state = self.runtime.lock()?;
        self.authorize(&state)?;
        if !state.admission.context_policy.as_ref().is_some_and(|p| {
            p.contexts
                .get(&self.owner)
                .is_some_and(|actors| actors.contains(&definition.target.coworker_ref))
        }) {
            return invalid("installation is not bound to a host context");
        }
        let route = state
            .graph
            .routes
            .iter()
            .find(|r| {
                r.actor_ref == definition.target.coworker_ref
                    && r.role_ref == definition.target.role_ref
                    && r.actor_integration_ref == definition.target.binding_ref
                    && r.action_name == definition.target.action_name
            })
            .ok_or_else(|| invalid_error("installed schedule target is unavailable"))?;
        if route_action_target(&state.graph.runtime_ref, route) != definition.target {
            return invalid("unsupported installed schedule action contract");
        }
        route.validate_input(&definition.input)?;
        schedule_store(&state)?.install(&self.owner, &self.actor, definition, next_due)
    }

    pub fn list(&self) -> CwrResult<Vec<InstalledSchedule>> {
        let state = self.runtime.lock()?;
        self.authorize(&state)?;
        schedule_store(&state)?.installed(Some(&self.owner))
    }
    pub fn get(&self, reference: &str) -> CwrResult<InstalledSchedule> {
        let state = self.runtime.lock()?;
        self.authorize(&state)?;
        schedule_store(&state)?
            .get(reference, Some(&self.owner))?
            .ok_or_else(|| invalid_error("schedule is unavailable in this context"))
    }
    pub fn set_paused(
        &self,
        reference: &str,
        paused: bool,
        now: u64,
    ) -> CwrResult<InstalledSchedule> {
        let state = self.runtime.lock()?;
        self.authorize(&state)?;
        let mut store = schedule_store(&state)?;
        let schedule = store
            .get(reference, Some(&self.owner))?
            .ok_or_else(|| invalid_error("schedule is unavailable in this context"))?;
        let due = if !paused && schedule.paused {
            next_schedule_tick(&schedule.definition, now)?
        } else {
            schedule.next_due_unix_ms
        };
        store.pause(&self.owner, &schedule, paused, due)
    }

    pub fn recent_runs(&self, reference: &str, limit: usize) -> CwrResult<Vec<ScheduleOccurrence>> {
        if limit == 0 || limit > 100 {
            return invalid("recent run limit must be 1 to 100");
        }
        let state = self.runtime.lock()?;
        self.authorize(&state)?;
        let mut store = schedule_store(&state)?;
        store
            .get(reference, Some(&self.owner))?
            .ok_or_else(|| invalid_error("schedule is unavailable in this context"))?;
        store.occurrences(reference, storage::OccurrenceSelection::Recent(limit))
    }
}

impl LocalRuntime {
    /// Called only through the trusted native installer transport. Installation
    /// metadata supplies owner and current availability; a schedule grants none.
    pub fn configure_installed_schedules_as(
        &self,
        actor: &str,
        owner: &str,
        availability: &str,
        declarations: Vec<crate::v2026_09::upstream::InstalledScheduleActivation>,
        now: u64,
    ) -> CwrResult<()> {
        if !matches!(availability, "ready" | "disabled" | "needs_setup") {
            return invalid("unsupported installed schedule availability");
        }
        let service = self.schedules_module_as(owner, actor)?;
        let current: std::collections::BTreeSet<_> = declarations
            .iter()
            .map(|d| d.declaration_ref.clone())
            .collect();
        if availability == "ready" {
            for declaration in declarations {
                let target = {
                    let state = self.lock()?;
                    let matches = state
                        .graph
                        .routes
                        .iter()
                        .filter(|r| {
                            r.actor_ref == declaration.target_actor_ref
                                && r.action_name == declaration.action_name
                        })
                        .collect::<Vec<_>>();
                    if matches.len() != 1 {
                        return invalid("schedule target is missing or ambiguous");
                    }
                    route_action_target(&state.graph.runtime_ref, matches[0])
                };
                service.install(
                    ScheduleDeclaration {
                        declaration_ref: declaration.declaration_ref,
                        revision: declaration.revision,
                        target,
                        input: declaration.input,
                        anchor_unix_ms: declaration.anchor_unix_ms,
                        interval_ms: declaration.interval_ms,
                    },
                    now,
                )?;
            }
        }
        let state = self.lock()?;
        service.authorize(&state)?;
        let mut store = schedule_store(&state)?;
        let mut changes = Vec::new();
        for schedule in store.installed(Some(owner))? {
            let effective = if availability == "ready"
                && !current.contains(&schedule.definition.declaration_ref)
            {
                "removed"
            } else {
                availability
            };
            if schedule.availability != effective {
                let due = if effective == "ready" {
                    next_schedule_tick(&schedule.definition, now)?
                } else {
                    schedule.next_due_unix_ms
                };
                changes.push((schedule, effective.to_owned(), due));
            }
        }
        store.availability(owner, changes)
    }
}
