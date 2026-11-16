//! Typed projections of shared CWR inspector reads; no provider event decoding.
use super::{
    messages::{datetime, DateTime},
    publications::Cursor,
    schema::{blocking, PageInfo, ReadService},
    windows::Window,
};
use async_graphql::{ComplexObject, Enum, Result, SimpleObject, ID};
use cwr::v2026_09::conversation::{
    ConversationInspectorCollection as Collection, ConversationInspectorEntry as Entry,
    ConversationInspectorKey as Key, ConversationInspectorPage as Page,
    ConversationInspectorRequest as Request, ConversationInspectorTextField as TextField,
};

#[derive(Clone, Copy, PartialEq, Eq, Enum)]
pub(super) enum InspectorCoverage {
    Complete,
    Partial,
}
#[derive(Clone, SimpleObject)]
pub(super) struct TextPreview {
    text: String,
    truncated: bool,
}
macro_rules! connection {
    ($connection:ident,$edge:ident,$node:ty) => {
        #[derive(SimpleObject)]
        pub(super) struct $connection {
            edges: Vec<$edge>,
            page_info: PageInfo,
            coverage: InspectorCoverage,
        }
        #[derive(SimpleObject)]
        struct $edge {
            cursor: Cursor,
            node: $node,
        }
    };
}
connection!(WorkActivityConnection, WorkActivityEdge, WorkActivity);
connection!(InvocationCallConnection, InvocationCallEdge, InvocationCall);
connection!(InvocationWaitConnection, InvocationWaitEdge, InvocationWait);
connection!(FileChangeConnection, FileChangeEdge, FileChange);

fn scope(service: &ReadService, invocation: &str, kind: &str) -> Result<String> {
    Ok(serde_json::to_string(&(
        &service.actor_ref,
        &service.owner,
        invocation,
        "inspector-v1",
        kind,
    ))?)
}
fn request(window: Window, scope: &str) -> Result<Request> {
    let decode = |cursor: Option<Cursor>| -> Result<Option<Key>> {
        cursor
            .map(|c| {
                if c.0.len() > 16 * 1024 {
                    return Err("Inspector cursor too large".into());
                }
                let (actual, key): (String, Key) = serde_json::from_str(&c.0)?;
                if actual != scope {
                    return Err("Invalid inspector cursor scope".into());
                }
                Ok(key)
            })
            .transpose()
    };
    let size = |n: Option<i32>| -> Result<Option<usize>> {
        n.map(|n| usize::try_from(n).map_err(Into::into))
            .transpose()
    };
    Ok(Request {
        first: size(window.first)?,
        last: size(window.last)?,
        after: decode(window.after)?,
        before: decode(window.before)?,
    })
}
fn cursor(scope: &str, key: &Key) -> Result<Cursor> {
    Ok(Cursor(serde_json::to_string(&(scope, key))?))
}
fn info(page: &Page, scope: &str) -> Result<PageInfo> {
    Ok(PageInfo::new(
        page.has_next_page,
        page.has_previous_page,
        page.entries
            .first()
            .map(|(k, _)| cursor(scope, k).map(|c| c.0))
            .transpose()?,
        page.entries
            .last()
            .map(|(k, _)| cursor(scope, k).map(|c| c.0))
            .transpose()?,
    ))
}
fn coverage(page: &Page) -> InspectorCoverage {
    if page.complete {
        InspectorCoverage::Complete
    } else {
        InspectorCoverage::Partial
    }
}
async fn load(
    service: ReadService,
    invocation: String,
    collection: Collection,
    window: Window,
) -> Result<Option<(String, Page)>> {
    let scope = scope(&service, &invocation, &format!("{collection:?}"))?;
    let request = request(window, &scope)?;
    blocking(service.reads.clone(), service.budget(), move || {
        Ok(service
            .publications
            .read(&service, |v| {
                v.inspector_page(&invocation, collection, &request)
            })?
            .map(|page| (scope, page)))
    })
    .await
}
pub(super) async fn activities(
    service: ReadService,
    invocation: String,
    window: Window,
) -> Result<Option<WorkActivityConnection>> {
    let Some((scope, page)) = load(
        service.clone(),
        invocation.clone(),
        Collection::Activities,
        window,
    )
    .await?
    else {
        return Ok(None);
    };
    let page_info = info(&page, &scope)?;
    let coverage = coverage(&page);
    let edges = page
        .entries
        .into_iter()
        .map(|(key, entry)| {
            let Entry::Activity {
                activity_ref,
                activity_kind,
                label,
                label_truncated,
                state,
                active,
                started_at,
                duration_ms,
                child_invocation_ref,
                details_available,
            } = entry
            else {
                return Err("Invalid activity projection".into());
            };
            let finished_at = started_at
                .zip(duration_ms)
                .filter(|_| state != "running")
                .map(|(start, duration)| {
                    start.checked_add(duration).ok_or("Activity time overflow")
                })
                .transpose()?
                .map(|ms| datetime(i128::from(ms)))
                .transpose()?;
            Ok(WorkActivityEdge {
                cursor: cursor(&scope, &key)?,
                node: WorkActivity {
                    service: service.clone(),
                    invocation: invocation.clone(),
                    id: ID(activity_ref),
                    kind: activity_kind,
                    label,
                    label_truncated,
                    state,
                    active,
                    started_at: started_at.map(|ms| datetime(i128::from(ms))).transpose()?,
                    finished_at,
                    child_ref: child_invocation_ref,
                    details_available,
                },
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(Some(WorkActivityConnection {
        edges,
        page_info,
        coverage,
    }))
}
pub(super) async fn calls(
    service: ReadService,
    invocation: String,
    window: Window,
) -> Result<Option<InvocationCallConnection>> {
    let Some((scope, page)) = load(
        service.clone(),
        invocation.clone(),
        Collection::Calls,
        window,
    )
    .await?
    else {
        return Ok(None);
    };
    let page_info = info(&page, &scope)?;
    let coverage = coverage(&page);
    let edges = page
        .entries
        .into_iter()
        .map(|(key, entry)| {
            let Entry::Call {
                call_id,
                callee_invocation_ref,
            } = entry
            else {
                return Err("Invalid call projection".into());
            };
            Ok(InvocationCallEdge {
                cursor: cursor(&scope, &key)?,
                node: InvocationCall {
                    service: service.clone(),
                    caller_ref: invocation.clone(),
                    callee_ref: callee_invocation_ref,
                    call_id,
                },
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(Some(InvocationCallConnection {
        edges,
        page_info,
        coverage,
    }))
}
pub(super) async fn waits(
    service: ReadService,
    invocation: String,
    window: Window,
) -> Result<Option<InvocationWaitConnection>> {
    let Some((scope, page)) = load(
        service.clone(),
        invocation.clone(),
        Collection::Waits,
        window,
    )
    .await?
    else {
        return Ok(None);
    };
    let page_info = info(&page, &scope)?;
    let coverage = coverage(&page);
    let edges = page
        .entries
        .into_iter()
        .map(|(key, entry)| {
            let Entry::Wait {
                call_id,
                awaited_invocation_ref,
                state,
            } = entry
            else {
                return Err("Invalid wait projection".into());
            };
            Ok(InvocationWaitEdge {
                cursor: cursor(&scope, &key)?,
                node: InvocationWait {
                    service: service.clone(),
                    waiter_ref: invocation.clone(),
                    awaited_ref: awaited_invocation_ref,
                    call_id,
                    state: state.into(),
                },
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(Some(InvocationWaitConnection {
        edges,
        page_info,
        coverage,
    }))
}

#[derive(SimpleObject)]
#[graphql(complex)]
pub(super) struct WorkActivity {
    #[graphql(skip)]
    service: ReadService,
    #[graphql(skip)]
    invocation: String,
    #[graphql(skip)]
    child_ref: Option<String>,
    id: ID,
    kind: String,
    label: String,
    label_truncated: bool,
    state: String,
    active: bool,
    started_at: Option<DateTime>,
    finished_at: Option<DateTime>,
    details_available: bool,
}
#[ComplexObject]
impl WorkActivity {
    async fn child_invocation(&self) -> Result<Option<super::invocations::ActionInvocation>> {
        super::messages::invocation(&self.service, self.child_ref.as_deref()).await
    }
    async fn command(&self) -> Result<Option<TextPreview>> {
        self.text(TextField::Command).await
    }
    async fn output(&self) -> Result<Option<TextPreview>> {
        self.text(TextField::Output).await
    }
    async fn error(&self) -> Result<Option<TextPreview>> {
        self.text(TextField::Error).await
    }
    async fn exit_code(&self) -> Result<Option<i32>> {
        let service = self.service.clone();
        let invocation = self.invocation.clone();
        let activity = self.id.to_string();
        blocking(service.reads.clone(), service.budget(), move || {
            service
                .publications
                .read(&service, |v| v.inspector_exit_code(&invocation, &activity))?
                .map(i32::try_from)
                .transpose()
                .map_err(Into::into)
        })
        .await
    }
    async fn files(
        &self,
        first: Option<i32>,
        after: Option<Cursor>,
        last: Option<i32>,
        before: Option<Cursor>,
    ) -> Result<Option<FileChangeConnection>> {
        files(
            self.service.clone(),
            self.invocation.clone(),
            self.id.to_string(),
            Window {
                first,
                after,
                last,
                before,
            },
        )
        .await
    }
}
impl WorkActivity {
    async fn text(&self, field: TextField) -> Result<Option<TextPreview>> {
        text(
            self.service.clone(),
            self.invocation.clone(),
            self.id.to_string(),
            None,
            field,
        )
        .await
    }
}
async fn text(
    service: ReadService,
    invocation: String,
    activity: String,
    file: Option<Key>,
    field: TextField,
) -> Result<Option<TextPreview>> {
    blocking(service.reads.clone(), service.budget(), move || {
        Ok(service
            .publications
            .read(&service, |v| {
                v.inspector_text(&invocation, &activity, file.as_ref(), field)
            })?
            .map(|p| TextPreview {
                text: p.text,
                truncated: p.truncated,
            }))
    })
    .await
}

include!("inspector/relationships.rs");
