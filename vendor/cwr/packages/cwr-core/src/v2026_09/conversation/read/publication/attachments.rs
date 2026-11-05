//! Visualization content is available only through a visible published message.
use super::*;
#[derive(Debug, Clone)]
pub struct ConversationAttachmentPresentation {
    pub artifact_ref: String,
    pub title: Option<String>,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationAttachmentFilter {
    pub search: Option<String>,
    pub media_types: Vec<String>,
}
impl ConversationAttachmentFilter {
    pub fn normalized(mut self) -> CwrResult<Self> {
        self.search = self
            .search
            .map(|s| s.trim().to_lowercase())
            .filter(|s| !s.is_empty());
        if self.search.as_ref().is_some_and(|s| s.len() > 1024) {
            return invalid("Attachment search exceeds 1024 bytes");
        }
        if self.media_types.iter().any(|m| m == "text/html") {
            self.media_types.clear();
        } else if !self.media_types.is_empty() {
            self.media_types = vec!["unsupported".into()];
        }
        Ok(self)
    }
}
impl ConversationPublicationSnapshot {
    pub fn filtered_attachments(
        &self,
        message: &str,
        filter: &ConversationAttachmentFilter,
    ) -> CwrResult<Option<Vec<(usize, ConversationAttachmentPresentation)>>> {
        let filter = filter.clone().normalized()?;
        Ok(self.attachments(message)?.map(|values| {
            values
                .into_iter()
                .enumerate()
                .filter(|(_, v)| {
                    filter.media_types.is_empty()
                        && filter.search.as_ref().is_none_or(|s| {
                            v.title
                                .as_ref()
                                .is_some_and(|t| t.to_lowercase().contains(s))
                        })
                })
                .collect()
        }))
    }
    pub fn attachments(
        &self,
        message_ref: &str,
    ) -> CwrResult<Option<Vec<ConversationAttachmentPresentation>>> {
        self.read(|op| {
            let Some(value) = self.resource_in(op, message_ref)? else {
                return Ok(None);
            };
            let ConversationResource::Message { message, .. } = value.resource else {
                return Ok(None);
            };
            let mut attachments = Vec::new();
            for visualization in message.visualizations {
                op.ensure_active()?;
                let Some(reference) = visualization.artifact_ref else {
                    continue;
                };
                let exists = op.finite_snapshot()?.artifact_exists(&reference)?;
                if exists {
                    attachments.push(ConversationAttachmentPresentation {
                        artifact_ref: reference,
                        title: visualization.title,
                    });
                }
            }
            Ok(Some(attachments))
        })
    }
    pub fn attachment(
        &self,
        reference: &str,
    ) -> CwrResult<Option<ConversationAttachmentPresentation>> {
        Ok(self
            .attachment_detail(reference, false)?
            .map(|(value, _)| value))
    }
    pub fn attachment_html(&self, reference: &str) -> CwrResult<Option<String>> {
        Ok(self
            .attachment_detail(reference, true)?
            .and_then(|(_, html)| html))
    }
    fn attachment_detail(
        &self,
        reference: &str,
        content: bool,
    ) -> CwrResult<Option<(ConversationAttachmentPresentation, Option<String>)>> {
        self.read(|op| {
            let exists = op.finite_snapshot()?.artifact_exists(reference)?;
            if !exists {
                return Ok(None);
            }
            for message_ref in op.finite_snapshot()?.artifact_messages(reference)? {
                op.ensure_active()?;
                let Some(value) = self.resource_in(op, &message_ref)? else {
                    continue;
                };
                let ConversationResource::Message { message, .. } = value.resource else {
                    continue;
                };
                let Some(visualization) = message
                    .visualizations
                    .into_iter()
                    .find(|v| v.artifact_ref.as_deref() == Some(reference))
                else {
                    continue;
                };
                let descriptor = ConversationAttachmentPresentation {
                    artifact_ref: reference.into(),
                    title: visualization.title,
                };
                if !content {
                    let exists = op.finite_snapshot()?.artifact_exists(reference)?;
                    if exists {
                        return Ok(Some((descriptor, None)));
                    } else {
                        continue;
                    }
                }
                let html = op.finite_snapshot()?.artifact_html(reference)?;
                if let Some(html) = html {
                    return Ok(Some((descriptor, Some(html))));
                }
            }
            Ok(None)
        })
    }
}
