impl ConversationAdmissionStore {
    pub fn complete(
        &mut self,
        invocation_ref: &str,
        result: Value,
    ) -> CwrResult<ConversationMessage> {
        if self.invocation_record(invocation_ref).is_none() {
            return self.complete_legacy(
                invocation_ref,
                ActionResponseStatus::COMPLETED,
                result,
                None,
            );
        }
        published_message(self.publish_root_message_completed(
            invocation_ref,
            ActionResponseStatus::COMPLETED,
            result,
            "Completed".into(),
            current_unix_ms(),
        )?)
    }

    pub fn complete_with_activity(
        &mut self,
        invocation_ref: &str,
        result: Value,
        status: u16,
        summary: String,
    ) -> CwrResult<ConversationMessage> {
        let status =
            ActionResponseStatus::new(status).map_err(|error| invalid_error(error.to_string()))?;
        if !status.is_completed() {
            return invalid(format!(
                "completed activity status must be 2xx, not {}",
                status.as_u16()
            ));
        }
        let activity = ConversationActivityEvent::TurnCompleted {
            status,
            summary: summary.clone(),
            result: Some(result.clone()),
        };
        if self.invocation_record(invocation_ref).is_none() {
            return self.complete_legacy(invocation_ref, status, result, Some(activity));
        }
        published_message(self.publish_root_message_completed_inner(
            invocation_ref,
            status,
            result,
            summary,
            current_unix_ms(),
            Some(activity),
            None,
        )?)
    }

    pub fn complete_action(
        &mut self,
        invocation_ref: &str,
        result: Value,
        text: String,
    ) -> CwrResult<ConversationMessage> {
        if self.invocation_record(invocation_ref).is_none() {
            return self.complete_action_legacy(
                invocation_ref,
                ActionResponseStatus::COMPLETED,
                result,
                text,
                None,
            );
        }
        published_message(self.publish_root_action_completed(
            invocation_ref,
            ActionResponseStatus::COMPLETED,
            result,
            text,
            "Completed".into(),
            current_unix_ms(),
        )?)
    }

    pub fn complete_action_with_activity(
        &mut self,
        invocation_ref: &str,
        result: Value,
        text: String,
        status: u16,
        summary: String,
    ) -> CwrResult<ConversationMessage> {
        let status =
            ActionResponseStatus::new(status).map_err(|error| invalid_error(error.to_string()))?;
        if !status.is_completed() {
            return invalid(format!(
                "completed activity status must be 2xx, not {}",
                status.as_u16()
            ));
        }
        let activity = ConversationActivityEvent::TurnCompleted {
            status,
            summary: summary.clone(),
            result: Some(result.clone()),
        };
        if self.invocation_record(invocation_ref).is_none() {
            return self.complete_action_legacy(
                invocation_ref,
                status,
                result,
                text,
                Some(activity),
            );
        }
        published_message(self.publish_root_action_completed_inner(
            invocation_ref,
            status,
            result,
            text,
            summary,
            current_unix_ms(),
            Some(activity),
            None,
        )?)
    }

    pub fn fail(
        &mut self,
        invocation_ref: &str,
        summary: String,
    ) -> CwrResult<Option<ConversationMessage>> {
        if self.invocation_record(invocation_ref).is_none() {
            return self.fail_legacy(invocation_ref, summary, None);
        }
        let terminal = InvocationTerminal {
            status: ActionResponseStatus::FAILED,
            summary: summary.clone(),
            result: None,
            error: Some(ActionResponseDetail::new("adapter_failure").with_message(summary.clone())),
        };
        published_optional_message(self.publish_root_failure(
            invocation_ref,
            terminal,
            summary,
            current_unix_ms(),
        )?)
    }

    pub fn fail_with_activity(
        &mut self,
        invocation_ref: &str,
        summary: String,
        error: String,
    ) -> CwrResult<Option<ConversationMessage>> {
        let activity = ConversationActivityEvent::TurnFailed {
            status: ActionResponseStatus::FAILED,
            error: error.clone(),
        };
        if self.invocation_record(invocation_ref).is_none() {
            return self.fail_legacy(invocation_ref, summary, Some(activity));
        }
        let terminal = InvocationTerminal {
            status: ActionResponseStatus::FAILED,
            summary: summary.clone(),
            result: None,
            error: Some(ActionResponseDetail::new("adapter_error").with_message(error)),
        };
        published_optional_message(self.publish_root_failure_inner(
            invocation_ref,
            terminal,
            summary,
            current_unix_ms(),
            Some(activity),
        )?)
    }

    pub fn publish_root_message_completed(
        &mut self,
        invocation_ref: &str,
        status: ActionResponseStatus,
        result: Value,
        summary: String,
        finished_at_unix_ms: u64,
    ) -> CwrResult<PublishInvocationTerminalOutcome> {
        self.publish_root_message_completed_inner(
            invocation_ref,
            status,
            result,
            summary,
            finished_at_unix_ms,
            None,
            None,
        )
    }

    pub(crate) fn publish_root_message_completed_with_rename(
        &mut self,
        invocation_ref: &str,
        status: ActionResponseStatus,
        result: Value,
        summary: String,
        finished_at_unix_ms: u64,
        rename: ConversationRenameMutation,
    ) -> CwrResult<PublishInvocationTerminalOutcome> {
        self.publish_root_message_completed_inner(
            invocation_ref,
            status,
            result,
            summary,
            finished_at_unix_ms,
            None,
            Some(ConversationIntrinsicMutation::Rename(rename)),
        )
    }

    pub(crate) fn publish_root_message_completed_with_archive(
        &mut self,
        invocation_ref: &str,
        status: ActionResponseStatus,
        result: Value,
        summary: String,
        finished_at_unix_ms: u64,
        archive: ConversationArchiveMutation,
    ) -> CwrResult<PublishInvocationTerminalOutcome> {
        self.publish_root_message_completed_inner(
            invocation_ref,
            status,
            result,
            summary,
            finished_at_unix_ms,
            None,
            Some(ConversationIntrinsicMutation::Archive(archive)),
        )
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "Terminal publication commits the complete response and optional title as one transition."
    )]
    fn publish_root_message_completed_inner(
        &mut self,
        invocation_ref: &str,
        status: ActionResponseStatus,
        result: Value,
        summary: String,
        finished_at_unix_ms: u64,
        activity: Option<ConversationActivityEvent>,
        mutation: Option<ConversationIntrinsicMutation>,
    ) -> CwrResult<PublishInvocationTerminalOutcome> {
        if !status.is_completed() {
            return invalid(format!(
                "root completion status must be 2xx, not {}",
                status.as_u16()
            ));
        }
        if let Some(existing) = existing_root_terminal(&self.state, invocation_ref)? {
            return Ok(existing);
        }
        let invocation = self
            .invocation_record(invocation_ref)
            .ok_or_else(|| invalid_error(format!("unknown invocation ref {invocation_ref}")))?;
        if invocation.action_name != RESPOND_TO_MESSAGE_ACTION_NAME {
            return invalid(format!(
                "invocation {invocation_ref} is not a {RESPOND_TO_MESSAGE_ACTION_NAME} action"
            ));
        }
        let parsed: RespondToMessageResult =
            serde_json::from_value(result.clone()).map_err(|error| {
                invalid_error(format!(
                    "{RESPOND_TO_MESSAGE_ACTION_NAME} result is invalid: {error}"
                ))
            })?;
        if parsed.text.trim().is_empty() {
            return invalid(format!(
                "{RESPOND_TO_MESSAGE_ACTION_NAME} result text must not be empty"
            ));
        }
        self.publish_root_finish(
            invocation_ref,
            InvocationTerminal {
                status,
                summary,
                result: Some(result),
                error: None,
            },
            parsed.text,
            RootReplyKind::Message,
            finished_at_unix_ms,
            activity,
            mutation,
        )
    }

    pub fn publish_root_action_completed(
        &mut self,
        invocation_ref: &str,
        status: ActionResponseStatus,
        result: Value,
        text: String,
        summary: String,
        finished_at_unix_ms: u64,
    ) -> CwrResult<PublishInvocationTerminalOutcome> {
        self.publish_root_action_completed_inner(
            invocation_ref,
            status,
            result,
            text,
            summary,
            finished_at_unix_ms,
            None,
            None,
        )
    }

    #[expect(clippy::too_many_arguments, reason = "The action result and title commit together.")]
    pub(crate) fn publish_root_action_completed_with_rename(
        &mut self,
        invocation_ref: &str,
        status: ActionResponseStatus,
        result: Value,
        text: String,
        summary: String,
        finished_at_unix_ms: u64,
        rename: ConversationRenameMutation,
    ) -> CwrResult<PublishInvocationTerminalOutcome> {
        self.publish_root_action_completed_inner(
            invocation_ref,
            status,
            result,
            text,
            summary,
            finished_at_unix_ms,
            None,
            Some(ConversationIntrinsicMutation::Rename(rename)),
        )
    }

    #[expect(clippy::too_many_arguments, reason = "The action result and archive state commit together.")]
    pub(crate) fn publish_root_action_completed_with_archive(
        &mut self,
        invocation_ref: &str,
        status: ActionResponseStatus,
        result: Value,
        text: String,
        summary: String,
        finished_at_unix_ms: u64,
        archive: ConversationArchiveMutation,
    ) -> CwrResult<PublishInvocationTerminalOutcome> {
        self.publish_root_action_completed_inner(
            invocation_ref,
            status,
            result,
            text,
            summary,
            finished_at_unix_ms,
            None,
            Some(ConversationIntrinsicMutation::Archive(archive)),
        )
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "Terminal publication commits the complete response and optional activity as one transition."
    )]
    fn publish_root_action_completed_inner(
        &mut self,
        invocation_ref: &str,
        status: ActionResponseStatus,
        result: Value,
        text: String,
        summary: String,
        finished_at_unix_ms: u64,
        activity: Option<ConversationActivityEvent>,
        mutation: Option<ConversationIntrinsicMutation>,
    ) -> CwrResult<PublishInvocationTerminalOutcome> {
        if !status.is_completed() {
            return invalid(format!(
                "root completion status must be 2xx, not {}",
                status.as_u16()
            ));
        }
        if let Some(existing) = existing_root_terminal(&self.state, invocation_ref)? {
            return Ok(existing);
        }
        if text.trim().is_empty() {
            return invalid("conversation action result text must not be empty");
        }
        let invocation = self
            .invocation_record(invocation_ref)
            .ok_or_else(|| invalid_error(format!("unknown invocation ref {invocation_ref}")))?;
        if invocation.action_name == RESPOND_TO_MESSAGE_ACTION_NAME {
            return invalid(format!(
                "invocation {invocation_ref} is not a direct conversation action"
            ));
        }
        self.publish_root_finish(
            invocation_ref,
            InvocationTerminal {
                status,
                summary,
                result: Some(result.clone()),
                error: None,
            },
            text,
            RootReplyKind::CompletedAction(result),
            finished_at_unix_ms,
            activity,
            mutation,
        )
    }

    pub fn publish_root_failure(
        &mut self,
        invocation_ref: &str,
        terminal: InvocationTerminal,
        text: String,
        finished_at_unix_ms: u64,
    ) -> CwrResult<PublishInvocationTerminalOutcome> {
        self.publish_root_failure_inner(invocation_ref, terminal, text, finished_at_unix_ms, None)
    }

    fn publish_root_failure_inner(
        &mut self,
        invocation_ref: &str,
        terminal: InvocationTerminal,
        text: String,
        finished_at_unix_ms: u64,
        activity: Option<ConversationActivityEvent>,
    ) -> CwrResult<PublishInvocationTerminalOutcome> {
        if let Some(existing) = existing_root_terminal(&self.state, invocation_ref)? {
            return Ok(existing);
        }
        if !terminal.status.is_blocked() && !terminal.status.is_failed() {
            return invalid("root failure publication requires a blocked or failed terminal");
        }
        if text.trim().is_empty() {
            return invalid("adapter failure summary must not be empty");
        }
        let reply_kind = root_non_success_reply_kind(&self.state, invocation_ref)?;
        self.publish_root_finish(
            invocation_ref,
            terminal,
            text,
            reply_kind,
            finished_at_unix_ms,
            activity,
            None,
        )
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "Terminal publication commits the complete response and optional title as one transition."
    )]
    fn publish_root_finish(
        &mut self,
        invocation_ref: &str,
        terminal: InvocationTerminal,
        text: String,
        reply_kind: RootReplyKind,
        finished_at_unix_ms: u64,
        activity: Option<ConversationActivityEvent>,
        mutation: Option<ConversationIntrinsicMutation>,
    ) -> CwrResult<PublishInvocationTerminalOutcome> {
        self.transact(|staged| {
            claim_if_admitted(staged, invocation_ref, finished_at_unix_ms)?;
            match staged
                .invocation_ledger
                .finish(invocation_ref, terminal, finished_at_unix_ms)?
            {
                FinishInvocationOutcome::AlreadyTerminal { response } => {
                    Ok(PublishInvocationTerminalOutcome::AlreadyTerminal {
                        response,
                        message: existing_root_reply(staged, invocation_ref)?,
                    })
                }
                FinishInvocationOutcome::Accepted {
                    response,
                    woken_invocation_refs,
                } => {
                    if let Some(mutation) = &mutation {
                        stage_conversation_intrinsic_mutation(staged, mutation)?;
                    }
                    let activity = match activity {
                        Some(activity) => activity,
                        None => terminal_activity(&response)?,
                    };
                    let message = stage_root_projection(
                        staged,
                        invocation_ref,
                        &response,
                        text,
                        reply_kind,
                        Some((activity, finished_at_unix_ms)),
                    )?;

                    Ok(PublishInvocationTerminalOutcome::Accepted {
                        response,
                        message,
                        woken_invocation_refs,
                    })
                }
            }
        })
    }

    pub fn publish_root_stop(
        &mut self,
        invocation_ref: &str,
        request: InvocationStopRequest,
        text: String,
    ) -> CwrResult<PublishInvocationStopOutcome> {
        if let Some(existing) = existing_root_stop(&self.state, invocation_ref, &request)? {
            return Ok(existing);
        }
        if text.trim().is_empty() {
            return invalid("stopped invocation reply text must not be empty");
        }
        let occurred_at_unix_ms = request.requested_at_unix_ms;
        self.transact(
            |staged| match staged.invocation_ledger.stop(invocation_ref, request)? {
                StopInvocationOutcome::Accepted {
                    response,
                    stopped_invocation_refs,
                    woken_invocation_refs,
                    replayed,
                } => {
                    for stopped_ref in &stopped_invocation_refs {
                        if stopped_ref == invocation_ref {
                            continue;
                        }
                        let stopped_response = retained_terminal_response(staged, stopped_ref)?;
                        if replayed {
                            if existing_child_result(staged, stopped_ref)?.is_none() {
                                stage_child_projection_without_activity(
                                    staged,
                                    stopped_ref,
                                    &stopped_response,
                                    stopped_response.summary.clone(),
                                )?;
                            }
                        } else {
                            stage_child_projection(
                                staged,
                                stopped_ref,
                                &stopped_response,
                                stopped_response.summary.clone(),
                                occurred_at_unix_ms,
                            )?;
                        }
                    }
                    let reply_kind = root_non_success_reply_kind(staged, invocation_ref)?;
                    let message = if !stopped_invocation_refs.iter().any(|r| r == invocation_ref) {
                        existing_root_reply(staged, invocation_ref)?
                    } else if replayed {
                        match existing_root_reply(staged, invocation_ref)? {
                            Some(message) => Some(message),
                            None if matches!(&reply_kind, RootReplyKind::None) => None,
                            None => {
                                let admission_index = root_admission_index(staged, invocation_ref)?;
                                stage_root_projection_at(
                                    staged,
                                    admission_index,
                                    invocation_ref,
                                    &response,
                                    text,
                                    reply_kind,
                                    None,
                                )?
                            }
                        }
                    } else {
                        let activity = terminal_activity(&response)?;
                        stage_root_projection(
                            staged,
                            invocation_ref,
                            &response,
                            text,
                            reply_kind,
                            Some((activity, occurred_at_unix_ms)),
                        )?
                    };

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
                        message: existing_root_reply(staged, invocation_ref)?,
                    })
                }
                StopInvocationOutcome::Unknown => Ok(PublishInvocationStopOutcome::Unknown),
            },
        )
    }
}
