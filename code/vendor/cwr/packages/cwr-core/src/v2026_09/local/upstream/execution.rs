impl LocalRuntime {
    fn execute_upstream_work(
        &self,
        record: &ActionInvocationRecord,
        route: UpstreamRoute,
        queue: &Rc<RefCell<VecDeque<String>>>,
        observer: &mut dyn FnMut(LocalRuntimeEvent),
    ) -> CwrResult<()> {
        let expected_integration = format!(
            "{}/routes/{}",
            route.provider_session_ref,
            route.coworker.handle.trim_start_matches('@')
        );
        if record.target_actor_ref != route.coworker.actor_ref
            || record.actor_integration_ref.as_deref() != Some(expected_integration.as_str())
        {
            return self.publish_claim_failure(
                record,
                "retained upstream route does not match the invocation".into(),
                queue,
                observer,
            );
        }
        let (writer, message, root_invocation_ref) = {
            let mut state = self.lock()?;
            if state
                .admission
                .invocation_record(&record.invocation_ref)
                .is_some_and(|record| record.state.is_terminal())
                || require_no_stop_fence(&state, &record.invocation_ref).is_err()
            {
                state.active.remove(&record.invocation_ref);
                if state
                    .admission
                    .invocation_record(&record.invocation_ref)
                    .is_some_and(|record| record.state == ActionInvocationState::Stopping)
                {
                    state.settle_stopped_invocation(&record.invocation_ref, current_unix_ms())?;
                } else {
                    // No provider was dispatched. Release the claim so an archive
                    // transaction does not retain a nonexistent execution owner.
                    state
                        .admission
                        .recover_interrupted_invocation(&record.invocation_ref)?;
                }
                return Ok(());
            }
            let writer = state
                .upstream_providers
                .get(&route.provider_session_ref)
                .filter(|provider| state.upstream.owns_route(&provider.owner, &route))
                .map(|provider| Arc::clone(&provider.writer));
            let source_ref = state
                .admission
                .execution_conversation(&record.invocation_ref)
                .and_then(|conversation| {
                    conversation
                        .activity
                        .iter()
                        .find_map(|activity| match &activity.event {
                            ConversationActivityEvent::TurnStarted {
                                invocation_ref,
                                source_message_ref,
                                ..
                            } if invocation_ref == &record.invocation_ref => {
                                Some(source_message_ref.as_str())
                            }
                            _ => None,
                        })
                });
            let message = record
                .conversation_ref
                .as_deref()
                .and_then(|reference| state.admission.conversation(reference))
                .and_then(|conversation| {
                    conversation
                        .messages
                        .iter()
                        .find(|message| Some(message.message_ref.as_str()) == source_ref)
                })
                .cloned();
            state.upstream_work.insert(
                record.invocation_ref.clone(),
                PendingUpstreamWork {
                    route: route.clone(),
                    terminal: None,
                    disconnected: writer.is_none(),
                    publication_error: None,
                    stop_requested: false,
                    stop_acknowledged: false,
                    stop_delivery: UpstreamStopDelivery::Idle,
                },
            );
            let mut root = record;
            while let Some(parent) = &root.parent_invocation_ref {
                root = state.admission.invocation_record(parent)
                    .ok_or_else(|| invalid_error("upstream ancestry is unavailable"))?;
            }
            (writer, message, root.invocation_ref.clone())
        };
        if let Some(writer) = writer {
            let event = UpstreamEvent::WorkRequested {
                work: Box::new(UpstreamWork {
                    invocation_ref: record.invocation_ref.clone(),
                    root_invocation_ref,
                    conversation_ref: record
                        .conversation_ref
                        .clone()
                        .ok_or_else(|| invalid_error("upstream work requires a conversation"))?,
                    requester_actor_ref: record.requester_actor_ref.clone(),
                    route,
                    action_name: record.action_name.clone(),
                    input: record.input.clone(),
                    addressed_message: message,
                }),
            };
            if writer(event).is_err() {
                self.lock()?
                    .upstream_work
                    .get_mut(&record.invocation_ref)
                    .unwrap()
                    .disconnected = true;
            }
        }
        loop {
            let canceled = {
                let state = self.lock()?;
                state
                    .admission
                    .invocation_record(&record.invocation_ref)
                    .is_some_and(|record| {
                        matches!(
                            record.state,
                            ActionInvocationState::Stopping | ActionInvocationState::Stopped
                        )
                    })
            };
            if canceled {
                let settled = {
                    let state = self.lock()?;
                    state
                        .upstream_work
                        .get(&record.invocation_ref)
                        .map(|pending| pending.stop_acknowledged || pending.terminal.is_some())
                        .unwrap_or(true)
                };
                if settled {
                    self.settle_stopped_execution(&record.invocation_ref)?;
                }
                let mut state = self.lock()?;
                state.active.remove(&record.invocation_ref);
                // An acknowledgement can arrive after the settlement read above.
                // Read it while removing the active worker so either this worker
                // or the acknowledging provider removes the pending entry.
                if state
                    .upstream_work
                    .get(&record.invocation_ref)
                    .is_some_and(|pending| {
                        pending.stop_acknowledged
                            || pending.terminal.is_some()
                            || pending.disconnected
                    })
                {
                    state.upstream_work.remove(&record.invocation_ref);
                }
                return Ok(());
            }
            let terminal = {
                let mut state = self.lock()?;
                let pending = state.upstream_work.get_mut(&record.invocation_ref).unwrap();
                if let Some(terminal) = pending.terminal.clone() {
                    Some(Ok(terminal))
                } else if pending.disconnected {
                    Some(Err("upstream provider disconnected".to_owned()))
                } else {
                    None
                }
            };
            if let Some(terminal) = terminal {
                self.lock()?.active.remove(&record.invocation_ref);
                let publication = match terminal {
                    Ok(terminal) => self.publish_terminal(record, terminal, queue, observer),
                    Err(error) => self.publish_execution_failure(record, error, queue, observer),
                };
                if let Err(error) = &publication {
                    if let Some(pending) =
                        self.lock()?.upstream_work.get_mut(&record.invocation_ref)
                    {
                        pending.publication_error = Some(error.to_string());
                    }
                }
                if publication.is_ok() {
                    self.lock()?.upstream_work.remove(&record.invocation_ref);
                }
                return publication;
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
}
impl LocalRuntime {
    fn fail_unavailable_upstream(&self) -> CwrResult<()> {
        let refs = {
            let state = self.lock()?;
            let refs = state
                .admission
                .invocation_records()
                .filter(|record| record.is_runnable())
                .filter_map(|record| {
                    let route = retained_upstream_route(&state, &record.invocation_ref)?;
                    (!state
                        .upstream_providers
                        .contains_key(&route.provider_session_ref))
                    .then(|| record.invocation_ref.clone())
                })
                .collect::<BTreeSet<_>>();
            order_runnable_refs(&state.admission, refs)
        };
        {
            let mut state = self.lock()?;
            for reference in &refs {
                if !state.pending_invocation_refs.contains(reference) {
                    state.pending_invocation_refs.push(reference.clone());
                }
            }
        }
        for (execution_group_ref, references) in self.group_refs_by_execution(&refs)? {
            for reference in references {
                let Some(_execution) = self.execution.try_acquire(&execution_group_ref, || {
                    self.invocation_is_next_in_group(&reference)
                })? else { break; };
                // Settlement owns the same conversation permit as normal execution.
                self.execute_requested_ref(
                    &reference,
                    &AdapterModelOutputMailbox::discard(),
                    &mut |_| {},
                )?;
                self.lock()?
                    .pending_invocation_refs
                    .retain(|pending| pending != &reference);
            }
        }
        Ok(())
    }
}

fn retained_upstream_route(state: &LocalRuntimeState, reference: &str) -> Option<UpstreamRoute> {
    state
        .admission
        .execution_conversation(reference)?
        .activity
        .iter()
        .find_map(|activity| match &activity.event {
            ConversationActivityEvent::TurnStarted {
                invocation: Some(snapshot),
                ..
            } => serde_json::from_value(
                snapshot
                    .get("adapter_params")?
                    .get("upstream_route")?
                    .clone(),
            )
            .ok(),
            _ => None,
        })
}
