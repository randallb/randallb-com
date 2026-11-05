struct LocalExecutionClaim {
    claimed: crate::v2026_09::conversation::ClaimedConversationInvocation,
    graph: ResolvedCwrGraph,
    claimed_at: u64,
    attempt_started: std::time::Instant,
}

impl LocalRuntime {
    pub fn execute_admission<F>(
        &self,
        admission: &LocalAdmission,
        observer: F,
    ) -> CwrResult<LocalTurn>
    where
        F: FnMut(LocalRuntimeEvent),
    {
        self.execute_admission_with_model_output(
            admission,
            &AdapterModelOutputMailbox::discard(),
            observer,
        )
    }

    pub fn execute_admission_with_model_output<F>(
        &self,
        admission: &LocalAdmission,
        model_output: &AdapterModelOutputMailbox,
        mut observer: F,
    ) -> CwrResult<LocalTurn>
    where
        F: FnMut(LocalRuntimeEvent),
    {
        // Runtime control work must be able to inspect or stop an earlier turn
        // in the same conversation instead of waiting behind that turn.
        if admission.record.target_actor_ref.as_deref() == Some(INTRINSIC_CWR_ACTOR_REF) {
            if let Some(invocation_ref) = admission.invocation_ref() {
                self.execute_requested_ref(invocation_ref, model_output, &mut observer)?;
            }
            return self.turn(&admission.record);
        }
        let execution_group_ref = {
            let state = self.lock()?;
            admission
                .invocation_ref()
                .and_then(|reference| state.admission.invocation_record(reference))
                .and_then(|record| execution_group_ref(&state.admission, record))
                .unwrap_or(admission.conversation_ref())
                .to_owned()
        };
        while self.execute_available_pending_group(
            &execution_group_ref,
            model_output,
            &mut observer,
        )? {}
        let execution = self.execution.acquire(&execution_group_ref, || {
            admission
                .invocation_ref()
                .map_or(Ok(true), |invocation_ref| {
                    self.invocation_is_next_in_group(invocation_ref)
                })
        })?;
        let refs = {
            let mut state = self.lock()?;
            take_ordered_pending_and_extra_refs_for_group(
                &mut state,
                &execution_group_ref,
                admission.invocation_ref().map(str::to_owned),
            )
        };
        self.execute_refs(&execution_group_ref, refs, model_output, &mut observer)?;
        drop(execution);
        if let Some(invocation_ref) = admission.invocation_ref() {
            self.execute_independent_descendants(invocation_ref, model_output, &mut observer)?;
        }
        self.turn(&admission.record)
    }

    pub fn execute_pending<F>(&self, mut observer: F) -> CwrResult<Vec<String>>
    where
        F: FnMut(LocalRuntimeEvent),
    {
        self.execute_pending_with_model_output(&AdapterModelOutputMailbox::discard(), &mut observer)
    }

    pub fn execute_pending_with_model_output<F>(
        &self,
        model_output: &AdapterModelOutputMailbox,
        mut observer: F,
    ) -> CwrResult<Vec<String>>
    where
        F: FnMut(LocalRuntimeEvent),
    {
        let refs = {
            let state = self.lock()?;
            let refs = state
                .admission
                .runnable_invocation_refs()
                .into_iter()
                .collect();
            order_runnable_refs(&state.admission, refs)
        };
        for (execution_group_ref, group_refs) in self.group_refs_by_execution(&refs)? {
            let first_ref = group_refs.first();
            let _execution = self.execution.acquire(&execution_group_ref, || {
                first_ref.map_or(Ok(true), |invocation_ref| {
                    self.invocation_is_next_in_group(invocation_ref)
                })
            })?;
            let refs = {
                let mut state = self.lock()?;
                take_ordered_runnable_refs_for_group(&mut state, &execution_group_ref)
            };
            self.execute_refs(&execution_group_ref, refs, model_output, &mut observer)?;
        }
        Ok(refs)
    }

    fn execute_available_pending_group(
        &self,
        execution_group_ref: &str,
        model_output: &AdapterModelOutputMailbox,
        observer: &mut dyn FnMut(LocalRuntimeEvent),
    ) -> CwrResult<bool> {
        let pending = self.lock()?.pending_invocation_refs.clone();
        for (pending_group_ref, group_refs) in self.group_refs_by_execution(&pending)? {
            let independently_scheduled_group = {
                let state = self.lock()?;
                group_refs
                    .first()
                    .and_then(|reference| state.admission.invocation_record(reference))
                    .is_some_and(|record| {
                        belongs_to_independently_scheduled_group(&state.admission, record)
                    })
            };
            if pending_group_ref != execution_group_ref && independently_scheduled_group {
                continue;
            }
            let first_ref = group_refs.first();
            let Some(_execution) = self.execution.try_acquire(&pending_group_ref, || {
                first_ref.map_or(Ok(true), |invocation_ref| {
                    self.invocation_is_next_in_group(invocation_ref)
                })
            })?
            else {
                continue;
            };
            let refs = {
                let mut state = self.lock()?;
                take_ordered_pending_refs_for_group(&mut state, &pending_group_ref)
            };
            self.execute_refs(&pending_group_ref, refs, model_output, observer)?;
            return Ok(true);
        }
        Ok(false)
    }

    fn invocation_is_next_in_group(&self, invocation_ref: &str) -> CwrResult<bool> {
        let state = self.lock()?;
        let Some(target) = state.admission.invocation_record(invocation_ref) else {
            return Ok(true);
        };
        if !target.is_runnable() {
            return Ok(true);
        }
        let Some(target_group_ref) = execution_group_ref(&state.admission, target) else {
            return Ok(true);
        };
        if group_has_stopping_execution(&state, target) { return Ok(false); }
        for admission_record in state.admission.admissions() {
            let Some(candidate_ref) = admission_record.invocation_ref.as_deref() else {
                continue;
            };
            if candidate_ref == invocation_ref {
                return Ok(true);
            }
            if state
                .admission
                .invocation_record(candidate_ref)
                .is_some_and(|candidate| {
                    candidate.is_runnable()
                        && execution_group_ref(&state.admission, candidate)
                            == Some(target_group_ref)
                })
            {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn group_refs_by_execution(&self, refs: &[String]) -> CwrResult<Vec<(String, Vec<String>)>> {
        let state = self.lock()?;
        let mut groups = Vec::<(String, Vec<String>)>::new();
        for invocation_ref in refs {
            let Some(execution_group_ref) = state
                .admission
                .invocation_record(invocation_ref)
                .and_then(|record| execution_group_ref(&state.admission, record))
                .map(str::to_owned)
            else {
                continue;
            };
            if let Some((_, refs)) = groups
                .iter_mut()
                .find(|(existing, _)| *existing == execution_group_ref)
            {
                refs.push(invocation_ref.clone());
            } else {
                groups.push((execution_group_ref, vec![invocation_ref.clone()]));
            }
        }
        Ok(groups)
    }

    pub fn turn(&self, record: &ConversationAdmissionRecord) -> CwrResult<LocalTurn> {
        let state = self.lock()?;
        let admission = state
            .admission
            .admission(&record.integration_ref, &record.provider_event_ref)
            .cloned()
            .ok_or_else(|| invalid_error("local conversation admission is unavailable"))?;
        let conversation = state
            .admission
            .full_conversation(&admission.conversation_ref)?
            .ok_or_else(|| invalid_error("local conversation is unavailable"))?;
        let invocation = admission
            .invocation_ref
            .as_deref()
            .and_then(|invocation_ref| state.admission.invocation_record(invocation_ref))
            .cloned();
        Ok(LocalTurn {
            admission,
            conversation,
            invocation,
        })
    }

    fn execute_refs(
        &self,
        execution_group_ref: &str,
        invocation_refs: impl IntoIterator<Item = String>,
        model_output: &AdapterModelOutputMailbox,
        observer: &mut dyn FnMut(LocalRuntimeEvent),
    ) -> CwrResult<()> {
        let queue = Rc::new(RefCell::new(
            invocation_refs.into_iter().collect::<VecDeque<_>>(),
        ));
        let mut seen = BTreeSet::new();
        loop {
            let next = { queue.borrow_mut().pop_front() };
            let Some(invocation_ref) = next else {
                break;
            };
            if !seen.insert(invocation_ref.clone()) {
                let runnable = self
                    .lock()?
                    .admission
                    .invocation_record(&invocation_ref)
                    .is_some_and(ActionInvocationRecord::is_runnable);
                if !runnable {
                    continue;
                }
            }
            self.execute_one(&invocation_ref, Rc::clone(&queue), model_output, observer)?;
            let pending = {
                let mut state = self.lock()?;
                take_ordered_pending_refs_for_group(&mut state, execution_group_ref)
            };
            let mut queue = queue.borrow_mut();
            for invocation_ref in pending.into_iter().rev() {
                queue.push_front(invocation_ref);
            }
        }
        Ok(())
    }

    /// Executes only the requested runtime-control invocation. It deliberately
    /// leaves unrelated pending work for the normal per-conversation permit.
    fn execute_requested_ref(
        &self,
        invocation_ref: &str,
        model_output: &AdapterModelOutputMailbox,
        observer: &mut dyn FnMut(LocalRuntimeEvent),
    ) -> CwrResult<()> {
        self.execute_one(
            invocation_ref,
            Rc::new(RefCell::new(VecDeque::new())),
            model_output,
            observer,
        )
    }

    fn claim_execution(
        &self,
        invocation_ref: &str,
        stop: &ProcessAdapterStopControl,
    ) -> CwrResult<Option<LocalExecutionClaim>> {
        let mut state = self.lock()?;
        if state.shutting_down || !drain_allows(&state, invocation_ref) || require_no_stop_fence(&state, invocation_ref).is_err() {
            return Ok(None);
        }
        let Some(record) = state.admission.invocation_record(invocation_ref) else {
            return invalid(format!("unknown invocation ref {invocation_ref}"));
        };
        if !record.is_runnable() {
            return Ok(None);
        }
        if group_has_stopping_execution(&state, record) { return Ok(None); }
        if state.admission.context_policy.as_ref().is_some_and(|policy| policy.suspended) && record.target_actor_ref != INTRINSIC_CWR_ACTOR_REF { return Ok(None); }
        state.admission.check_context_execution(&state.graph, record)?;
        require_current_request_authority(&state, record)?;
        if state
            .active
            .insert(invocation_ref.into(), stop.clone())
            .is_some()
        {
            return invalid(format!(
                "invocation {invocation_ref} already has an active claim"
            ));
        }
        let claimed_at = current_unix_ms();
        let attempt_started = std::time::Instant::now();
        match state.admission.claim_invocation(invocation_ref, claimed_at) {
            Ok(ConversationInvocationClaimOutcome::Claimed(claimed)) => {
                Ok(Some(LocalExecutionClaim {
                    claimed: *claimed,
                    graph: state.graph.clone(),
                    claimed_at,
                    attempt_started,
                }))
            }
            Ok(ConversationInvocationClaimOutcome::NotRunnable { .. }) => {
                state.active.remove(invocation_ref);
                Ok(None)
            }
            Err(error) => {
                state.active.remove(invocation_ref);
                Err(error)
            }
        }
    }

    fn execute_one(
        &self,
        invocation_ref: &str,
        queue: Rc<RefCell<VecDeque<String>>>,
        model_output: &AdapterModelOutputMailbox,
        observer: &mut dyn FnMut(LocalRuntimeEvent),
    ) -> CwrResult<()> {
        let stop = ProcessAdapterStopControl::new();
        let Some(LocalExecutionClaim {
            claimed,
            graph,
            claimed_at,
            attempt_started,
        }) = self.claim_execution_or_fail(invocation_ref, &stop, &queue, observer)?
        else {
            return Ok(());
        };

        let claim_persistence_ms = crate::metrics::elapsed_ms(attempt_started);
        let source_message_ref = claimed.source_message_ref.clone();
        let record = claimed.claim.invocation.clone();
        let mut metrics = self.start_attempt_metrics(
            &record,
            &claimed.adapter_invocation,
            claimed_at,
            attempt_started,
            claim_persistence_ms,
        );
        let timing_sink = metrics
            .recorder
            .adapter_sink(metrics.context.clone(), attempt_started);

        self.observe_invocation_started(&record, observer, &mut metrics);
        let attempt_count = record.attempt_count;
        let mut invocation = claimed.adapter_invocation;
        if let Some(route) = invocation.adapter_params.get("upstream_route") {
            let route: UpstreamRoute = serde_json::from_value(route.clone())?;
            let outcome = self.execute_upstream_work(&record, route, &queue, observer);
            self.finish_attempt_metrics(&mut metrics);
            return outcome;
        }
        let mut route = match metrics.measure_setup("route_selection", || {
            selected_process_route(&graph, &record, &mut invocation)
        }) {
            Ok(route) => route,
            Err(error) => {
                self.publish_claim_failure(&record, error.to_string(), &queue, observer)?;
                self.finish_attempt_metrics(&mut metrics);
                return Ok(());
            }
        };
        if let Some(result) = metrics.measure_setup("action_only_message", || {
            self.finish_action_only_message(&graph, &record, &source_message_ref, &queue, observer)
        })? {
            metrics.outcome = if result.is_ok() {
                "completed"
            } else {
                "failed"
            }
            .into();
            self.finish_attempt_metrics(&mut metrics);
            return result;
        }
        if let Err(error) = metrics.measure_setup("lifecycle_operations", || {
            self.install_lifecycle_operations(&mut invocation, &record)
        }) {
            self.publish_claim_failure(&record, error.to_string(), &queue, observer)?;
            self.finish_attempt_metrics(&mut metrics);
            return Ok(());
        }
        let retention = metrics.measure_setup("invocation_retention", || {
            self.lock()?
                .admission
                .retain_execution_invocation(invocation_ref, &invocation)
        });
        if let Err(error) = retention {
            self.publish_claim_failure(&record, error.to_string(), &queue, observer)?;
            self.finish_attempt_metrics(&mut metrics);
            return Ok(());
        }

        if record.target_actor_ref == INTRINSIC_CWR_ACTOR_REF {
            return self.finish_intrinsic_execution(
                &record,
                &invocation,
                &queue,
                observer,
                &mut metrics,
            );
        }

        // Retain references above; resolve values only for this process dispatch.
        if !self.prepare_adapter_dispatch(
            &mut route,
            &mut invocation,
            &record,
            &queue,
            observer,
            &mut metrics,
        )? {
            self.finish_attempt_metrics(&mut metrics);
            return Ok(());
        }
        let observer = Rc::new(RefCell::new(observer));
        let progress_failure = Rc::new(RefCell::new(None::<String>));
        let progress_runtime = self.clone();
        let progress_ref = invocation_ref.to_owned();
        let progress_actor_ref = record.target_actor_ref.clone();
        let progress_stop = stop.clone();
        let progress_observer = Rc::clone(&observer);
        let progress_error = Rc::clone(&progress_failure);
        let activity_runtime = self.clone();
        let activity_ref = invocation_ref.to_owned();
        let activity_actor_ref = record.target_actor_ref.clone();
        let activity_observer = Rc::clone(&observer);
        let host_runtime = self.clone();
        let host_record = record.clone();
        let host_queue = Rc::clone(&queue);
        let host_observer = Rc::clone(&observer);
        metrics.recorder.record(
            &metrics.context,
            "runtime",
            "adapter_dispatch",
            json!({
                "attempt_elapsed_ms": crate::metrics::elapsed_ms(attempt_started)
            }),
        );
        let outcome = route.run_with_timing(
            invocation,
            &stop,
            crate::adapter::AdapterOutputSinks {
                model_output,
                timing: &timing_sink,
            },
            move |progress| match progress_runtime.record_progress_activity(
                &progress_ref,
                progress.status,
                progress.summary,
                progress.published,
                progress.progress,
            ) {
                Ok(activity) => (progress_observer.borrow_mut())(LocalRuntimeEvent::Activity {
                    invocation_ref: progress_ref.clone(),
                    target_actor_ref: progress_actor_ref.clone(),
                    activity: Box::new(activity),
                }),
                Err(error) => {
                    *progress_error.borrow_mut() = Some(error.to_string());
                    progress_stop.stop();
                }
            },
            move |activity| {
                let retained = activity_runtime.record_adapter_activity(
                    &activity_ref,
                    attempt_count,
                    activity.event,
                )?;
                if let Some(activity) = retained {
                    (activity_observer.borrow_mut())(LocalRuntimeEvent::Activity {
                        invocation_ref: activity_ref.clone(),
                        target_actor_ref: activity_actor_ref.clone(),
                        activity: Box::new(activity),
                    });
                }
                Ok(())
            },
            move |call| {
                host_runtime.handle_host_call(
                    call,
                    &host_record,
                    &host_queue,
                    model_output,
                    &mut **host_observer.borrow_mut(),
                )
            },
        );
        if let Err(error) = &outcome {
            self.diagnostics.record_failure(&metrics.context, error.as_ref());
        }
        metrics.outcome = match &outcome {
            Ok(ProcessAdapterOutcome::Stopped) => "stopped",
            Ok(ProcessAdapterOutcome::Completed(_)) => "completed",
            Err(_) => "failed",
        }
        .into();
        self.lock()?.active.remove(invocation_ref);

        if let Some(error) = progress_failure.borrow_mut().take() {
            metrics.outcome = "failed".into();
            let settlement = self.publish_execution_failure(
                &record,
                error,
                &queue,
                &mut **observer.borrow_mut(),
            );
            let result = self.recover_claim_after_error(invocation_ref, settlement);
            self.finish_attempt_metrics(&mut metrics);
            return result;
        }
        let settlement = self.settle_adapter_outcome(
            outcome,
            &record,
            &queue,
            &mut **observer.borrow_mut(),
            &mut metrics,
        );
        let result = self.recover_claim_after_error(invocation_ref, settlement);
        self.finish_attempt_metrics(&mut metrics);
        if result.is_err() {
            metrics.outcome = "failed".into();
        }
        result
    }

    fn settle_adapter_outcome(
        &self,
        outcome: CwrResult<ProcessAdapterOutcome>,
        record: &ActionInvocationRecord,
        queue: &Rc<RefCell<VecDeque<String>>>,
        observer: &mut dyn FnMut(LocalRuntimeEvent),
        metrics: &mut crate::metrics::AttemptMetrics,
    ) -> CwrResult<()> {
        if self.settle_stopped_execution(&record.invocation_ref)? {
            metrics.outcome = "stopped".into();
            return Ok(());
        }
        match outcome {
            Ok(ProcessAdapterOutcome::Stopped) => self.publish_execution_failure(
                record,
                "adapter stopped before the invocation was stopped".into(),
                queue,
                observer,
            ),
            Ok(ProcessAdapterOutcome::Completed(terminal)) => {
                self.publish_terminal(record, *terminal, queue, observer)
            }
            Err(error) => {
                metrics
                    .recorder
                    .record_adapter_process_failure(&metrics.context, error.as_ref());
                self.publish_execution_failure(record, error.to_string(), queue, observer)
            }
        }
    }

}

include!("execution/failure.rs");
include!("execution/detached.rs");
include!("execution/synchronous.rs");
