impl ConversationAdmissionStore {
    pub(crate) fn record_source_input_fingerprint(
        &mut self,
        integration_ref: &str,
        provider_event_ref: &str,
        source_input_fingerprint: &str,
    ) -> CwrResult<ConversationAdmissionRecord> {
        self.transact(|staged| {
            let record = staged
                .admissions
                .find_mut(|record| {
                    record.integration_ref == integration_ref
                        && record.provider_event_ref == provider_event_ref
                })
                .ok_or_else(|| {
                    invalid_error(format!(
                        "conversation ingress event {provider_event_ref} has no retained admission"
                    ))
                })?;
            match record.source_input_fingerprint.as_deref() {
                None => {
                    record.source_input_fingerprint = Some(source_input_fingerprint.to_owned());
                }
                Some(retained) if retained == source_input_fingerprint => {}
                Some(_) => {
                    return invalid(format!(
                        "conversation ingress event {provider_event_ref} was reused with different source input"
                    ));
                }
            }
            Ok(record.clone())
        })
    }
}
