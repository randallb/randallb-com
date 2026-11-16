struct ArchiveStopFence {
    _mutation: LocalExecutionPermit,
    _order: Arc<LocalArchiveReservation>,
    archived: bool,
    runtime: LocalRuntime,
    conversation_ref: String,
    roots: Vec<UpstreamStopFence>,
}

impl Drop for ArchiveStopFence {
    fn drop(&mut self) {
        if !self.archived { return; }
        if let Ok(mut state) = self.runtime.lock() {
            if let Some(count) = state
                .archiving_conversations
                .get_mut(&self.conversation_ref)
            {
                *count -= 1;
                if *count == 0 {
                    state.archiving_conversations.remove(&self.conversation_ref);
                }
            }
        }
    }
}

impl LocalRuntime {
    fn prepare_archive_terminal_stops(
        &self,
        record: &ActionInvocationRecord,
        mutation: &Option<ConversationIntrinsicMutation>,
    ) -> CwrResult<Option<ArchiveStopFence>> {
        if self
            .lock()?
            .admission
            .invocation_record(&record.invocation_ref)
            .is_some_and(|record| record.state.is_terminal())
        {
            return Ok(None);
        }
        match mutation {
            Some(ConversationIntrinsicMutation::Archive(archive)) => {
                self.prepare_archive_stops(archive, Some(&record.invocation_ref))
            }
            _ => Ok(None),
        }
    }

    fn settle_canceled_archive_stops(
        &self,
        fence: &Option<ArchiveStopFence>,
        mutation: &Option<ConversationIntrinsicMutation>,
        publication: &PublishInvocationTerminalOutcome,
    ) -> CwrResult<Vec<String>> {
        if let (
            Some(fence),
            Some(ConversationIntrinsicMutation::Archive(archive)),
            PublishInvocationTerminalOutcome::AlreadyTerminal { .. },
        ) = (fence, mutation, publication)
        {
            // Provider cancellation was already requested; it cannot be undone.
            // Settle the original work, including claims not yet registered with a provider.
            // Leave the archive flag and new independent requests alone.
            return self.settle_prepared_archive_cancellation(fence, archive);
        }
        Ok(Vec::new())
    }

    fn settle_prepared_archive_cancellation(
        &self, fence: &ArchiveStopFence, archive: &ConversationArchiveMutation,
    ) -> CwrResult<Vec<String>> {
        let stopped = {
            let mut state = self.lock()?;
            let before = stopped_archive_snapshot(&state);
            state.admission.settle_archive_cancellation(
                archive,
                fence.roots.iter().map(|fence| fence.root.clone()).collect(),
            )?;
            self.sync_archive_reservations(&mut state)?;
            new_archive_stops(&state, &before)
        };
        self.signal_archive_stops(&stopped)?;
        Ok(stopped)
    }

    // Include completed ancestors: their delegated work may still be running.
    // Fence every existing request before waiting for providers or committing.
    // Queued work cannot start and active callers cannot delegate through a race.
    fn prepare_archive_stops(
        &self,
        archive: &ConversationArchiveMutation,
        invocation_ref: Option<&str>,
    ) -> CwrResult<Option<ArchiveStopFence>> {
        let order = {
            let mut state = self.lock()?;
            self.sync_archive_reservations(&mut state)?;
            invocation_ref.and_then(|reference| state.archive_reservations.get(reference).cloned())
        };
        let order = match order {
            Some(order) => order,
            None => self.reserve_conversation_archive(&archive.conversation_ref)?,
        };
        self.prepare_archive_stops_reserved(archive, invocation_ref, order)
    }

    fn prepare_archive_stops_reserved(
        &self, archive: &ConversationArchiveMutation, invocation_ref: Option<&str>,
        order: Arc<LocalArchiveReservation>,
    ) -> CwrResult<Option<ArchiveStopFence>> {
        self.prepare_archive_stops_reserved_in_upstream_scope(archive, invocation_ref, order, None)
    }

    fn prepare_archive_stops_reserved_in_upstream_scope(
        &self, archive: &ConversationArchiveMutation, invocation_ref: Option<&str>,
        order: Arc<LocalArchiveReservation>, owner: Option<&str>,
    ) -> CwrResult<Option<ArchiveStopFence>> {
        if !order.wait(|| {
            let state = self.lock()?;
            Ok(invocation_ref.is_some_and(|reference| {
                state
                    .admission
                    .invocation_record(reference)
                    .is_none_or(|record| record.state.is_terminal())
            }))
        })? {
            return Ok(None);
        }
        // Both connected-host controls and scheduled intrinsic mutations use
        // this guard, so a restore cannot overtake an archive waiting for ack.
        let mutation = self.archive_mutations.acquire(&archive.conversation_ref, || Ok(true))?;
        if !archive.archived {
            return Ok(Some(ArchiveStopFence {
                _mutation: mutation, _order: order, archived: false, runtime: self.clone(),
                conversation_ref: archive.conversation_ref.clone(), roots: Vec::new(),
            }));
        }
        let (fences, upstream) = {
            let mut state = self.lock()?;
            local_catalog(&upstream_graph(&state, owner)?, &archive.caller_actor_ref)?;
            // The ordering guard may have waited while another control canceled
            // this invocation. Do not select or stop newer work in that case.
            if invocation_ref.is_some_and(|reference| {
                state
                    .admission
                    .invocation_record(reference)
                    .is_some_and(|record| record.state.is_terminal())
            }) {
                return Ok(None);
            }
            state.admission.prepare_conversation_archive(
                &archive.conversation_ref,
                &archive.caller_actor_ref,
                true,
            )?;
            *state
                .archiving_conversations
                .entry(archive.conversation_ref.clone())
                .or_default() += 1;
            let roots = state.admission.archive_stop_targets(&archive.conversation_ref);
            let upstream = state
                .upstream_work
                .keys()
                .filter(|reference| {
                    roots
                        .iter()
                        .any(|root| descends_from(&state, reference, root))
                })
                .cloned()
                .collect::<Vec<_>>();
            let fences = roots
                .into_iter()
                .map(|root| {
                    *state.stopping_roots.entry(root.clone()).or_default() += 1;
                    UpstreamStopFence {
                        runtime: self.clone(),
                        root,
                    }
                })
                .collect::<Vec<_>>();
            (fences, upstream)
        };
        let fence = ArchiveStopFence {
            _mutation: mutation,
            _order: order,
            archived: true,
            runtime: self.clone(),
            conversation_ref: archive.conversation_ref.clone(),
            roots: fences,
        };
        let local_controls = {
            let state = self.lock()?;
            state
                .active
                .iter()
                .filter(|(reference, _)| {
                    fence
                        .roots
                        .iter()
                        .any(|root| descends_from(&state, reference, &root.root))
                })
                .map(|(_, control)| control.clone())
                .collect::<Vec<_>>()
        };
        for control in local_controls { control.stop(); }
        let cancellation = (|| -> CwrResult<()> {
            for reference in &upstream { self.request_upstream_stop(reference)?; }
            for reference in upstream { self.wait_upstream_stop_until(&reference, invocation_ref)?; }
            // A disconnected provider or a reopened store may have no in-memory
            // work entry. Its retained Stopping state still requires settlement.
            loop {
                let state = self.lock()?;
                if invocation_ref.is_some_and(|reference| state.admission.invocation_record(reference)
                    .is_none_or(|record| record.state.is_terminal())) { break; }
                let pending = fence.roots.iter().any(|root| state.admission.invocation_record(&root.root)
                    .is_some_and(|record| record.state == ActionInvocationState::Stopping));
                if !pending { break; }
                drop(state);
                thread::sleep(Duration::from_millis(10));
            }
            Ok(())
        })();
        if let Err(error) = cancellation {
            self.settle_prepared_archive_cancellation(&fence, archive)?;
            return Err(error);
        }
        Ok(Some(fence))
    }

    fn signal_archive_stops(&self, stopped: &[String]) -> CwrResult<()> {
        if stopped.is_empty() { return Ok(()); }
        let controls = {
            let mut state = self.lock()?;
            let controls = stopped
                .iter()
                .filter_map(|r| state.active.get(r).cloned())
                .collect::<Vec<_>>();
            state.admission.clear_provider_questions(false)?;
            controls
        };
        for control in controls {
            control.stop();
        }
        Ok(())
    }
}

// Snapshot and compare under the same runtime lock as the archive transaction.
// Historical and concurrently completed stops must not be published again.
fn stopped_archive_snapshot(state: &LocalRuntimeState) -> BTreeSet<String> {
    state
        .admission
        .invocation_records()
        .filter(|record| {
            matches!(
                record.state,
                ActionInvocationState::Stopping | ActionInvocationState::Stopped
            )
        })
        .map(|record| record.invocation_ref.clone())
        .collect()
}

fn new_archive_stops(state: &LocalRuntimeState, before: &BTreeSet<String>) -> Vec<String> {
    state
        .admission
        .invocation_records()
        .filter(|record| {
            matches!(
                record.state,
                ActionInvocationState::Stopping | ActionInvocationState::Stopped
            ) && !before.contains(&record.invocation_ref)
        })
        .map(|record| record.invocation_ref.clone())
        .collect()
}
