impl LocalRuntime {
    fn observe_invocation_started(
        &self,
        record: &ActionInvocationRecord,
        observer: &mut dyn FnMut(LocalRuntimeEvent),
        metrics: &mut crate::metrics::AttemptMetrics,
    ) {
        metrics.measure_setup("invocation_started_observer", || {
            observer(LocalRuntimeEvent::InvocationStarted {
                invocation_ref: record.invocation_ref.clone(),
                generation: record.attempt_count,
                target_actor_ref: record.target_actor_ref.clone(),
            })
        });
    }

    fn prepare_adapter_dispatch(
        &self,
        route: &mut crate::adapter::ResolvedProcessAdapterRoute,
        invocation: &mut AdapterInvocation,
        record: &ActionInvocationRecord,
        queue: &Rc<RefCell<VecDeque<String>>>,
        observer: &mut dyn FnMut(LocalRuntimeEvent),
        metrics: &mut crate::metrics::AttemptMetrics,
    ) -> CwrResult<bool> {
        if connections::deliver(&self.lock()?.cwr_toml, route, invocation).is_err() {
            self.publish_claim_failure(
                record,
                "Could not access this project's local setup. Review Connections in Settings."
                    .into(),
                queue,
                observer,
            )?;
            return Ok(false);
        }
        self.ensure_adapter_ready(route, invocation, record, queue, observer, metrics)
    }

    fn ensure_adapter_ready(
        &self,
        route: &crate::adapter::ResolvedProcessAdapterRoute,
        invocation: &AdapterInvocation,
        record: &ActionInvocationRecord,
        queue: &Rc<RefCell<VecDeque<String>>>,
        observer: &mut dyn FnMut(LocalRuntimeEvent),
        metrics: &mut crate::metrics::AttemptMetrics,
    ) -> CwrResult<bool> {
        let readiness = metrics.measure_setup("adapter_readiness", || route.readiness_for_invocation(invocation))?;
        if readiness.is_ready() {
            return Ok(true);
        }
        metrics.outcome = "provider_unavailable".into();
        self.publish_claim_provider_unavailable(record, readiness, queue, observer)?;
        Ok(false)
    }

    fn claim_execution_or_fail(
        &self,
        invocation_ref: &str,
        stop: &ProcessAdapterStopControl,
        queue: &Rc<RefCell<VecDeque<String>>>,
        observer: &mut dyn FnMut(LocalRuntimeEvent),
    ) -> CwrResult<Option<LocalExecutionClaim>> {
        match self.claim_execution(invocation_ref, stop) {
            Ok(claim) => Ok(claim),
            Err(error) => {
                let record = self
                    .lock()?
                    .admission
                    .invocation_record(invocation_ref)
                    .cloned()
                    .ok_or_else(|| invalid_error("failed claim has no invocation"))?;
                if record.attempt_active {
                    return Err(error);
                }
                if !record.state.is_terminal() {
                    self.publish_claim_failure(&record, error.to_string(), queue, observer)?;
                }
                Ok(None)
            }
        }
    }

    fn publish_claim_failure(
        &self,
        record: &ActionInvocationRecord,
        error: String,
        queue: &Rc<RefCell<VecDeque<String>>>,
        observer: &mut dyn FnMut(LocalRuntimeEvent),
    ) -> CwrResult<()> {
        self.lock()?.active.remove(&record.invocation_ref);
        let settlement = self.publish_execution_failure(record, error, queue, observer);
        self.recover_claim_after_error(&record.invocation_ref, settlement)
    }

    fn publish_claim_provider_unavailable(
        &self,
        record: &ActionInvocationRecord,
        readiness: AdapterReadiness,
        queue: &Rc<RefCell<VecDeque<String>>>,
        observer: &mut dyn FnMut(LocalRuntimeEvent),
    ) -> CwrResult<()> {
        self.lock()?.active.remove(&record.invocation_ref);
        let settlement = self.publish_provider_unavailable(record, readiness, queue, observer);
        self.recover_claim_after_error(&record.invocation_ref, settlement)
    }

    fn recover_claim_after_error(
        &self,
        invocation_ref: &str,
        settlement: CwrResult<()>,
    ) -> CwrResult<()> {
        let Err(error) = settlement else {
            return Ok(());
        };
        if let Ok(mut state) = self.lock() {
            state.active.remove(invocation_ref);
            let _ = state
                .admission
                .recover_interrupted_invocation(invocation_ref);
        }
        Err(error)
    }

    fn settle_stopped_execution(&self, invocation_ref: &str) -> CwrResult<bool> {
        loop {
            let mut state = self.lock()?;
            if state
                .admission
                .invocation_record(invocation_ref)
                .is_some_and(|record| {
                    matches!(
                        record.state,
                        ActionInvocationState::Stopping | ActionInvocationState::Stopped
                    )
                })
            {
                state.settle_stopped_invocation(invocation_ref, current_unix_ms())?;
                return Ok(true);
            }
            // Archive signals adapters before the transaction can commit while
            // upstream acknowledgements are outstanding. That is cancellation,
            // not an adapter failure. Let the archive settle the selected tree
            // before clearing this worker's active attempt.
            if state.admission.invocation_record(invocation_ref)
                .is_none_or(|record| record.state.is_terminal())
                || !state.stopping_roots.keys().any(|root| descends_from(&state, invocation_ref, root)) {
                return Ok(false);
            }
            drop(state);
            thread::sleep(Duration::from_millis(10));
        }
    }
}
