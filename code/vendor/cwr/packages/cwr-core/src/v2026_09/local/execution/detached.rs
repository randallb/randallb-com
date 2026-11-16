impl LocalRuntime {
    fn execute_independent_descendants(
        &self,
        root_invocation_ref: &str,
        model_output: &AdapterModelOutputMailbox,
        observer: &mut dyn FnMut(LocalRuntimeEvent),
    ) -> CwrResult<()> {
        loop {
            let refs = {
                let state = self.lock()?;
                if state.shutting_down {
                    return Ok(());
                }
                let refs = state
                    .admission
                    .invocation_records()
                    .filter(|candidate| {
                        candidate.is_runnable()
                            && drain_allows(&state, &candidate.invocation_ref)
                            && require_no_stop_fence(&state, &candidate.invocation_ref).is_ok()
                            && independently_scheduled(&state.admission, candidate)
                            && is_attached_descendant_or_self(
                                &state.admission,
                                candidate,
                                root_invocation_ref,
                            )
                    })
                    .map(|candidate| candidate.invocation_ref.clone())
                    .collect();
                order_runnable_refs(&state.admission, refs)
            };
            if refs.is_empty() {
                return Ok(());
            }
            for (execution_group_ref, group_refs) in self.group_refs_by_execution(&refs)? {
                let first_ref = group_refs.first();
                let _execution = self.execution.acquire(&execution_group_ref, || {
                    first_ref.map_or(Ok(true), |invocation_ref| {
                        self.invocation_is_next_in_group(invocation_ref)
                    })
                })?;
                let refs = {
                    let mut state = self.lock()?;
                    take_ordered_runnable_refs_for_group(&mut state, &execution_group_ref)
                };
                self.execute_refs(&execution_group_ref, refs, model_output, observer)?;
            }
        }
    }
}
