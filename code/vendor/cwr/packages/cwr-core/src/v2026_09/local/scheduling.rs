/// A reserved execution slot. Hosts create a worker only after obtaining a task
/// and retain it until that worker and its output publisher have finished.
pub struct LocalExecutionTask {
    runtime: LocalRuntime,
    _permit: LocalExecutionPermit,
    record: ActionInvocationRecord,
    execution_group_ref: String,
    control: bool,
}

impl LocalExecutionTask {
    pub fn record(&self) -> &ActionInvocationRecord {
        &self.record
    }

    pub fn execute_with_model_output(
        &mut self,
        model_output: &AdapterModelOutputMailbox,
        mut observer: impl FnMut(LocalRuntimeEvent),
    ) -> CwrResult<()> {
        if self.control {
            return self.runtime.execute_requested_ref(
                &self.record.invocation_ref,
                model_output,
                &mut observer,
            );
        }
        // Re-read under the execution reservation: stopped or drained work may
        // have changed since selection. claim_execution enforces those fences.
        let refs = {
            let mut state = self.runtime.lock()?;
            take_ordered_runnable_refs_for_group(&mut state, &self.execution_group_ref)
        };
        self.runtime.execute_refs(
            &self.execution_group_ref,
            refs,
            model_output,
            &mut observer,
        )
    }
}

impl LocalRuntime {
    /// Selects retained work without waiting for capacity. Recovery and new
    /// admissions share this queue; queued records consume no worker threads.
    pub fn try_schedule_execution(&self) -> CwrResult<Option<LocalExecutionTask>> {
        let refs = {
            let mut state = self.lock()?;
            self.sync_archive_reservations(&mut state)?;
            if state.shutting_down {
                return Ok(None);
            }
            order_runnable_refs(
                &state.admission,
                state
                    .admission
                    .runnable_invocation_refs()
                    .into_iter()
                    .collect(),
            )
        };
        // A separate, single control slot keeps stop/reload available while all
        // conversation workers are occupied, including in the same conversation.
        for control in [true, false] {
            for invocation_ref in &refs {
                let (execution_group_ref, archive_control) = {
                    let state = self.lock()?;
                    let Some(record) = state.admission.invocation_record(invocation_ref) else {
                        continue;
                    };
                    if state.admission.context_policy.as_ref().is_some_and(|policy| policy.suspended) && record.target_actor_ref != INTRINSIC_CWR_ACTOR_REF { continue; }
                    if (record.target_actor_ref == INTRINSIC_CWR_ACTOR_REF) != control
                        || !record.is_runnable()
                        || !drain_allows(&state, invocation_ref)
                        || require_no_stop_fence(&state, invocation_ref).is_err()
                    {
                        continue;
                    }
                    (execution_group_ref(&state.admission, record).map(str::to_owned),
                        control && matches!(record.action_name.as_str(),
                            "archive--conversation" | "restore--conversation"))
                };
                let Some(execution_group_ref) = execution_group_ref else {
                    continue;
                };
                // Archive controls can await provider acknowledgements. Keep
                // them serialized separately so stop/reload remain available.
                let coordinator = if archive_control {
                    &self.archive_execution
                } else if control {
                    &self.control_execution
                } else {
                    &self.execution
                };
                let Some(permit) = coordinator.try_acquire(&execution_group_ref, || {
                    if control {
                        Ok(true)
                    } else {
                        self.invocation_is_next_in_group(invocation_ref)
                    }
                })?
                else {
                    continue;
                };
                let record = {
                    let state = self.lock()?;
                    let Some(record) = state.admission.invocation_record(invocation_ref) else {
                        continue;
                    };
                    if !record.is_runnable()
                        || !drain_allows(&state, invocation_ref)
                        || require_no_stop_fence(&state, invocation_ref).is_err()
                    {
                        continue;
                    }
                    record.clone()
                };
                return Ok(Some(LocalExecutionTask {
                    runtime: self.clone(),
                    _permit: permit,
                    record,
                    execution_group_ref,
                    control,
                }));
            }
        }
        Ok(None)
    }
}
