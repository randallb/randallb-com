impl ResolvedCwrGraph {
    /// Loaded coworkers can be requested within this configured runtime. Operation
    /// grants are checked by dispatch, independently of participation.
    pub fn may_admit_actor(&self, caller: &str, target: &str) -> bool {
        self.actors.iter().any(|actor| actor.actor_ref == caller)
            && self.actors.iter().any(|actor| actor.actor_ref == target)
    }
}

impl ConversationAdmissionStore {
    pub fn prepare_conversation_archive(
        &self,
        conversation_ref: &str,
        caller: &str,
        archived: bool,
    ) -> CwrResult<ConversationArchiveMutation> {
        let mutation = ConversationArchiveMutation {
            conversation_ref: conversation_ref.to_owned(),
            caller_actor_ref: caller.to_owned(),
            archived,
        };
        validate_conversation_archive(&self.state, &mutation)?;
        Ok(mutation)
    }

    pub fn set_conversation_archived(
        &mut self,
        conversation_ref: &str,
        caller: &str,
        archived: bool,
    ) -> CwrResult<RetainedConversation> {
        let mutation = self.prepare_conversation_archive(conversation_ref, caller, archived)?;
        self.transact(|state| {
            stage_conversation_archive(state, &mutation)?;
            Ok(())
        })?;
        self.committed_conversation(conversation_ref)
    }

    pub fn prepare_conversation_rename(
        &self,
        conversation_ref: &str,
        caller: &str,
        title: &str,
    ) -> CwrResult<ConversationRenameMutation> {
        let rename = ConversationRenameMutation {
            conversation_ref: conversation_ref.to_owned(),
            caller_actor_ref: caller.to_owned(),
            title: normalize_conversation_title(title)?,
        };
        validate_conversation_rename(&self.state, &rename)?;
        Ok(rename)
    }

    pub fn rename_conversation(
        &mut self,
        conversation_ref: &str,
        caller: &str,
        title: &str,
    ) -> CwrResult<RetainedConversation> {
        let rename = self.prepare_conversation_rename(conversation_ref, caller, title)?;
        self.transact(|state| {
            stage_conversation_rename(state, &rename)?;
            Ok(())
        })?;
        self.committed_conversation(conversation_ref)
    }

    fn committed_conversation(&self, conversation_ref: &str) -> CwrResult<RetainedConversation> {
        self.conversation(conversation_ref)
            .cloned()
            .ok_or_else(|| invalid_error("committed conversation disappeared"))
    }

    /// Explicit audience management for hosts. Ordinary requests do not require it.
    pub fn invite_actor(
        &mut self,
        graph: &ResolvedCwrGraph,
        conversation_ref: &str,
        caller: &str,
        target: &str,
    ) -> CwrResult<bool> {
        if !graph.may_admit_actor(caller, target) {
            return invalid("audience selection names an unavailable coworker");
        }
        self.transact(|staged| {
            let conversation = staged
                .conversations
                .find_mut(|c| c.conversation_ref == conversation_ref)
                .ok_or_else(|| invalid_error("audience conversation is unavailable"))?;
            if !conversation
                .participant_actor_refs
                .iter()
                .any(|r| r == caller)
            {
                return invalid("audience selection caller is not a participant");
            }
            if conversation
                .participant_actor_refs
                .iter()
                .any(|r| r == target)
            {
                return Ok(false);
            }
            push_unique(&mut conversation.participant_actor_refs, target);
            Ok(true)
        })
    }

    fn prepare_participants(
        &self,
        graph: &ResolvedCwrGraph,
        conversation: &mut RetainedConversation,
        ingress: &ConversationIngress,
        target_actor_ref: Option<&str>,
    ) -> CwrResult<()> {
        if matches!(
            ingress.conversation,
            ConversationIngressConversation::Continue { .. }
        ) && !conversation
            .participant_actor_refs
            .contains(&ingress.author_actor_ref)
        {
            return invalid(format!(
                "{} is not a conversation participant",
                ingress.author_actor_ref
            ));
        }
        push_unique(
            &mut conversation.participant_actor_refs,
            &ingress.author_actor_ref,
        );
        if let Some(target) = target_actor_ref {
            let gateway_handler = graph.gateways.iter().any(|gateway| {
                [
                    Some(&gateway.route),
                    gateway.ingress_route.as_ref(),
                    gateway.delivery_route.as_ref(),
                ]
                .into_iter()
                .flatten()
                .any(|route| route.actor_ref == target)
            });
            if gateway_handler {
                return Ok(());
            }
            if !graph.may_admit_actor(&ingress.author_actor_ref, target) {
                return invalid(format!(
                    "coworker {target} is not available in this runtime"
                ));
            }
            push_unique(&mut conversation.participant_actor_refs, target);
        }
        Ok(())
    }
}

fn validate_conversation_archive(
    state: &ConversationRuntimeState,
    mutation: &ConversationArchiveMutation,
) -> CwrResult<()> {
    let conversation = state
        .conversations
        .iter()
        .find(|conversation| conversation.conversation_ref == mutation.conversation_ref)
        .ok_or_else(|| {
            invalid_error(format!(
                "unknown conversation ref {}",
                mutation.conversation_ref
            ))
        })?;
    if !conversation
        .participant_actor_refs
        .iter()
        .any(|participant| participant == &mutation.caller_actor_ref)
    {
        return invalid("conversation archive caller is not a participant");
    }
    Ok(())
}

fn stage_conversation_archive(
    state: &mut ConversationRuntimeState,
    mutation: &ConversationArchiveMutation,
) -> CwrResult<RetainedConversation> {
    validate_conversation_archive(state, mutation)?;
    if mutation.archived {
        terminal::stage_archive_stops(state, mutation)?;
    }
    let conversation = state
        .conversations
        .find_mut(|conversation| conversation.conversation_ref == mutation.conversation_ref)
        .expect("validated conversation exists");
    conversation.archived = mutation.archived;
    Ok(conversation.clone())
}

fn stage_conversation_intrinsic_mutation(
    state: &mut ConversationRuntimeState,
    mutation: &ConversationIntrinsicMutation,
) -> CwrResult<RetainedConversation> {
    match mutation {
        ConversationIntrinsicMutation::Rename(rename) => stage_conversation_rename(state, rename),
        ConversationIntrinsicMutation::Archive(archive) => {
            stage_conversation_archive(state, archive)
        }
    }
}

fn validate_conversation_rename(
    state: &ConversationRuntimeState,
    rename: &ConversationRenameMutation,
) -> CwrResult<()> {
    let conversation = state
        .conversations
        .iter()
        .find(|conversation| conversation.conversation_ref == rename.conversation_ref)
        .ok_or_else(|| {
            invalid_error(format!(
                "unknown conversation ref {}",
                rename.conversation_ref
            ))
        })?;
    if !conversation
        .participant_actor_refs
        .iter()
        .any(|participant| participant == &rename.caller_actor_ref)
    {
        return invalid("conversation rename caller is not a participant");
    }
    Ok(())
}

fn stage_conversation_rename(
    state: &mut ConversationRuntimeState,
    rename: &ConversationRenameMutation,
) -> CwrResult<RetainedConversation> {
    validate_conversation_rename(state, rename)?;
    let conversation = state
        .conversations
        .find_mut(|conversation| conversation.conversation_ref == rename.conversation_ref)
        .expect("validated conversation exists");
    conversation.title = Some(rename.title.clone());
    Ok(conversation.clone())
}

impl ConversationAdmissionStore {
    /// Resolve collaboration from the admitted snapshot, unaffected by graph reloads.
    pub fn working_conversation_ref(
        &self,
        record: &crate::v2026_09::invocation::ActionInvocationRecord,
    ) -> CwrResult<&str> {
        let execution = self
            .execution_conversation(&record.invocation_ref)
            .ok_or_else(|| invalid_error("invocation has no execution conversation"))?;
        let own = execution
            .activity
            .iter()
            .find_map(|activity| match &activity.event {
                ConversationActivityEvent::TurnStarted {
                    invocation: Some(snapshot),
                    ..
                } => Some(
                    snapshot.get("execution_scope").and_then(Value::as_str)
                        == Some("own_conversation"),
                ),
                _ => None,
            })
            .unwrap_or(false);
        if own {
            Ok(&execution.conversation_ref)
        } else {
            self.invocation_record(&record.invocation_ref)
                .and_then(|r| r.conversation_ref.as_deref())
                .ok_or_else(|| invalid_error("invocation has no requesting conversation"))
        }
    }
}

impl ConversationAdmissionStore {
    pub(crate) fn retain_upstream_declarations(
        &mut self,
        declarations: &[crate::v2026_09::upstream::UpstreamCoworker],
    ) -> CwrResult<()> {
        let mut backend=self.backend.lock().map_err(|_|invalid_error("conversation backend lock poisoned"))?;
        backend.set_expected_revision(self.commit_revision);
        let mut changes=cwpdb_store::conversation::Changes::default();
        for declaration in declarations {
            let reference=crate::cwpdb::derived_ref("upstream_declaration", &[&declaration.actor_ref,&declaration.coworker_ref]);
            let record=serde_json::to_value(declaration)?;
            if let Some(existing)=backend.get(&reference)? {
                if existing.properties["record"]!=record {return invalid("upstream declaration changed without a new coworker version");}
            } else {
                changes.records.push(cwpdb_store::conversation::WriteRecord {reference:reference.clone(),kind:cwpdb_store::conversation::RecordKind::UpstreamDeclaration,properties:json!({"record":record}),owner_ref:None});
                changes.memberships.push(cwpdb_store::conversation::MembershipChange::Upsert{source_ref:declaration.actor_ref.clone(),kind:cwpdb_store::conversation::MembershipKind::UpstreamDeclaration,member:cwpdb_store::conversation::Member{target_ref:reference,position:0,properties:json!({})}});
            }
        }
        if !changes.records.is_empty() {self.commit_revision=backend.commit_changes(changes)?;}
        Ok(())
    }
}

impl ConversationAdmissionStore {
    pub(crate) fn retained_upstream_actor(
        &self,
        conversation: &str,
        actor: &str,
    ) -> CwrResult<Option<crate::v2026_09::upstream::UpstreamCoworker>> {
        let Some(version) = self
            .state
            .upstream_participant_versions
            .get(conversation)
            .and_then(|actors| actors.get(actor))
        else {
            return Ok(None);
        };
        let reference = crate::cwpdb::derived_ref("upstream_declaration", &[actor, version]);
        let value=self.backend.lock().map_err(|_|invalid_error("conversation backend lock poisoned"))?.get(&reference)?.ok_or("retained upstream declaration is unavailable")?.properties;
        Ok(Some(serde_json::from_value(value["record"].clone())?))
    }

    pub(crate) fn retained_upstream_actor_handle(&self, actor: &str) -> CwrResult<Option<String>> {
        let mut retained_handle = None;
        let page=self.backend.lock().map_err(|_|invalid_error("conversation backend lock poisoned"))?.pages(cwpdb_store::conversation::Selection::Records{kind:cwpdb_store::conversation::RecordKind::UpstreamDeclaration})?;
        for value in page.records {
            let declaration:crate::v2026_09::upstream::UpstreamCoworker=serde_json::from_value(value.properties["record"].clone())?;
            if declaration.actor_ref!=actor {continue;}
            if retained_handle
                .as_ref()
                .is_some_and(|handle| handle != &declaration.handle)
            {
                return Ok(None);
            }
            retained_handle = Some(declaration.handle);
        }
        Ok(retained_handle)
    }
}

impl ConversationAdmissionStore {
    pub(crate) fn retain_local_identities(
        &mut self,
        actors: &[crate::v2026_09::local::LocalActorIdentity],
    ) -> CwrResult<Vec<crate::v2026_09::local::LocalActorIdentity>> {
        let mut backend=self.backend.lock().map_err(|_|invalid_error("conversation backend lock poisoned"))?;
        backend.set_expected_revision(self.commit_revision);
        let mut changes=cwpdb_store::conversation::Changes::default();
        for actor in actors {
            let reference=crate::cwpdb::derived_ref("local_actor_identity",&[&actor.actor_ref]);
            changes.records.push(cwpdb_store::conversation::WriteRecord{reference,kind:cwpdb_store::conversation::RecordKind::LocalActorIdentity,properties:json!({"record":actor}),owner_ref:None});
        }
        self.commit_revision=backend.commit_changes(changes)?;
        let page=backend.pages(cwpdb_store::conversation::Selection::Records{kind:cwpdb_store::conversation::RecordKind::LocalActorIdentity})?;
        page.records.into_iter().map(|record|Ok(serde_json::from_value(record.properties["record"].clone())?)).collect()
    }
}
