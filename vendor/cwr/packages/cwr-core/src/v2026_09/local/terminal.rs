impl LocalRuntime {
    fn publish_terminal(
        &self,
        record: &ActionInvocationRecord,
        terminal: AdapterTerminal,
        queue: &Rc<RefCell<VecDeque<String>>>,
        observer: &mut dyn FnMut(LocalRuntimeEvent),
    ) -> CwrResult<()> {
        self.publish_terminal_with_rename(record, terminal, None, queue, observer)
    }

    fn publish_terminal_with_rename(
        &self,
        record: &ActionInvocationRecord,
        terminal: AdapterTerminal,
        rename: Option<ConversationRenameMutation>,
        queue: &Rc<RefCell<VecDeque<String>>>,
        observer: &mut dyn FnMut(LocalRuntimeEvent),
    ) -> CwrResult<()> {
        self.publish_terminal_with_mutation(
            record,
            terminal,
            rename.map(ConversationIntrinsicMutation::Rename),
            queue,
            observer,
        )
    }

    fn publish_terminal_with_mutation(
        &self,
        record: &ActionInvocationRecord,
        terminal: AdapterTerminal,
        mutation: Option<ConversationIntrinsicMutation>,
        queue: &Rc<RefCell<VecDeque<String>>>,
        observer: &mut dyn FnMut(LocalRuntimeEvent),
    ) -> CwrResult<()> {
        if !self.check_upstream_result_delivery(record)? {
            self.settle_stopped_execution(&record.invocation_ref)?;
            return Ok(());
        }
        let invocation_terminal = match adapter_terminal(&terminal) {
            Ok(terminal) => terminal,
            Err(error) => {
                return self.publish_execution_failure(record, error.to_string(), queue, observer);
            }
        };
        let text = terminal.summary.clone();
        self.publish_invocation_terminal(
            record,
            invocation_terminal,
            text,
            mutation,
            queue,
            observer,
        )
    }

    fn publish_execution_failure(
        &self,
        record: &ActionInvocationRecord,
        error: String,
        queue: &Rc<RefCell<VecDeque<String>>>,
        observer: &mut dyn FnMut(LocalRuntimeEvent),
    ) -> CwrResult<()> {
        if self.settle_stopped_execution(&record.invocation_ref)? {
            return Ok(());
        }
        self.publish_invocation_terminal(
            record,
            InvocationTerminal {
                status: ActionResponseStatus::FAILED,
                summary: "I couldn't complete that request.".into(),
                result: None,
                error: Some(
                    ActionResponseDetail::new("adapter_execution_failed").with_message(error),
                ),
            },
            "I couldn't complete that request.".into(),
            None,
            queue,
            observer,
        )
    }

    fn publish_provider_unavailable(
        &self,
        record: &ActionInvocationRecord,
        readiness: AdapterReadiness,
        queue: &Rc<RefCell<VecDeque<String>>>,
        observer: &mut dyn FnMut(LocalRuntimeEvent),
    ) -> CwrResult<()> {
        if self.settle_stopped_execution(&record.invocation_ref)? {
            return Ok(());
        }
        let message = readiness.message.clone();
        self.publish_invocation_terminal(
            record,
            InvocationTerminal {
                status: ActionResponseStatus::PROVIDER_UNAVAILABLE,
                summary: message.clone(),
                result: None,
                error: Some(
                    ActionResponseDetail::new("provider_unavailable")
                        .with_message(message.clone())
                        .with_details(serde_json::to_value(readiness)?),
                ),
            },
            message,
            None,
            queue,
            observer,
        )
    }

    fn publish_invocation_terminal(
        &self,
        record: &ActionInvocationRecord,
        terminal: InvocationTerminal,
        text: String,
        mutation: Option<ConversationIntrinsicMutation>,
        queue: &Rc<RefCell<VecDeque<String>>>,
        observer: &mut dyn FnMut(LocalRuntimeEvent),
    ) -> CwrResult<()> {
        let archive_stops = self.prepare_archive_terminal_stops(record, &mutation)?;
        let publication = (|| -> CwrResult<_> {
            let mut state = self.lock()?;
            // Failure summaries and error details can contain resource data too.
            // Check every terminal under the publication lock, not only success.
            let (terminal, text) = if require_native_gateway_publication_current(&state, record).is_err() {
                let summary = "Connected resource access changed before result publication.".to_owned();
                (InvocationTerminal {
                    status: ActionResponseStatus::FAILED, summary: summary.clone(), result: None,
                    error: Some(ActionResponseDetail::new("gateway_access_changed")),
                }, summary)
            } else { (terminal, text) };
            if let Some(ConversationIntrinsicMutation::Archive(archive)) = &mutation {
                if state.admission.invocation_record(&record.invocation_ref)
                    .is_some_and(|record| !record.state.is_terminal()) {
                    local_catalog(&state.graph, &archive.caller_actor_ref)?;
                }
            }
            let before = if archive_stops.is_some() { stopped_archive_snapshot(&state) } else { BTreeSet::new() };
            let authority=state.protected_publication_authority.remove(&record.invocation_ref).unwrap_or_default();
            let guarded=!authority.is_empty();
            let attempted=state.admission.with_publication_authority(authority,|admission|Ok(match &record.origin {
                InvocationOrigin::Ingress { .. }
                    if record.action_name == RESPOND_TO_MESSAGE_ACTION_NAME =>
                {
                    if terminal.status.is_completed() {
                        let result = terminal
                            .result
                            .ok_or_else(|| invalid_error("completed terminal has no result"))?;
                        match mutation.clone() {
                            Some(ConversationIntrinsicMutation::Rename(rename)) => {
                                admission.publish_root_message_completed_with_rename(
                                    &record.invocation_ref,
                                    terminal.status,
                                    result,
                                    terminal.summary,
                                    current_unix_ms(),
                                    rename,
                                )?
                            }
                            Some(ConversationIntrinsicMutation::Archive(archive)) => admission.publish_root_message_completed_with_archive(
                                    &record.invocation_ref,
                                    terminal.status,
                                    result,
                                    terminal.summary,
                                    current_unix_ms(),
                                    archive,
                                )?,
                            None => admission.publish_root_message_completed(
                                &record.invocation_ref,
                                terminal.status,
                                result,
                                terminal.summary,
                                current_unix_ms(),
                            )?,
                        }
                    } else {
                        admission.publish_root_failure(
                            &record.invocation_ref,
                            terminal,
                            text,
                            current_unix_ms(),
                        )?
                    }
                }
                InvocationOrigin::Ingress { .. } => {
                    if terminal.status.is_completed() {
                        let result = terminal
                            .result
                            .ok_or_else(|| invalid_error("completed terminal has no result"))?;
                        match mutation.clone() {
                            Some(ConversationIntrinsicMutation::Rename(rename)) => {
                                admission.publish_root_action_completed_with_rename(
                                    &record.invocation_ref,
                                    terminal.status,
                                    result,
                                    text,
                                    terminal.summary,
                                    current_unix_ms(),
                                    rename,
                                )?
                            }
                            Some(ConversationIntrinsicMutation::Archive(archive)) => {
                                admission.publish_root_action_completed_with_archive(
                                    &record.invocation_ref,
                                    terminal.status,
                                    result,
                                    text,
                                    terminal.summary,
                                    current_unix_ms(),
                                    archive,
                                )?
                            }
                            None => admission.publish_root_action_completed(
                                &record.invocation_ref,
                                terminal.status,
                                result,
                                text,
                                terminal.summary,
                                current_unix_ms(),
                            )?,
                        }
                    } else {
                        admission.publish_root_failure(
                            &record.invocation_ref,
                            terminal,
                            text,
                            current_unix_ms(),
                        )?
                    }
                }
                InvocationOrigin::HostCall { .. } => match mutation.clone() {
                    Some(ConversationIntrinsicMutation::Rename(rename)) => {
                        admission.publish_child_action_terminal_with_rename(
                            &record.invocation_ref,
                            terminal,
                            text,
                            current_unix_ms(),
                            rename,
                        )?
                    }
                    Some(ConversationIntrinsicMutation::Archive(archive)) => {
                        admission.publish_child_action_terminal_with_archive(
                            &record.invocation_ref,
                            terminal,
                            text,
                            current_unix_ms(),
                            archive,
                        )?
                    }
                    None => admission.publish_child_action_terminal(
                        &record.invocation_ref,
                        terminal,
                        text,
                        current_unix_ms(),
                    )?,
                },
            }));
            let mut authority_stops=Vec::new();
            let publication=match attempted {
                Ok(publication)=>publication,
                Err(error) if guarded && error.is::<crate::cwpdb::conversation::SourceAuthorityConflict>()=>{
                    let (publication,stops)=publish_source_authority_failure(&mut state.admission,record,error)?;
                    authority_stops=stops;
                    publication
                },
                Err(error)=>return Err(error),
            };
            for reference in &authority_stops {state.protected_upstream_results.remove(reference);}
            state.protected_upstream_results.remove(&record.invocation_ref);
            self.sync_archive_reservations(&mut state)?;
            let mut stopped=if archive_stops.is_some() {new_archive_stops(&state, &before)} else {Vec::new()};
            stopped.extend(authority_stops.into_iter().filter(|reference|reference!=&record.invocation_ref));
            Ok((publication,stopped))
        })();
        let (publication, mut stopped) = match publication {
            Ok(publication) => publication,
            Err(error) => {
                if let (Some(fence), Some(ConversationIntrinsicMutation::Archive(archive))) = (&archive_stops, &mutation) {
                    self.settle_prepared_archive_cancellation(fence, archive)?;
                }
                return Err(error);
            }
        };
        stopped.extend(self.settle_canceled_archive_stops(
            &archive_stops,
            &mutation,
            &publication,
        )?);
        self.signal_archive_stops(&stopped)?;
        // External observers can synchronously submit another control. Release
        // mutation and cancellation guards once settlement is complete.
        if let Some(fence) = &archive_stops { fence._order.release(); }
        drop(archive_stops);
        for reference in stopped {
            if let Some((_, target_actor_ref, response)) = self.terminal_execution_for_invocation(&reference)? {
                observer(LocalRuntimeEvent::Terminal {
                    target_actor_ref, response: Box::new(response), message: None,
                });
            }
        }
        self.lock()?.admission.clear_provider_questions(false)?;
        match publication {
            PublishInvocationTerminalOutcome::Accepted {
                response,
                message,
                woken_invocation_refs,
            } => {
                let mut queue = queue.borrow_mut();
                for invocation_ref in woken_invocation_refs.into_iter().rev() {
                    queue.push_front(invocation_ref);
                }
                drop(queue);
                observer(LocalRuntimeEvent::Terminal {
                    target_actor_ref: record.target_actor_ref.clone(),
                    response: Box::new(response),
                    message: message.map(Box::new),
                });
            }
            PublishInvocationTerminalOutcome::AlreadyTerminal { .. } => {
                // A stop can arrive between the pre-publication check and commit.
                self.settle_stopped_execution(&record.invocation_ref)?;
            }
        }
        Ok(())
    }

    fn lock(&self) -> CwrResult<MutexGuard<'_, LocalRuntimeState>> {
        self.inner
            .lock()
            .map_err(|_| invalid_error("local runtime lock was poisoned"))
    }
}

fn publish_source_authority_failure(
    admission:&mut crate::v2026_09::conversation::ConversationAdmissionStore,
    record:&ActionInvocationRecord,
    original_error:Box<dyn std::error::Error+Send+Sync>,
)->CwrResult<(PublishInvocationTerminalOutcome,Vec<String>)> {
    // The guarded transaction rolled back source-dependent bytes. Stop through
    // the ordinary ledger path so descendants, waits and public status agree.
    let summary="Connected resource authority changed before result publication.".to_owned();
    let request=crate::v2026_09::invocation::InvocationStopRequest {
        requested_at_unix_ms:current_unix_ms(),
        requested_by_actor_ref:None,
        request_id:Some(format!("source-authority-{}",Uuid::new_v4().simple())),
        reason:Some(summary.clone()),
    };
    let persisted=match &record.origin {
        InvocationOrigin::Ingress{..}=>admission.publish_root_stop(&record.invocation_ref,request,summary),
        InvocationOrigin::HostCall{..}=>admission.publish_child_action_stop(&record.invocation_ref,request,summary),
    }?;
    use crate::v2026_09::conversation::PublishInvocationStopOutcome;
    match persisted {
        PublishInvocationStopOutcome::Accepted{response,message,stopped_invocation_refs,woken_invocation_refs}=>Ok((PublishInvocationTerminalOutcome::Accepted{response,message,woken_invocation_refs},stopped_invocation_refs)),
        PublishInvocationStopOutcome::TooLate{response,message}=>Ok((PublishInvocationTerminalOutcome::AlreadyTerminal{response,message},Vec::new())),
        PublishInvocationStopOutcome::Unknown=>Err(original_error),
    }
}
