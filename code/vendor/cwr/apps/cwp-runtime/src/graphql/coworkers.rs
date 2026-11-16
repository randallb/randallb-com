use super::{
    messages::Coworker,
    schema::{blocking, ReadService},
};
use async_graphql::{ComplexObject, Context, Enum, Result, SimpleObject, ID};
use cwr::v2026_09::local::{LocalCoworkerBinding, LocalCoworkerConfiguration};

pub(super) type ConfigurationSnapshot = std::sync::Arc<
    tokio::sync::OnceCell<std::collections::BTreeMap<String, LocalCoworkerConfiguration>>,
>;

#[derive(Clone, Copy, Eq, PartialEq, Enum)]
enum CoworkerCategory {
    Human,
    Ai,
    Software,
}

#[derive(Clone, SimpleObject)]
struct Adapter {
    id: ID,
    name: String,
}
#[derive(Clone, SimpleObject)]
struct ModelConfiguration {
    provider: Option<String>,
    model: Option<String>,
    reasoning_effort: Option<String>,
}
#[derive(Clone, SimpleObject)]
struct CoworkerBinding {
    id: ID,
    adapter: Adapter,
    model: Option<ModelConfiguration>,
}
impl From<LocalCoworkerBinding> for CoworkerBinding {
    fn from(value: LocalCoworkerBinding) -> Self {
        Self {
            id: ID(value.binding_ref),
            adapter: Adapter {
                id: ID(value.adapter_ref),
                name: value.adapter_name,
            },
            model: value.model.map(|value| ModelConfiguration {
                provider: value.provider,
                model: value.model,
                reasoning_effort: value.reasoning_effort,
            }),
        }
    }
}
async fn configuration(
    coworker: &Coworker,
    ctx: &Context<'_>,
) -> Result<Option<LocalCoworkerConfiguration>> {
    read_configuration(coworker, ctx.data_unchecked::<ReadService>().clone()).await
}
pub(super) async fn read_configuration(
    coworker: &Coworker,
    service: ReadService,
) -> Result<Option<LocalCoworkerConfiguration>> {
    let target = coworker.id.to_string();
    coworker
        .configuration
        .get_or_try_init(|| async {
            let snapshot = service.configurations.clone();
            let values = snapshot
                .get_or_try_init(|| async {
                    blocking(service.reads.clone(), service.budget(), move || {
                        let budget = service.budget();
                        service.runtime.coworker_configurations_as_until(
                            &service.actor_ref,
                            service.owner.as_deref(),
                            budget.deadline(),
                            || budget.cancelled(),
                        )
                    })
                    .await
                })
                .await?;
            Ok::<_, async_graphql::Error>(values.get(&target).cloned())
        })
        .await
        .cloned()
}
#[ComplexObject]
impl Coworker {
    /// Category of the current default adapter; unknown or historical-only identities return null.
    async fn category(&self, ctx: &Context<'_>) -> Result<Option<CoworkerCategory>> {
        use cwr::v2026_09::graph::CoworkerCategory as Category;
        Ok(configuration(self, ctx)
            .await?
            .and_then(|value| value.category)
            .map(|value| match value {
                Category::Human => CoworkerCategory::Human,
                Category::Ai => CoworkerCategory::Ai,
                Category::Software => CoworkerCategory::Software,
            }))
    }
    /// Current configured bindings visible to this session. Does not expose adapter parameters.
    async fn bindings(&self, ctx: &Context<'_>) -> Result<Vec<CoworkerBinding>> {
        Ok(configuration(self, ctx)
            .await?
            .map(|value| value.bindings.into_iter().map(Into::into).collect())
            .unwrap_or_default())
    }
    /// Declared default, including when unavailable. Never substitutes another binding.
    async fn default_binding(&self, ctx: &Context<'_>) -> Result<Option<CoworkerBinding>> {
        Ok(configuration(self, ctx)
            .await?
            .and_then(|value| {
                value.bindings.into_iter().find(|binding| {
                    Some(&binding.binding_ref) == value.default_binding_ref.as_ref()
                })
            })
            .map(Into::into))
    }
}
