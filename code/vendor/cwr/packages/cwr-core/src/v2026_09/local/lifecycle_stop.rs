impl LocalRuntime {
    fn handle_stop_invocation(
        &self,
        call: &AdapterCall,
        caller: &ActionInvocationRecord,
        queue: &Rc<RefCell<VecDeque<String>>>,
    ) -> CwrResult<AdapterHostCallOutcome> {
        let input: StopInvocationInput = parse_input("stop-invocation", &call.input)?;
        let (result, controls, woken_invocation_refs) = {
            let mut state = self.lock()?;
            if call.invocation_ref != caller.invocation_ref {
                return invalid("host call belongs to another caller");
            }
            let retained_call_time = match retained_host_call_time(&state.admission, call)? {
                Some(occurred_at_unix_ms) => occurred_at_unix_ms,
                None => {
                    require_active_caller(&state.admission, call)?;
                    ensure_host_call(&mut state.admission, call)?
                }
            };
            let target = state
                .admission
                .invocation_record(&input.invocation_ref)
                .cloned()
                .ok_or_else(|| {
                    invalid_error(format!("unknown stop target {}", input.invocation_ref))
                })?;
            if target.invocation_ref != caller.invocation_ref
                && target.parent_invocation_ref.as_deref() != Some(caller.invocation_ref.as_str())
            {
                return invalid("stop target must be the caller or one direct child");
            }

            if retained_stop_matches(
                &target,
                &caller.target_actor_ref,
                input.reason.as_deref(),
                &call.call_id,
                retained_call_time,
            ) {
                let stopped_invocation_refs = retained_stopped_subtree(
                    &state.admission,
                    &target,
                    &caller.target_actor_ref,
                    input.reason.as_deref(),
                    &call.call_id,
                );
                let woken_invocation_refs =
                    retained_woken_waiters(&state.admission, &stopped_invocation_refs);
                let controls = stopped_invocation_refs
                    .iter()
                    .filter_map(|invocation_ref| state.active.get(invocation_ref).cloned())
                    .collect();
                (
                    StopInvocationResult::Stopped {
                        invocation_ref: target.invocation_ref,
                    },
                    controls,
                    woken_invocation_refs,
                )
            } else {
                require_active_caller(&state.admission, call)?;
                let request = InvocationStopRequest {
                    requested_at_unix_ms: retained_call_time,
                    requested_by_actor_ref: Some(caller.target_actor_ref.clone()),
                    request_id: Some(call.call_id.clone()),
                    reason: input.reason.clone(),
                };
                let text = input
                    .reason
                    .as_deref()
                    .map(|reason| format!("Stopped: {reason}"))
                    .unwrap_or_else(|| "Stopped that action.".into());
                let publication = if matches!(target.origin, InvocationOrigin::Ingress { .. }) {
                    state
                        .admission
                        .publish_root_stop(&target.invocation_ref, request, text)?
                } else {
                    state.admission.publish_child_action_stop(
                        &target.invocation_ref,
                        request,
                        text,
                    )?
                };
                match publication {
                    PublishInvocationStopOutcome::Accepted {
                        stopped_invocation_refs,
                        woken_invocation_refs,
                        ..
                    } => {
                        let controls = stopped_invocation_refs
                            .iter()
                            .filter_map(|invocation_ref| state.active.get(invocation_ref).cloned())
                            .collect();
                        (
                            StopInvocationResult::Stopped {
                                invocation_ref: input.invocation_ref.clone(),
                            },
                            controls,
                            woken_invocation_refs,
                        )
                    }
                    PublishInvocationStopOutcome::TooLate { .. } => (
                        StopInvocationResult::TooLate {
                            invocation_ref: input.invocation_ref.clone(),
                        },
                        Vec::new(),
                        Vec::new(),
                    ),
                    PublishInvocationStopOutcome::Unknown => {
                        return invalid("unknown stop target invocation");
                    }
                }
            }
        };
        for control in controls {
            control.stop();
        }
        if matches!(result, StopInvocationResult::Stopped { .. }) {
            let refs = {
                let state = self.lock()?;
                state
                    .upstream_work
                    .keys()
                    .filter(|reference| descends_from(&state, reference, &input.invocation_ref))
                    .cloned()
                    .collect::<Vec<_>>()
            };
            for reference in refs {
                self.request_upstream_stop_without_wait(reference)?;
            }
        }
        let mut queue = queue.borrow_mut();
        for invocation_ref in woken_invocation_refs.into_iter().rev() {
            queue.push_front(invocation_ref);
        }
        drop(queue);
        Ok(AdapterHostCallOutcome::Returned(serde_json::to_value(
            result,
        )?))
    }

}
