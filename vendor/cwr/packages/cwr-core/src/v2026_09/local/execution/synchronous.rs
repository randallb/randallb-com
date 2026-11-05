impl LocalRuntime {
    fn execute_until_terminal(
        &self,
        execution_group_ref: &str,
        target_invocation_ref: &str,
        queue: &Rc<RefCell<VecDeque<String>>>,
        model_output: &AdapterModelOutputMailbox,
        observer: &mut dyn FnMut(LocalRuntimeEvent),
        deadline_failure: Option<&Arc<Mutex<Option<String>>>>,
    ) -> CwrResult<ActionResponse> {
        loop {
            if let Some(error) = deadline_failure
                .and_then(|failure| failure.lock().unwrap().clone())
            {
                return invalid(error);
            }
            if let Some(response) = self
                .lock()?
                .admission
                .invocation_record(target_invocation_ref)
                .and_then(ActionInvocationRecord::terminal_response)
                .cloned()
            {
                return Ok(response);
            }
            let next = {
                let state = self.lock()?;
                take_next_synchronous_invocation(
                    &state.admission,
                    &mut queue.borrow_mut(),
                    target_invocation_ref,
                )
            };
            let Some(invocation_ref) = next else {
                return invalid(format!(
                    "invoked action {target_invocation_ref} cannot make progress"
                ));
            };
            if let Some(error) = deadline_failure
                .and_then(|failure| failure.lock().unwrap().clone())
            {
                return invalid(error);
            }
            self.execute_one(
                &invocation_ref,
                Rc::clone(queue),
                model_output,
                observer,
            )?;
            let pending = {
                let mut state = self.lock()?;
                take_ordered_pending_refs_for_group(&mut state, execution_group_ref)
            };
            let mut queue = queue.borrow_mut();
            for invocation_ref in pending.into_iter().rev() {
                queue.push_front(invocation_ref);
            }
        }
    }
}

fn take_next_synchronous_invocation(
    admission: &ConversationAdmissionStore,
    queue: &mut VecDeque<String>,
    target_invocation_ref: &str,
) -> Option<String> {
    let position = queue.iter().position(|invocation_ref| {
        admission
            .invocation_record(invocation_ref)
            .is_some_and(|candidate| {
                candidate.is_runnable()
                    && is_attached_descendant_or_self(
                        admission,
                        candidate,
                        target_invocation_ref,
                    )
            })
    });
    position
        .and_then(|position| queue.remove(position))
        .or_else(|| {
            admission
                .invocation_record(target_invocation_ref)
                .is_some_and(ActionInvocationRecord::is_runnable)
                .then(|| target_invocation_ref.to_owned())
        })
}
