impl LocalRuntime {
    fn prepare_conversation_archive_as(
        &self,
        conversation_ref: Option<&str>,
        caller: &str,
        archived: bool,
    ) -> CwrResult<(IntrinsicCwrResponse, ConversationArchiveMutation)> {
        self.prepare_conversation_archive_in_upstream_scope_as(
            conversation_ref,
            caller,
            archived,
            None,
        )
    }

    fn prepare_conversation_archive_in_upstream_scope_as(
        &self,
        conversation_ref: Option<&str>,
        caller: &str,
        archived: bool,
        owner: Option<&str>,
    ) -> CwrResult<(IntrinsicCwrResponse, ConversationArchiveMutation)> {
        let conversation_ref =
            conversation_ref.ok_or_else(|| invalid_error("CWR invocation has no conversation"))?;
        let mutation = {
            let state = self.lock()?;
            local_catalog(&upstream_graph(&state, owner)?, caller)?;
            state
                .admission
                .prepare_conversation_archive(conversation_ref, caller, archived)?
        };
        let response = intrinsic_cwr_conversation_archived(
            mutation.conversation_ref.clone(),
            mutation.archived,
        );
        Ok((response, mutation))
    }

    pub fn set_conversation_archived_as(
        &self,
        conversation_ref: Option<&str>,
        caller: &str,
        archived: bool,
    ) -> CwrResult<IntrinsicCwrResponse> {
        self.set_conversation_archived_with_stops_as(conversation_ref, caller, archived)
            .map(|(response, _)| response)
    }

    pub fn set_conversation_archived_with_stops_as(
        &self,
        conversation_ref: Option<&str>,
        caller: &str,
        archived: bool,
    ) -> CwrResult<(IntrinsicCwrResponse, Vec<String>)> {
        self.set_conversation_archived_with_stops_in_upstream_scope_as(
            conversation_ref,
            caller,
            archived,
            None,
        )
    }

    fn set_conversation_archived_in_upstream_scope_as(
        &self,
        conversation_ref: Option<&str>,
        caller: &str,
        archived: bool,
        owner: Option<&str>,
    ) -> CwrResult<IntrinsicCwrResponse> {
        self.set_conversation_archived_with_stops_in_upstream_scope_as(
            conversation_ref,
            caller,
            archived,
            owner,
        )
        .map(|(response, _)| response)
    }

    fn set_conversation_archived_with_stops_in_upstream_scope_as(
        &self,
        conversation_ref: Option<&str>,
        caller: &str,
        archived: bool,
        owner: Option<&str>,
    ) -> CwrResult<(IntrinsicCwrResponse, Vec<String>)> {
        let conversation =
            conversation_ref.ok_or_else(|| invalid_error("CWR invocation has no conversation"))?;
        let reservation = self.reserve_conversation_archive(conversation)?;
        self.set_conversation_archived_reserved_in_upstream_scope_as(
            conversation_ref,
            caller,
            archived,
            reservation,
            owner,
        )
    }

    pub fn set_conversation_archived_reserved_as(
        &self,
        conversation_ref: Option<&str>,
        caller: &str,
        archived: bool,
        reservation: Arc<LocalArchiveReservation>,
    ) -> CwrResult<(IntrinsicCwrResponse, Vec<String>)> {
        self.set_conversation_archived_reserved_in_upstream_scope_as(
            conversation_ref,
            caller,
            archived,
            reservation,
            None,
        )
    }

    fn set_conversation_archived_reserved_in_upstream_scope_as(
        &self,
        conversation_ref: Option<&str>,
        caller: &str,
        archived: bool,
        reservation: Arc<LocalArchiveReservation>,
        owner: Option<&str>,
    ) -> CwrResult<(IntrinsicCwrResponse, Vec<String>)> {
        if conversation_ref != Some(reservation.conversation.as_str()) {
            return invalid("archive reservation belongs to a different conversation");
        }
        let (response, mutation) = self.prepare_conversation_archive_in_upstream_scope_as(
            conversation_ref,
            caller,
            archived,
            owner,
        )?;
        let stops = self.prepare_archive_stops_reserved_in_upstream_scope(
            &mutation,
            None,
            reservation,
            owner,
        )?;
        let settlement = (|| -> CwrResult<_> {
            let mut state = self.lock()?;
            local_catalog(&upstream_graph(&state, owner)?, caller)?;
            let before = stopped_archive_snapshot(&state);
            state.admission.set_conversation_archived(
                &mutation.conversation_ref,
                caller,
                archived,
            )?;
            self.sync_archive_reservations(&mut state)?;
            Ok(new_archive_stops(&state, &before))
        })();
        let stopped = match settlement {
            Ok(stopped) => stopped,
            Err(error) => {
                if let Some(fence) = &stops {
                    self.settle_prepared_archive_cancellation(fence, &mutation)?;
                }
                return Err(error);
            }
        };
        self.signal_archive_stops(&stopped)?;
        if let Some(fence) = &stops {
            fence._order.release();
        }
        drop(stops);
        Ok((response, stopped))
    }
}
