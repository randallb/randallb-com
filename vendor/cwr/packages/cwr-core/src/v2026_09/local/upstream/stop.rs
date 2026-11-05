impl LocalRuntime {
    pub fn acknowledge_upstream_stop(&self, owner: &str, invocation: &str) -> CwrResult<()> {
        let mut state = self.lock()?;
        if let Some(pending) = state.upstream_work.get(invocation) {
            if !state.upstream.owns_route(owner, &pending.route) || !pending.stop_requested {
                return invalid("provider cannot acknowledge this stop");
            }
            state
                .upstream_work
                .get_mut(invocation)
                .unwrap()
                .stop_acknowledged = true;
            if state
                .admission
                .invocation_record(invocation)
                .is_some_and(|record| record.state == ActionInvocationState::Stopping)
            {
                state.settle_stopped_invocation(invocation, current_unix_ms())?;
                if !state.active.contains_key(invocation) {
                    state.upstream_work.remove(invocation);
                }
            }
            return Ok(());
        }
        state
            .admission
            .invocation_record(invocation)
            .filter(|record| {
                record.state.is_terminal() && record.stop_requested_at_unix_ms.is_some()
            })
            .ok_or_else(|| invalid_error("unknown upstream invocation"))?;
        let route = retained_upstream_route(&state, invocation)
            .ok_or_else(|| invalid_error("unknown upstream invocation"))?;
        if !state.upstream.owns_route(owner, &route) {
            return invalid("provider cannot acknowledge this stop");
        }
        state.settle_stopped_invocation(invocation, current_unix_ms())?;
        Ok(())
    }

    fn prepare_upstream_stop(
        &self,
        invocation: &str,
    ) -> CwrResult<Option<(UpstreamWriter, std::sync::mpsc::SyncSender<UpstreamEvent>)>> {
        let delivery = {
            let mut state = self.lock()?;
            let Some(pending) = state.upstream_work.get_mut(invocation) else {
                return Ok(None);
            };
            if pending.stop_acknowledged && pending.terminal.is_none() {
                return Ok(None);
            }
            pending.stop_requested = true;
            let session = pending.route.provider_session_ref.clone();
            state
                .upstream_providers
                .get(&session)
                .map(|provider| {
                    (
                        Arc::clone(&provider.writer),
                        provider.deadline_stop_sender.clone(),
                    )
                })
        };
        Ok(delivery)
    }

    fn request_upstream_stop(&self, invocation: &str) -> CwrResult<()> {
        let delivery = self.prepare_upstream_stop(invocation)?;
        if let Some((writer, _)) = delivery {
            if writer(UpstreamEvent::StopRequested {
                invocation_ref: invocation.into(),
            })
            .is_err()
            {
                if let Some(pending) = self.lock()?.upstream_work.get_mut(invocation) {
                    pending.disconnected = true;
                }
            }
        }
        Ok(())
    }

    fn request_upstream_stop_without_wait(&self, invocation: String) -> CwrResult<()> {
        let Some((_, sender)) = self.prepare_upstream_stop(&invocation)? else {
            return Ok(());
        };
        // Keep the caller nonblocking, but retain full-queue delivery for retry.
        let mut event = match sender.try_send(UpstreamEvent::StopRequested {
            invocation_ref: invocation.clone(),
        }) {
            Ok(()) => return Ok(()),
            Err(std::sync::mpsc::TrySendError::Disconnected(_)) => {
                if let Some(pending) = self.lock()?.upstream_work.get_mut(&invocation) {
                    pending.disconnected = true;
                }
                return Ok(());
            }
            Err(std::sync::mpsc::TrySendError::Full(event)) => event,
        };
        {
            let mut state = self.lock()?;
            let Some(pending) = state.upstream_work.get_mut(&invocation) else {
                return Ok(());
            };
            if matches!(pending.stop_delivery, UpstreamStopDelivery::Retrying) { return Ok(()); }
            pending.stop_delivery = UpstreamStopDelivery::Retrying;
        }
        let weak = Arc::downgrade(&self.inner);
        let retry_invocation = invocation.clone();
        let result = thread::Builder::new().name("upstream-stop-retry".into()).spawn(move || {
            loop {
                thread::sleep(Duration::from_millis(10));
                let Some(runtime) = weak.upgrade() else { break; };
                let Ok(mut state) = runtime.lock() else { break; };
                let Some(pending) = state.upstream_work.get_mut(&retry_invocation) else { break; };
                if pending.stop_acknowledged || pending.disconnected || pending.terminal.is_some() {
                    pending.stop_delivery = UpstreamStopDelivery::Idle;
                    break;
                }
                match sender.try_send(event) {
                    Ok(()) => { pending.stop_delivery = UpstreamStopDelivery::Idle; break; }
                    Err(std::sync::mpsc::TrySendError::Full(retained)) => event = retained,
                    Err(std::sync::mpsc::TrySendError::Disconnected(_)) => {
                        pending.stop_delivery = UpstreamStopDelivery::Idle;
                        pending.disconnected = true;
                        break;
                    }
                }
            }
        });
        if let Err(error) = result {
            if let Some(pending) = self.lock()?.upstream_work.get_mut(&invocation) {
                pending.stop_delivery = UpstreamStopDelivery::Idle;
            }
            return Err(error.into());
        }
        Ok(())
    }

    #[cfg(test)]
    fn await_upstream_stop(&self, invocation: &str) -> CwrResult<()> {
        self.request_upstream_stop(invocation)?;
        self.wait_upstream_stop(invocation)
    }

    #[cfg(test)]
    fn wait_upstream_stop(&self, invocation: &str) -> CwrResult<()> {
        self.wait_upstream_stop_until(invocation, None)
    }

    fn wait_upstream_stop_until(&self, invocation: &str, canceling: Option<&str>) -> CwrResult<()> {
        loop {
            let state = self.lock()?;
            if canceling.is_some_and(|reference| {
                state
                    .admission
                    .invocation_record(reference)
                    .is_none_or(|record| record.state.is_terminal())
            }) {
                return Ok(());
            }
            let Some(pending) = state.upstream_work.get(invocation) else {
                return Ok(());
            };
            if (pending.stop_acknowledged && pending.terminal.is_none())
                || state
                    .admission
                    .invocation_record(invocation)
                    .is_some_and(|record| record.state.is_terminal() && record.state != ActionInvocationState::Stopping)
            {
                return Ok(());
            }
            drop(state);
            thread::sleep(Duration::from_millis(10));
        }
    }
}

struct UpstreamStopFence {
    runtime: LocalRuntime,
    root: String,
}

impl Drop for UpstreamStopFence {
    fn drop(&mut self) {
        if let Ok(mut state) = self.runtime.lock() {
            if let Some(count) = state.stopping_roots.get_mut(&self.root) {
                *count -= 1;
                if *count == 0 {
                    state.stopping_roots.remove(&self.root);
                }
            }
        }
    }
}

fn descends_from(state: &LocalRuntimeState, invocation: &str, root: &str) -> bool {
    let mut current = Some(invocation);
    let mut seen = BTreeSet::new();
    while let Some(reference) = current {
        if reference == root {
            return true;
        }
        if !seen.insert(reference) {
            return false;
        }
        current = state
            .admission
            .invocation_record(reference)
            .and_then(|record| record.parent_invocation_ref.as_deref());
    }
    false
}

fn require_no_stop_fence(state: &LocalRuntimeState, invocation: &str) -> CwrResult<()> {
    let independent_control = state
        .admission
        .invocation_record(invocation)
        .is_some_and(|record| {
            record.target_actor_ref == INTRINSIC_CWR_ACTOR_REF
                && record.parent_invocation_ref.is_none()
                && !matches!(
                    record.action_name.as_str(),
                    "archive--conversation" | "restore--conversation"
                )
        });
    if !independent_control
        && state
            .admission
            .invocation_record(invocation)
            .and_then(|record| record.conversation_ref.as_ref())
            .is_some_and(|conversation| state.archiving_conversations.contains_key(conversation))
        || state
        .stopping_roots
        .keys()
        .any(|root| descends_from(state, invocation, root))
    {
        return invalid("invocation is stopping and cannot start more work");
    }
    Ok(())
}

impl LocalRuntime {
    #[cfg(test)]
    fn fence_upstream_stop_tree(&self, root: &str) -> CwrResult<UpstreamStopFence> {
        let targets = {
            let mut state = self.lock()?;
            *state.stopping_roots.entry(root.into()).or_default() += 1;
            state
                .upstream_work
                .keys()
                .filter(|reference| descends_from(&state, reference, root))
                .cloned()
                .collect::<Vec<_>>()
        };
        let fence = UpstreamStopFence {
            runtime: self.clone(),
            root: root.into(),
        };
        for reference in targets {
            self.await_upstream_stop(&reference)?;
        }
        Ok(fence)
    }
}

impl LocalRuntime {
    /// Resource owners can withdraw only results they supplied. This private
    /// provider operation does not grant an actor authority over other turns.
    pub fn revoke_upstream_result(
        &self,
        owner: &str,
        invocation: &str,
    ) -> CwrResult<LocalStopOutcome> {
        let (root_ref, actor_ref) = {
            let state = self.lock()?;
            let route = retained_upstream_route(&state, invocation)
                .ok_or_else(|| invalid_error("unknown upstream result"))?;
            if !state.upstream.owns_route(owner, &route) {
                return invalid("provider cannot revoke another provider's result");
            }
            let mut root = state
                .admission
                .invocation_record(invocation)
                .ok_or_else(|| invalid_error("unknown upstream invocation"))?;
            while let Some(parent) = &root.parent_invocation_ref {
                root = state
                    .admission
                    .invocation_record(parent)
                    .ok_or_else(|| invalid_error("upstream ancestry is unavailable"))?;
            }
            (root.invocation_ref.clone(), route.coworker.actor_ref)
        };
        self.stop_invocation_as(
            &root_ref,
            &actor_ref,
            Some("Connected resource access ended.".into()),
        )
    }
}

impl LocalRuntime {
    pub fn protect_upstream_result(&self, owner: &str, invocation: &str) -> CwrResult<()> {
        let mut state = self.lock()?;
        let route = retained_upstream_route(&state, invocation)
            .ok_or_else(|| invalid_error("unknown upstream result"))?;
        if !state.upstream.owns_route(owner, &route) {
            return invalid("provider cannot protect another provider's result");
        }
        let record = state.admission.invocation_record(invocation)
            .ok_or_else(|| invalid_error("upstream result is unavailable"))?;
        require_native_gateway_result_current(&state, record)?;
        let mut root = state
            .admission
            .invocation_record(invocation)
            .ok_or_else(|| invalid_error("unknown upstream invocation"))?;
        while let Some(parent) = &root.parent_invocation_ref {
            root = state
                .admission
                .invocation_record(parent)
                .ok_or_else(|| invalid_error("upstream ancestry is unavailable"))?;
        }
        if root.state.is_terminal() {
            return invalid("consuming turn already ended");
        }
        let root_ref = root.invocation_ref.clone();
        state.admission.retain_protected_result(&root_ref, invocation)?;
        state
            .protected_upstream_results
            .entry(root_ref)
            .or_default()
            .insert(invocation.into(), route);
        Ok(())
    }

    pub fn validate_upstream_result(
        &self,
        owner: &str,
        validation: &str,
        allowed: bool,
    ) -> CwrResult<()> {
        self.validate_upstream_result_with_authority(owner,validation,allowed,Vec::new())
    }
    pub fn validate_upstream_result_with_authority(&self,owner:&str,validation:&str,allowed:bool,authority_guards:Vec<cwpdb_store::conversation::AuthorityGuard>)->CwrResult<()> {
        if authority_guards.len()>32 || !allowed && !authority_guards.is_empty() {return invalid("invalid host result authority guards");}
        for guard in &authority_guards {CwrRef::parse(&guard.reference)?;if guard.expected_revision.is_empty() || guard.expected_revision.len()>256 {return invalid("invalid host result authority revision");}}
        let mut state = self.lock()?;
        let pending = state
            .upstream_result_validations
            .get_mut(validation)
            .ok_or_else(|| invalid_error("unknown result validation"))?;
        if pending.0 != owner || pending.1.is_some() {
            return invalid("provider cannot answer this result validation");
        }
        pending.1 = Some(NativeResultValidation{allowed,authority_guards});
        Ok(())
    }

    // Before publishing a consuming root's answer, ask every resource owner for
    // a fresh check. A lost host/control connection fails closed. Ordinary actor
    // invocation and stop authority are unchanged.
    fn check_upstream_result_delivery(&self, record: &ActionInvocationRecord) -> CwrResult<bool> {
        // Result-purpose authority is for the successful child consumed by a
        // parent. Before a child is retained it is still running, so asking
        // its provider for completed-result authority would stop valid work.
        if record.parent_invocation_ref.is_some() {
            return Ok(true);
        }
        let guards = {
            let state=self.lock()?;
            let mut selected=record;
            while let Some(parent)=&selected.parent_invocation_ref {selected=state.admission.invocation_record(parent).ok_or("protected result ancestry unavailable")?;}
            let root_ref=&selected.invocation_ref;
            let mut guards=state.protected_upstream_results.get(root_ref).cloned().unwrap_or_default();
            for child in state.admission.protected_results(root_ref) {
                let route=retained_upstream_route(&state,&child).ok_or("retained protected result route unavailable")?;
                guards.entry(child).or_insert(route);
            }
            guards
        };
        let mut authority=Vec::new();
        for (invocation, route) in guards {
            let validation = format!("result-check-{}", Uuid::new_v4().simple());
            let writer = {
                let state = self.lock()?;
                let current = state.admission.invocation_record(&invocation)
                    .is_some_and(|child| require_native_gateway_result_current(&state, child).is_ok());
                state.upstream_providers.get(&route.provider_session_ref)
                    .filter(|_| current)
                    .map(|provider| (provider.owner.clone(), Arc::clone(&provider.writer)))
            };
            let delivered = if let Some((owner, writer)) = writer {
                self.lock()?
                    .upstream_result_validations
                    .insert(validation.clone(), (owner, None));
                writer(UpstreamEvent::ValidateResult {
                    invocation_ref: invocation.clone(),
                    validation_ref: validation.clone(),
                })
                .is_ok()
            } else {
                false
            };
            let deadline = Instant::now() + Duration::from_secs(12);
            let allowed = if delivered {
                loop {
                    let state = self.lock()?;
                    if state
                        .admission
                        .invocation_record(&record.invocation_ref)
                        .is_none_or(|r| r.state.is_terminal())
                    {
                        break None;
                    }
                    if state.admission.invocation_record(&invocation)
                        .is_none_or(|child| require_native_gateway_result_current(&state, child).is_err()) {
                        break None;
                    }
                    if let Some((_, Some(allowed))) =
                        state.upstream_result_validations.get(&validation)
                    {
                        break Some(allowed.clone());
                    }
                    if Instant::now() >= deadline
                        || !state
                            .upstream_providers
                            .contains_key(&route.provider_session_ref)
                    {
                        break None;
                    }
                    drop(state);
                    thread::sleep(Duration::from_millis(10));
                }
            } else {
                None
            };
            self.lock()?.upstream_result_validations.remove(&validation);
            if !allowed.as_ref().is_some_and(|value|value.allowed) {
                self.stop_invocation_as_without_upstream_wait(
                    &record.invocation_ref,
                    &route.coworker.actor_ref,
                    Some("Connected resource access ended before reply delivery.".into()),
                )?;
                self.lock()?
                    .protected_upstream_results
                    .remove(&record.invocation_ref);
                return Ok(false);
            }
            authority.extend(allowed.unwrap().authority_guards);
        }
        self.lock()?.protected_publication_authority.insert(record.invocation_ref.clone(),authority);
        // Keep guards until the publication lock performs its final native
        // route check. A graph reload may occur after a host replies allowed.
        Ok(true)
    }
}
