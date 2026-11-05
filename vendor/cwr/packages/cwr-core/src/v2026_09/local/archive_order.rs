// Reserve at admission/acceptance, before any detached worker can be delayed.
#[derive(Default)]
struct LocalArchiveOrder {
    queues: Mutex<BTreeMap<String, VecDeque<String>>>,
    changed: Condvar,
}

pub struct LocalArchiveReservation {
    order: Arc<LocalArchiveOrder>,
    conversation: String,
    id: String,
}

impl LocalArchiveReservation {
    fn wait(&self, mut canceled: impl FnMut() -> CwrResult<bool>) -> CwrResult<bool> {
        loop {
            // Never hold the queue mutex while checking runtime state: admission
            // reserves tickets under the runtime lock in the opposite order.
            if canceled()? {
                return Ok(false);
            }
            let queues = self
                .order
                .queues
                .lock()
                .map_err(|_| invalid_error("archive ordering is unavailable"))?;
            if queues
                .get(&self.conversation)
                .and_then(|queue| queue.front())
                == Some(&self.id)
            {
                return Ok(true);
            }
            let _ = self
                .order
                .changed
                .wait_timeout(queues, Duration::from_millis(25))
                .map_err(|_| invalid_error("archive ordering is unavailable"))?;
        }
    }
}

impl LocalArchiveReservation {
    fn release(&self) {
        if let Ok(mut queues) = self.order.queues.lock() {
            if let Some(queue) = queues.get_mut(&self.conversation) {
                queue.retain(|id| id != &self.id);
                if queue.is_empty() {
                    queues.remove(&self.conversation);
                }
            }
            self.order.changed.notify_all();
        }
    }
}

impl LocalRuntime {
    pub fn reserve_conversation_archive(
        &self,
        conversation: &str,
    ) -> CwrResult<Arc<LocalArchiveReservation>> {
        let mut state = self.lock()?;
        self.sync_archive_reservations(&mut state)?;
        self.reserve_archive_order(conversation)
    }

    fn sync_archive_reservations(&self, state: &mut LocalRuntimeState) -> CwrResult<()> {
        state.archive_reservations.retain(|reference, _| {
            state
                .admission
                .invocation_record(reference)
                .is_some_and(|record| !record.state.is_terminal())
        });
        let refs = order_runnable_refs(
            &state.admission,
            state
                .admission
                .runnable_invocation_refs()
                .into_iter()
                .collect(),
        );
        for reference in refs {
            let Some(record) = state.admission.invocation_record(&reference) else {
                continue;
            };
            if record.target_actor_ref != INTRINSIC_CWR_ACTOR_REF
                || !matches!(
                    record.action_name.as_str(),
                    "archive--conversation" | "restore--conversation"
                )
                || state.archive_reservations.contains_key(&reference)
            {
                continue;
            }
            if let Some(conversation) = &record.conversation_ref {
                let order = self.reserve_archive_order(conversation)?;
                state.archive_reservations.insert(reference, order);
            }
        }
        Ok(())
    }

    fn reserve_archive_order(&self, conversation: &str) -> CwrResult<Arc<LocalArchiveReservation>> {
        let id = Uuid::new_v4().to_string();
        self.archive_order
            .queues
            .lock()
            .map_err(|_| invalid_error("archive ordering is unavailable"))?
            .entry(conversation.into())
            .or_default()
            .push_back(id.clone());
        Ok(Arc::new(LocalArchiveReservation {
            order: Arc::clone(&self.archive_order),
            conversation: conversation.into(),
            id,
        }))
    }
}

impl Drop for LocalArchiveReservation {
    fn drop(&mut self) {
        self.release();
    }
}
