impl ConversationAdmissionStore {
    pub fn publish_child_action_terminal(
        &mut self,
        invocation_ref: &str,
        terminal: InvocationTerminal,
        text: String,
        finished_at_unix_ms: u64,
    ) -> CwrResult<PublishInvocationTerminalOutcome> {
        self.publish_child_action_terminal_inner(
            invocation_ref,
            terminal,
            text,
            finished_at_unix_ms,
            None,
        )
    }

    pub fn publish_child_action_terminal_with_rename(
        &mut self,
        invocation_ref: &str,
        terminal: InvocationTerminal,
        text: String,
        finished_at_unix_ms: u64,
        rename: ConversationRenameMutation,
    ) -> CwrResult<PublishInvocationTerminalOutcome> {
        self.publish_child_action_terminal_inner(
            invocation_ref,
            terminal,
            text,
            finished_at_unix_ms,
            Some(ConversationIntrinsicMutation::Rename(rename)),
        )
    }

    pub(crate) fn publish_child_action_terminal_with_archive(
        &mut self,
        invocation_ref: &str,
        terminal: InvocationTerminal,
        text: String,
        finished_at_unix_ms: u64,
        archive: ConversationArchiveMutation,
    ) -> CwrResult<PublishInvocationTerminalOutcome> {
        self.publish_child_action_terminal_inner(
            invocation_ref,
            terminal,
            text,
            finished_at_unix_ms,
            Some(ConversationIntrinsicMutation::Archive(archive)),
        )
    }

    fn publish_child_action_terminal_inner(
        &mut self,
        invocation_ref: &str,
        terminal: InvocationTerminal,
        text: String,
        finished_at_unix_ms: u64,
        mutation: Option<ConversationIntrinsicMutation>,
    ) -> CwrResult<PublishInvocationTerminalOutcome> {
        if let Some(existing) = existing_child_terminal(&self.state, invocation_ref)? {
            return Ok(existing);
        }
        if text.trim().is_empty() {
            return invalid("conversation action result text must not be empty");
        }
        require_child(&self.state, invocation_ref)?;
        self.transact(|staged| {
            claim_if_admitted(staged, invocation_ref, finished_at_unix_ms)?;
            match staged
                .invocation_ledger
                .finish(invocation_ref, terminal, finished_at_unix_ms)?
            {
                FinishInvocationOutcome::AlreadyTerminal { response } => {
                    Ok(PublishInvocationTerminalOutcome::AlreadyTerminal {
                        response,
                        message: existing_child_result(staged, invocation_ref)?,
                    })
                }
                FinishInvocationOutcome::Accepted {
                    response,
                    woken_invocation_refs,
                } => {
                    if let Some(mutation) = &mutation {
                        stage_conversation_intrinsic_mutation(staged, mutation)?;
                    }
                    let message = stage_child_projection(
                        staged,
                        invocation_ref,
                        &response,
                        text,
                        finished_at_unix_ms,
                    )?;

                    Ok(PublishInvocationTerminalOutcome::Accepted {
                        response,
                        message: Some(message),
                        woken_invocation_refs,
                    })
                }
            }
        })
    }

    pub fn publish_child_action_stop(
        &mut self,
        invocation_ref: &str,
        request: InvocationStopRequest,
        text: String,
    ) -> CwrResult<PublishInvocationStopOutcome> {
        if let Some(existing) = existing_child_stop(&self.state, invocation_ref, &request)? {
            return Ok(existing);
        }
        if text.trim().is_empty() {
            return invalid("conversation action result text must not be empty");
        }
        require_child(&self.state, invocation_ref)?;
        let occurred_at_unix_ms = request.requested_at_unix_ms;
        self.transact(
            |staged| match staged.invocation_ledger.stop(invocation_ref, request)? {
                StopInvocationOutcome::Accepted {
                    response,
                    stopped_invocation_refs,
                    woken_invocation_refs,
                    replayed,
                } => {
                    let mut requested_message = None;
                    for stopped_ref in &stopped_invocation_refs {
                        let stopped_response = retained_terminal_response(staged, stopped_ref)?;
                        let stopped_text = if stopped_ref == invocation_ref {
                            text.clone()
                        } else {
                            stopped_response.summary.clone()
                        };
                        let message = if replayed {
                            match existing_child_result(staged, stopped_ref)? {
                                Some(message) => message,
                                None => stage_child_projection_without_activity(
                                    staged,
                                    stopped_ref,
                                    &stopped_response,
                                    stopped_text,
                                )?,
                            }
                        } else {
                            stage_child_projection(
                                staged,
                                stopped_ref,
                                &stopped_response,
                                stopped_text,
                                occurred_at_unix_ms,
                            )?
                        };
                        if stopped_ref == invocation_ref {
                            requested_message = Some(message);
                        }
                    }
                    let message =
                        requested_message.or(existing_child_result(staged, invocation_ref)?);

                    Ok(PublishInvocationStopOutcome::Accepted {
                        response,
                        message,
                        stopped_invocation_refs,
                        woken_invocation_refs,
                    })
                }
                StopInvocationOutcome::TooLate { response } => {
                    Ok(PublishInvocationStopOutcome::TooLate {
                        response,
                        message: existing_child_result(staged, invocation_ref)?,
                    })
                }
                StopInvocationOutcome::Unknown => Ok(PublishInvocationStopOutcome::Unknown),
            },
        )
    }
}
