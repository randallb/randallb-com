impl LocalRuntime {
    pub fn stop_invocation(
        &self,
        invocation_ref: &str,
        reason: Option<String>,
    ) -> CwrResult<LocalStopOutcome> {
        let requested_by_actor_ref = self.catalog()?.acting_actor.actor_ref;
        self.stop_invocation_as(invocation_ref, &requested_by_actor_ref, reason)
    }

    pub fn stop_conversation_as(
        &self,
        conversation_ref: &str,
        requested_by_actor_ref: &str,
        reason: Option<String>,
    ) -> CwrResult<LocalStopOutcome> {
        let invocation_ref = {
            let state = self.lock()?;
            let selection = state
                .admission
                .select_invocation_for_stop(Some(conversation_ref), None)?;
            let invocation_ref = match selection {
                InvocationStopSelection::Selected(invocation_ref) => invocation_ref,
                InvocationStopSelection::None => return Ok(LocalStopOutcome::Unknown),
                InvocationStopSelection::Ambiguous(invocation_refs) => {
                    return invalid(format!(
                        "more than one invocation can be stopped: {}",
                        invocation_refs.join(", ")
                    ))
                }
            };
            if !state
                .admission
                .is_actor_authorized_to_stop_invocation(&invocation_ref, requested_by_actor_ref)?
            {
                return invalid(format!(
                    "actor {requested_by_actor_ref} is not authorized to stop {invocation_ref}"
                ));
            }
            invocation_ref
        };
        self.stop_invocation_as(&invocation_ref, requested_by_actor_ref, reason)
    }

    pub fn stop_invocation_as(
        &self,
        invocation_ref: &str,
        requested_by_actor_ref: &str,
        reason: Option<String>,
    ) -> CwrResult<LocalStopOutcome> {
        let request = InvocationStopRequest {
            requested_at_unix_ms: current_unix_ms(),
            requested_by_actor_ref: Some(requested_by_actor_ref.into()),
            request_id: Some(format!("local-stop-{}", Uuid::new_v4().simple())),
            reason,
        };
        self.apply_stop(|_| Ok(Some((invocation_ref.into(), request))))
    }

    fn stop_invocation_as_without_upstream_wait(
        &self,
        invocation_ref: &str,
        requested_by_actor_ref: &str,
        reason: Option<String>,
    ) -> CwrResult<LocalStopOutcome> {
        let request = InvocationStopRequest {
            requested_at_unix_ms: current_unix_ms(),
            requested_by_actor_ref: Some(requested_by_actor_ref.into()),
            request_id: Some(format!("local-deadline-stop-{}", Uuid::new_v4().simple())),
            reason,
        };
        self.apply_stop_with_delivery(|_| Ok(Some((invocation_ref.into(), request))), true)
    }

    fn apply_stop(
        &self,
        select: impl FnOnce(&LocalRuntimeState) -> CwrResult<Option<(String, InvocationStopRequest)>>,
    ) -> CwrResult<LocalStopOutcome> {
        self.apply_stop_with_delivery(select, false)
    }

    fn apply_stop_with_delivery(
        &self,
        select: impl FnOnce(&LocalRuntimeState) -> CwrResult<Option<(String, InvocationStopRequest)>>,
        best_effort: bool,
    ) -> CwrResult<LocalStopOutcome> {
        let selected = {
            let state = self.lock()?;
            select(&state)?
        };
        let Some((invocation_ref, request)) = selected else {
            return Ok(LocalStopOutcome::Unknown);
        };
        let prepared = {
            let mut state = self.lock()?;
            self.prepare_invocation_stop_locked(&mut state, &invocation_ref, request)?
        };
        self.deliver_invocation_stop(prepared, best_effort)
    }

    // Callers changing authority can commit the stop while still holding the
    // same lock that fences admission and publication. Provider IO follows later.
    fn prepare_invocation_stop_locked(
        &self,
        state: &mut LocalRuntimeState,
        invocation_ref: &str,
        request: InvocationStopRequest,
    ) -> CwrResult<(PublishInvocationStopOutcome, Vec<ProcessAdapterStopControl>)> {
        let record = match state.admission.invocation_record(invocation_ref).cloned() {
            Some(record) => record,
            None => return Ok((PublishInvocationStopOutcome::Unknown, Vec::new())),
        };
        let text = request
            .reason
            .as_deref()
            .map(|reason| format!("Stop requested: {reason}"))
            .unwrap_or_else(|| "Stop requested.".into());
        let outcome = if matches!(record.origin, InvocationOrigin::Ingress { .. }) {
            state
                .admission
                .publish_root_stop(invocation_ref, request, text)?
        } else {
            state
                .admission
                .publish_child_action_stop(invocation_ref, request, text)?
        };
        let stopped = match &outcome {
            PublishInvocationStopOutcome::Accepted {
                stopped_invocation_refs,
                ..
            } => stopped_invocation_refs.as_slice(),
            _ => &[],
        };
        if let PublishInvocationStopOutcome::Accepted {
            woken_invocation_refs,
            ..
        } = &outcome
        {
            state
                .pending_invocation_refs
                .extend(woken_invocation_refs.iter().cloned());
        }
        let controls = stopped
            .iter()
            .filter_map(|invocation_ref| state.active.get(invocation_ref).cloned())
            .collect::<Vec<_>>();
        for reference in stopped {
            state.protected_upstream_results.remove(reference);
        }
        self.sync_archive_reservations(state)?;
        state.admission.clear_provider_questions(false)?;
        Ok((outcome, controls))
    }

    fn deliver_invocation_stop(
        &self,
        (outcome, controls): (PublishInvocationStopOutcome, Vec<ProcessAdapterStopControl>),
        best_effort: bool,
    ) -> CwrResult<LocalStopOutcome> {
        for control in controls {
            control.stop();
        }
        if let PublishInvocationStopOutcome::Accepted {
            stopped_invocation_refs,
            ..
        } = &outcome
        {
            for reference in stopped_invocation_refs {
                if best_effort {
                    self.request_upstream_stop_without_wait(reference.clone())?;
                } else {
                    self.request_upstream_stop(reference)?;
                }
            }
        }
        Ok(match outcome {
            PublishInvocationStopOutcome::Accepted {
                response,
                stopped_invocation_refs,
                woken_invocation_refs,
                ..
            } => LocalStopOutcome::Accepted {
                response,
                stopped_invocation_refs,
                woken_invocation_refs,
            },
            PublishInvocationStopOutcome::TooLate { response, .. } => {
                LocalStopOutcome::TooLate { response }
            }
            PublishInvocationStopOutcome::Unknown => LocalStopOutcome::Unknown,
        })
    }

    /// Resolves and applies a stop from a host-native `@cwr` control surface.
    ///
    /// Explicit selectors are workspace-wide. A bare stop is scoped to the
    /// supplied conversation and refuses to guess when more than one target is
    /// eligible. Selection and authority are checked before the existing local
    /// stop transition persists lifecycle state and signals active adapters.
    pub fn stop_intrinsic_invocation(
        &self,
        conversation_ref: Option<&str>,
        invocation_selector: Option<&str>,
        reason: Option<String>,
    ) -> CwrResult<IntrinsicCwrResponse> {
        let requested_by_actor_ref = self.catalog()?.acting_actor.actor_ref;
        self.stop_intrinsic_invocation_as(
            conversation_ref,
            invocation_selector,
            &requested_by_actor_ref,
            reason,
        )
    }

    pub fn stop_intrinsic_invocation_as(
        &self,
        conversation_ref: Option<&str>,
        invocation_selector: Option<&str>,
        requested_by_actor_ref: &str,
        reason: Option<String>,
    ) -> CwrResult<IntrinsicCwrResponse> {
        self.stop_intrinsic_invocation_with_request(
            conversation_ref,
            invocation_selector,
            requested_by_actor_ref,
            reason,
            None,
        )
    }

    fn stop_intrinsic_invocation_with_request(
        &self,
        conversation_ref: Option<&str>,
        invocation_selector: Option<&str>,
        requested_by_actor_ref: &str,
        reason: Option<String>,
        control_ref: Option<&str>,
    ) -> CwrResult<IntrinsicCwrResponse> {
        let requested_ref = invocation_selector.map(str::to_owned);
        if invocation_selector.is_none() && conversation_ref.is_none() {
            return Ok(intrinsic_cwr_invocation_stop_failed(
                None,
                "provide an invocation ref or resume a conversation with retained work",
            ));
        }

        let (invocation_ref, requested_at_unix_ms) = {
            let state = self.lock()?;
            let selection_conversation_ref = if invocation_selector.is_none() {
                conversation_ref
            } else {
                None
            };
            let selection = match state
                .admission
                .select_invocation_for_stop(selection_conversation_ref, invocation_selector)
            {
                Ok(selection) => selection,
                Err(error) => {
                    return Ok(intrinsic_cwr_invocation_stop_failed(
                        requested_ref,
                        error.to_string(),
                    ))
                }
            };
            let invocation_ref = match selection {
                InvocationStopSelection::Selected(invocation_ref) => invocation_ref,
                InvocationStopSelection::None if invocation_selector.is_some() => {
                    return Ok(intrinsic_cwr_invocation_stop_failed(
                        requested_ref,
                        "invocation was not found",
                    ))
                }
                InvocationStopSelection::None => {
                    return Ok(intrinsic_cwr_invocation_stop_failed(
                        None,
                        "this conversation has no invocation that can be stopped",
                    ))
                }
                InvocationStopSelection::Ambiguous(invocation_refs) => {
                    return Ok(intrinsic_cwr_invocation_stop_failed(
                        requested_ref,
                        format!(
                            "more than one invocation matches; use an exact ref: {}",
                            invocation_refs.join(", ")
                        ),
                    ))
                }
            };
            if !state
                .admission
                .is_actor_authorized_to_stop_invocation(&invocation_ref, requested_by_actor_ref)?
            {
                return Ok(intrinsic_cwr_invocation_stop_failed(
                    Some(invocation_ref),
                    "the acting actor is not authorized to stop that invocation",
                ));
            }
            let requested_at = state.admission.invocation_record(&invocation_ref)
                .filter(|target| control_ref.is_some()
                    && target.stop_request_id.as_deref() == control_ref
                    && target.stop_requested_by_actor_ref.as_deref() == Some(requested_by_actor_ref)
                    && target.stop_reason == reason)
                .and_then(|target| target.stop_requested_at_unix_ms)
                .unwrap_or_else(current_unix_ms);
            (invocation_ref, requested_at)
        };

        let request = InvocationStopRequest {
            requested_at_unix_ms,
            requested_by_actor_ref: Some(requested_by_actor_ref.into()),
            request_id: Some(
                control_ref
                    .map(str::to_owned)
                    .unwrap_or_else(|| format!("local-stop-{}", Uuid::new_v4().simple())),
            ),
            reason,
        };
        Ok(
            match self.apply_stop(|_| Ok(Some((invocation_ref.clone(), request))))? {
                LocalStopOutcome::Accepted {
                    stopped_invocation_refs,
                    ..
                } => {
                    intrinsic_cwr_invocation_stop_succeeded(invocation_ref, stopped_invocation_refs)
                }
                LocalStopOutcome::TooLate { .. } => {
                    intrinsic_cwr_invocation_stop_too_late(invocation_ref)
                }
                LocalStopOutcome::Unknown => intrinsic_cwr_invocation_stop_failed(
                    Some(invocation_ref),
                    "invocation was not found",
                ),
            },
        )
    }

    /// Stops the highest live ancestor that still owns the invocation currently
    /// visible in a local host. Attached edges are crossed so a waiting parent
    /// cannot resume after cancellation; detached and terminal boundaries are
    /// not crossed. Explicitly selected child stops remain exact.
    pub fn stop_active_intrinsic_invocation(
        &self,
        active_invocation_ref: &str,
        reason: Option<String>,
    ) -> CwrResult<IntrinsicCwrResponse> {
        let stop_invocation_ref = {
            let state = self.lock()?;
            let mut current_ref = active_invocation_ref.to_owned();
            let mut visited = BTreeSet::new();
            loop {
                if !visited.insert(current_ref.clone()) {
                    return invalid(format!("invocation parent cycle contains {current_ref}"));
                }
                let Some(record) = state.admission.invocation_record(&current_ref) else {
                    return Ok(intrinsic_cwr_invocation_stop_failed(
                        Some(active_invocation_ref.into()),
                        "invocation was not found",
                    ));
                };
                let parent_ref = match &record.origin {
                    InvocationOrigin::Ingress { .. } => break current_ref,
                    InvocationOrigin::HostCall { caller_invocation_ref, .. } => caller_invocation_ref.clone(),
                };
                let parent = state
                    .admission
                    .invocation_record(&parent_ref)
                    .ok_or_else(|| {
                        invalid_error(format!(
                            "invocation {current_ref} has unknown parent {parent_ref}"
                        ))
                    })?;
                if parent.state.is_terminal() {
                    break current_ref;
                }
                current_ref = parent_ref;
            }
        };
        let response = self.stop_intrinsic_invocation(None, Some(&stop_invocation_ref), reason)?;
        if let IntrinsicCwrResponse::InvocationStopped {
            stopped_invocation_refs,
            ..
        } = &response
        {
            let active_is_terminal = self
                .lock()?
                .admission
                .invocation_record(active_invocation_ref)
                .is_some_and(|record| record.state.is_terminal());
            if !active_is_terminal
                && !stopped_invocation_refs
                    .iter()
                    .any(|stopped_ref| stopped_ref == active_invocation_ref)
            {
                return invalid(format!(
                    "active invocation {active_invocation_ref} was not stopped with {stop_invocation_ref}"
                ));
            }
        }
        Ok(response)
    }
}
