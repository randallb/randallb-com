impl ActionInvocationLedger {
    #[cfg(test)]
    pub(crate) fn fixture_replace(&mut self, replacement: &Self) -> CwrResult<()> {
        self.invocations.fixture_replace(&replacement.invocations);
        self.waits.fixture_replace(&replacement.waits)
    }

    pub(crate) fn retain_fork_history(&mut self, record: ActionInvocationRecord) -> CwrResult<()> {
        if record.forked_from_invocation_ref.is_none() || !record.state.is_terminal() || record.state == ActionInvocationState::Stopping || self.invocations.contains_key(&record.invocation_ref) {
            return invalid("fork history must be new and settled");
        }
        self.invocations.insert(record.invocation_ref.clone(), record);
        Ok(())
    }
    pub(crate) fn begin(&mut self) {
        self.invocations.begin();
        self.waits.begin();
    }
    pub(crate) fn commit(&mut self) {
        self.invocations.commit();
        self.waits.commit();
    }
    pub(crate) fn rollback(&mut self) {
        self.invocations.rollback();
        self.waits.rollback();
    }
    pub(crate) fn changes(
        &self,
    ) -> impl Iterator<
        Item = (
            &String,
            Option<&ActionInvocationRecord>,
            Option<&ActionInvocationRecord>,
        ),
    > {
        self.invocations.changes()
    }
    pub(crate) fn changed_waits(
        &self,
    ) -> impl Iterator<Item = (usize, Option<&InvocationWait>, &InvocationWait)> {
        self.waits.changes()
    }
    pub(crate) fn validate_changes(&self) -> CwrResult<()> {
        let mut refs = BTreeSet::new();
        for (key, old, new) in self.invocations.changes() {
            let Some(record) = new else {
                return invalid("retained invocations cannot be removed during a transaction");
            };
            refs.insert(key.clone());
            if old.is_some_and(|old| old.forked_from_invocation_ref.is_some() && old != record) {
                return invalid("forked invocation history is immutable");
            }
            if old.is_none_or(|old| old.origin != record.origin)
                && self.invocations.values().any(|other| {
                    other.invocation_ref != record.invocation_ref && other.origin == record.origin
                })
            {
                return invalid("invocation origin appears more than once");
            }
            if old.is_some_and(|old| old.admission_sequence != record.admission_sequence) {
                return invalid("retained invocation admission order cannot change");
            }
            if old.is_some_and(|old| old.parent_invocation_ref != record.parent_invocation_ref) {
                return invalid("retained invocation parent cannot change");
            }
            if old.is_some_and(|old| {
                old.source_message_ref.is_some()
                    && old.source_message_ref != record.source_message_ref
            }) {
                return invalid("retained invocation source message cannot change");
            }
            if old.is_some_and(|old| {
                old
                    .timeline_publications
                    .iter()
                    .any(|(reference, disposition)| {
                        record.timeline_publications.get(reference) != Some(disposition)
                    })
            }) {
                return invalid("retained invocation timeline publications are append-only");
            }
            if record.state == ActionInvocationState::Stopped
                && old.is_none_or(|old| old.state != record.state)
            {
                refs.extend(
                    self.invocations
                        .values()
                        .filter(|child| child.parent_invocation_ref.as_deref() == Some(key))
                        .map(|child| child.invocation_ref.clone()),
                );
            }
        }
        if refs.is_empty() && self.waits.changes().next().is_none() {
            return Ok(());
        }
        self.validate_selected(Some(&refs))
    }
}
