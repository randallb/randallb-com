//! Attachment connections expose canonical visualization resources.
use super::{
    publications::Cursor,
    schema::{blocking, PageInfo, ReadService},
};
use async_graphql::{InputObject, Interface, Object, Result, SimpleObject, ID};
#[derive(Clone, Default, InputObject)]
pub(super) struct AttachmentFilter {
    search: Option<String>,
    media_types: Option<Vec<String>>,
}
#[derive(Clone)]
pub(super) struct HtmlVisualization {
    service: ReadService,
    pub(super) reference: String,
    title: Option<String>,
}
#[derive(SimpleObject)]
pub(super) struct VisualizationContent {
    html: Option<String>,
    error: Option<String>,
}
#[Object]
impl HtmlVisualization {
    /// Artifact failure stays local even when content is composed into a screen.
    async fn content(&self, ctx: &async_graphql::Context<'_>) -> VisualizationContent {
        match self.html(ctx).await {
            Ok(html) => VisualizationContent {
                html: Some(html),
                error: None,
            },
            Err(_) => VisualizationContent {
                html: None,
                error: Some("This visualization artifact is unavailable.".into()),
            },
        }
    }

    pub(super) async fn id(&self) -> ID {
        ID(self.reference.clone())
    }
    async fn title(&self) -> Option<String> {
        self.title.clone()
    }
    async fn media_type(&self) -> String {
        "text/html".into()
    }
    async fn html(&self) -> Result<String> {
        let service = self.service.clone();
        let reference = self.reference.clone();
        blocking(service.reads.clone(), service.budget(), move || {
            service
                .publications
                .read(&service, |v| v.attachment_html(&reference))?
                .ok_or_else(|| std::io::Error::other("Attachment unavailable").into())
        })
        .await
    }
}
#[derive(Clone, Interface)]
#[graphql(
    field(name = "id", ty = "ID"),
    field(name = "title", ty = "Option<String>"),
    field(name = "media_type", ty = "String")
)]
pub(super) enum Attachment {
    Html(HtmlVisualization),
}
#[derive(SimpleObject)]
pub(super) struct AttachmentConnection {
    edges: Vec<AttachmentEdge>,
    page_info: PageInfo,
    total_count: i32,
}
#[derive(SimpleObject)]
struct AttachmentEdge {
    cursor: Cursor,
    node: Attachment,
}
pub(super) async fn page(
    service: ReadService,
    message: String,
    filter: Option<AttachmentFilter>,
    first: Option<i32>,
    after: Option<Cursor>,
    last: Option<i32>,
    before: Option<Cursor>,
) -> Result<AttachmentConnection> {
    let filter = normalized(filter)?;
    let scope = serde_json::to_string(&(
        &service.actor_ref,
        &service.owner,
        "attachments",
        &message,
        &filter,
    ))?;
    let window = super::windows::Window {
        first,
        after,
        last,
        before,
    };
    blocking(service.reads.clone(), service.budget(), move || {
        let values = service
            .publications
            .read(&service, |v| v.filtered_attachments(&message, &filter))?
            .ok_or_else(|| std::io::Error::other("Message unavailable"))?;
        let values = values
            .into_iter()
            .map(|(i, v)| {
                (
                    i,
                    v.artifact_ref.clone(),
                    Attachment::Html(HtmlVisualization::new(service.clone(), v)),
                )
            })
            .collect();
        let page = window
            .select(&scope, values)
            .map_err(|e| std::io::Error::other(e.message))?;
        Ok(AttachmentConnection {
            edges: page
                .values
                .into_iter()
                .map(|(cursor, node)| AttachmentEdge { cursor, node })
                .collect(),
            page_info: page.info,
            total_count: page.total,
        })
    })
    .await
}

impl HtmlVisualization {
    pub(super) fn new(
        service: ReadService,
        value: cwr::v2026_09::conversation::ConversationAttachmentPresentation,
    ) -> Self {
        Self {
            service,
            reference: value.artifact_ref,
            title: value.title,
        }
    }
}
fn normalized(
    filter: Option<AttachmentFilter>,
) -> cwr::CwrResult<cwr::v2026_09::conversation::ConversationAttachmentFilter> {
    let filter = filter.unwrap_or_default();
    cwr::v2026_09::conversation::ConversationAttachmentFilter {
        search: filter.search,
        media_types: filter.media_types.unwrap_or_default(),
    }
    .normalized()
}
pub(super) fn empty(
    filter: Option<AttachmentFilter>,
    first: Option<i32>,
    after: Option<Cursor>,
    last: Option<i32>,
    before: Option<Cursor>,
) -> Result<AttachmentConnection> {
    let filter = normalized(filter)?;
    let scope = serde_json::to_string(&("empty-attachments", filter))?;
    let page = super::windows::Window {
        first,
        after,
        last,
        before,
    }
    .select::<Attachment>(&scope, vec![])?;
    Ok(AttachmentConnection {
        edges: vec![],
        page_info: page.info,
        total_count: page.total,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn empty_connections_validate_windows_and_filters() {
        assert!(empty(None, Some(0), None, None, None).is_err());
        assert!(empty(None, Some(1), Some(Cursor("invalid".into())), None, None).is_err());
        assert!(empty(
            Some(AttachmentFilter {
                search: Some("x".repeat(1025)),
                media_types: None
            }),
            None,
            None,
            None,
            None
        )
        .is_err());
        assert!(empty(None, None, None, Some(10), None).is_ok());
    }
}

#[cfg(test)]
mod content_tests {
    use super::*;
    use async_graphql::{EmptyMutation, EmptySubscription, Schema};
    use cwr::v2026_09::local::{LocalHostIdentity, LocalRuntime};
    use std::{path::PathBuf, sync::Arc, time::Duration};

    struct TestQuery(HtmlVisualization);
    #[Object]
    impl TestQuery {
        async fn conversation_text(&self) -> &str {
            "Conversation stays readable"
        }
        async fn artifact(&self) -> HtmlVisualization {
            self.0.clone()
        }
    }

    #[tokio::test]
    async fn unavailable_artifact_content_does_not_fail_composed_screen() {
        let directory = tempfile::tempdir().unwrap();
        let runtime = LocalRuntime::open_with_state_path_and_runtime_ref(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(
                "../../packages/cwr-core/fixtures/contracts/agent-recovery-turn/workspace/CWR.toml",
            ),
            directory.path().join("cwpdb.sqlite3"),
            "cwp://example.test/runtime",
            LocalHostIdentity::new(
                crate::test_refs::actor_ref("person").as_str(),
                "cwp://example.test/integrations/graphql",
            ),
        )
        .unwrap();
        for duration in [Duration::from_secs(10), Duration::ZERO] {
            let service = ReadService {
                configurations: Default::default(),
                action_titles: Default::default(),
                runtime: runtime.clone(),
                runtime_ref: "cwp://example.test/runtime".into(),
                actor_ref: crate::test_refs::actor_ref("person"),
                owner: None,
                ingress: "cwp://example.test/integrations/graphql".into(),
                publications: Default::default(),
                reads: Arc::new(tokio::sync::Semaphore::new(8)),
                budget: Some(super::super::budget::ReadBudget::new(duration)),
                publisher: Arc::new(|_| Ok(())),
            };
            let schema = Schema::build(
                TestQuery(HtmlVisualization {
                    service,
                    reference: "cwp://example.test/visualization-artifacts/missing".into(),
                    title: None,
                }),
                EmptyMutation,
                EmptySubscription,
            )
            .finish();
            let response = schema
                .execute("{ conversationText artifact { content { html error } } }")
                .await;
            assert!(response.errors.is_empty(), "{:?}", response.errors);
            let data = response.data.into_json().unwrap();
            assert_eq!(data["conversationText"], "Conversation stays readable");
            assert!(data["artifact"]["content"]["html"].is_null());
            assert_eq!(
                data["artifact"]["content"]["error"],
                "This visualization artifact is unavailable."
            );
        }
    }
}
