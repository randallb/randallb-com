//! Cap the complete wire response of selections that request inspector data.
use async_graphql::{
    extensions::{
        Extension, ExtensionContext, ExtensionFactory, NextExecute, NextResolve, NextSubscribe,
        ResolveInfo,
    },
    Error, Response, ServerResult, Value,
};
use futures_util::{stream::BoxStream, StreamExt};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

pub(super) struct InspectorResponseBudget;
impl ExtensionFactory for InspectorResponseBudget {
    fn create(&self) -> Arc<dyn Extension> {
        Arc::new(InspectorBudget {
            used: Arc::new(AtomicBool::new(false)),
        })
    }
}
struct InspectorBudget {
    used: Arc<AtomicBool>,
}
fn limit(response: Response, used: bool) -> Response {
    if used && serde_json::to_vec(&response.data).map_or(true, |bytes| bytes.len() > 512 * 1024) {
        Response::from_errors(vec![Error::new(
            "Inspector response exceeds 512 KiB; select a smaller page or fewer fields",
        )
        .into_server_error(Default::default())])
    } else {
        response
    }
}
#[async_trait::async_trait]
impl Extension for InspectorBudget {
    async fn resolve(
        &self,
        ctx: &ExtensionContext<'_>,
        info: ResolveInfo<'_>,
        next: NextResolve<'_>,
    ) -> ServerResult<Option<Value>> {
        if info.parent_type.ends_with("Invocation")
            && matches!(info.name, "activities" | "calls" | "waits")
        {
            self.used.store(true, Ordering::Release);
        }
        next.run(ctx, info).await
    }
    async fn execute(
        &self,
        ctx: &ExtensionContext<'_>,
        operation_name: Option<&str>,
        next: NextExecute<'_>,
    ) -> Response {
        let response = next.run(ctx, operation_name).await;
        limit(response, self.used.load(Ordering::Acquire))
    }
    fn subscribe<'s>(
        &self,
        ctx: &ExtensionContext<'_>,
        stream: BoxStream<'s, Response>,
        next: NextSubscribe<'_>,
    ) -> BoxStream<'s, Response> {
        let used = self.used.clone();
        next.run(ctx, stream)
            .map(move |response| limit(response, used.load(Ordering::Acquire)))
            .boxed()
    }
}
