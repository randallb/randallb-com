//! Product-context availability intersects existing session visibility, never authority.
use super::{
    messages::Coworker,
    schema::{blocking, ReadService},
};
use async_graphql::{Result, ID};

pub(super) async fn message_targets(
    service: ReadService,
    context: Option<ID>,
) -> Result<Vec<Coworker>> {
    blocking(service.reads.clone(), service.budget(), move || {
        let budget = service.budget();
        let allowed = service
            .runtime
            .conversation_context_targets(context.as_ref().map(|id| id.as_str()))?;
        Ok(service
            .runtime
            .upstream_message_targets_as_until(
                &service.actor_ref,
                service.owner.as_deref(),
                budget.deadline(),
                || budget.cancelled(),
            )?
            .into_iter()
            .filter(|actor| {
                allowed
                    .as_ref()
                    .is_none_or(|allowed| allowed.contains(&actor.actor_ref))
            })
            .map(Coworker::from)
            .collect())
    })
    .await
}
pub(super) async fn default_message_target(
    service: ReadService,
    context: Option<ID>,
) -> Result<Option<Coworker>> {
    let targets = message_targets(service.clone(), context.clone()).await?;
    blocking(service.reads.clone(), service.budget(), move || {
        let allowed = service
            .runtime
            .conversation_context_targets(context.as_ref().map(|id| id.as_str()))?;
        let selected = match allowed {
            Some(allowed) => allowed.into_iter().find_map(|id| {
                targets
                    .iter()
                    .find(|actor| actor.id.as_str() == id)
                    .cloned()
            }),
            None => {
                let default = service.catalog()?.default_conversation_target_ref;
                targets
                    .into_iter()
                    .find(|actor| Some(actor.id.to_string()) == default)
            }
        };
        Ok(selected)
    })
    .await
}
