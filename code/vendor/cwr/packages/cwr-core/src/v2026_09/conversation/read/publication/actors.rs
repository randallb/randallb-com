use super::metadata::ConversationActorPresentation;
use super::*;

fn load_actor_record(
    op: &ConversationReadOperation<'_>,
    actor: &str,
) -> CwrResult<ConversationActorPresentation> {
    let identity = op
        .finite_snapshot()?
        .actor_identity(actor)?
        .map(serde_json::from_value::<crate::v2026_09::local::LocalActorIdentity>)
        .transpose()?;
    Ok(ConversationActorPresentation {
        actor_ref: actor.into(),
        name: identity
            .as_ref()
            .map(|a| a.name.clone())
            .filter(|s| !s.is_empty()),
        handle: identity.as_ref().map(|a| a.handle.clone()),
        coworker: identity.is_some(),
    })
}
impl ConversationPublicationSnapshot {
    fn actor_record(
        &self,
        op: &ConversationReadOperation<'_>,
        actor: &str,
    ) -> CwrResult<ConversationActorPresentation> {
        self.cache
            .actors
            .get_or_load(actor, || load_actor_record(op, actor))
    }
    /// Enrich only an already authorized identity relationship, from retained evidence.
    pub fn related_actor(
        &self,
        resource: &str,
        actor: &str,
    ) -> CwrResult<Option<ConversationActorPresentation>> {
        self.read(|op| {
            let Some(value) = self.resource_in(op, resource)? else {
                return Ok(None);
            };
            let related = match value.resource {
                ConversationResource::Message { message, .. } => {
                    message.author_actor_ref == actor
                        || message.addressed_actor_refs.iter().any(|a| a == actor)
                }
                ConversationResource::Invocation { invocation } => {
                    invocation.requester_actor_ref == actor || invocation.target_actor_ref == actor
                }
            };
            if !related {
                return Ok(None);
            }
            self.actor_record(op, actor).map(Some)
        })
    }
}
impl ConversationPublicationSnapshot {
    pub fn actor(&self, actor: &str) -> CwrResult<Option<ConversationActorPresentation>> {
        self.read(|op| {
            if actor == self.actor {
                return self.actor_record(op, actor).map(Some);
            }
            let candidate = op.finite_snapshot()?.actor_candidate(actor)?;
            if !candidate {
                return Ok(None);
            }
            for conversation in op.finite_snapshot()?.actor_conversations(actor)? {
                if self.visible(op, &conversation)? {
                    return self.actor_record(op, actor).map(Some);
                }
            }
            for resource in op.finite_snapshot()?.actor_resources(actor)? {
                if self.resource_in(op, &resource)?.is_some() {
                    return self.actor_record(op, actor).map(Some);
                }
            }
            Ok(None)
        })
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationActorFilter {
    pub search: Option<String>,
    pub refs: Vec<String>,
}
impl ConversationActorFilter {
    pub fn normalized(mut self) -> CwrResult<Self> {
        self.search = self
            .search
            .map(|s| s.trim().to_lowercase())
            .filter(|s| !s.is_empty());
        if self.search.as_ref().is_some_and(|s| s.len() > 1024) {
            return invalid("Actor search exceeds 1024 bytes");
        }
        self.refs.sort();
        self.refs.dedup();
        Ok(self)
    }
    fn matches(&self, value: &ConversationActorPresentation) -> bool {
        (self.refs.is_empty() || self.refs.contains(&value.actor_ref))
            && self.search.as_ref().is_none_or(|s| {
                value
                    .name
                    .iter()
                    .chain(&value.handle)
                    .any(|t| t.to_lowercase().contains(s))
            })
    }
}
impl ConversationPublicationSnapshot {
    pub fn message_recipients(
        &self,
        reference: &str,
        filter: &ConversationActorFilter,
    ) -> CwrResult<Option<Vec<(usize, ConversationActorPresentation)>>> {
        let filter = filter.clone().normalized()?;
        self.read(|op| {
            let Some(value) = self.resource_in(op, reference)? else {
                return Ok(None);
            };
            let ConversationResource::Message { message, .. } = value.resource else {
                return Ok(None);
            };
            let mut values = Vec::new();
            for (index, reference) in message.addressed_actor_refs.iter().enumerate() {
                op.ensure_active()?;
                let actor = self.actor_record(op, reference)?;
                if filter.matches(&actor) {
                    values.push((index, actor));
                }
            }
            Ok(Some(values))
        })
    }
}
