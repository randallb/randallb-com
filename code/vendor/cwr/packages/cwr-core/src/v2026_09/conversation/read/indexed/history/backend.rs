//! Pure CWR projection of storage-owned published-history facts.
use super::*;
use crate::v2026_09::conversation::backend::CanonicalHistoryReader;
use cwpdb_store::conversation::{
    HistoryItemFacts, HistoryReadFacts, HistorySelection, PositionCursor,
};
fn cursor(key: &ConversationPositionKey) -> PositionCursor {
    PositionCursor {
        position: key.position,
        revision: key.revision.clone(),
    }
}
fn decode(
    item: HistoryItemFacts,
    canonical: bool,
    revision: &str,
    conversation: &str,
    operation: &'static str,
) -> CwrResult<ConversationHistoryEntry> {
    let record = item.record.ok_or_else(|| {
        Box::new(crate::v2026_09::conversation::BrowseMessageOverage {
            conversation_ref: conversation.into(),
            message_ref: item.message_ref.clone(),
            operation,
            payload_bytes: item.payload_bytes,
            limit_bytes: cwpdb_store::conversation::MAX_HISTORY_MESSAGE_BYTES,
        }) as Box<dyn std::error::Error + Send + Sync>
    })?;
    let message: ConversationMessage = serde_json::from_value(record)?;
    if message.message_ref != item.message_ref {
        return invalid("history message identity mismatch");
    }
    Ok(ConversationHistoryEntry {
        key: ConversationPositionKey {
            position: item.position,
            revision: canonical.then(|| revision.into()),
        },
        message,
        origin_invocation_ref: item.origin_invocation_ref,
    })
}
fn entries(facts: HistoryReadFacts, operation: &'static str) -> CwrResult<ConversationHistoryPage> {
    Ok(ConversationHistoryPage {
        content: facts
            .items
            .into_iter()
            .map(|item| {
                decode(
                    item,
                    facts.canonical,
                    &facts.revision,
                    &facts.conversation_ref,
                    operation,
                )
            })
            .collect::<CwrResult<_>>()?,
        has_next_page: facts.has_next_page,
        has_previous_page: facts.has_previous_page,
        total_count: usize::try_from(facts.total_count)?,
        revision: facts.revision,
    })
}
pub(crate) trait HistorySource: BrowseControl {
    fn facts(&self, selection: HistorySelection) -> CwrResult<HistoryReadFacts>;
}
impl HistorySource for CanonicalHistoryReader {
    fn facts(&self, selection: HistorySelection) -> CwrResult<HistoryReadFacts> {
        CanonicalHistoryReader::facts(self, selection)
    }
}
pub(crate) trait HistoryProjection: HistorySource + Sized {
    fn point_record(
        &self,
        key: HistoryItemFacts,
        page: &HistoryReadFacts,
    ) -> CwrResult<HistoryItemFacts> {
        let facts = self.facts(HistorySelection::RetainedMessage {
            message_ref: key.message_ref.clone(),
        })?;
        if facts.revision != page.revision
            || facts.canonical != page.canonical
            || facts.commit_revision != page.commit_revision
            || facts.items.len() != 1
        {
            return invalid("history point read snapshot mismatch");
        }
        let item = facts
            .items
            .into_iter()
            .next()
            .ok_or("history point read is empty")?;
        if item.position != key.position || item.message_ref != key.message_ref {
            return invalid("history point read identity mismatch");
        }
        Ok(item)
    }
    fn selected_records(&self, mut facts: HistoryReadFacts) -> CwrResult<HistoryReadFacts> {
        let keys = std::mem::take(&mut facts.items);
        let mut records = Vec::new();
        for key in keys {
            self.ensure_active()?;
            records.push(self.point_record(key, &facts)?);
        }
        facts.items = records;
        Ok(facts)
    }

    fn history(
        &self,
        window: ConversationRecordWindow<'_>,
        operation: &'static str,
    ) -> CwrResult<ConversationHistoryPage> {
        entries(
            self.selected_records(self.facts(HistorySelection::PagePositions {
                after: window.after.map(cursor),
                before: window.before.map(cursor),
                first: window.first,
                last: window.last,
            })?)?,
            operation,
        )
    }
    fn message(
        &self,
        message_ref: &str,
        operation: &'static str,
    ) -> CwrResult<(ConversationHistoryEntry, String)> {
        let facts = self.facts(HistorySelection::Message {
            message_ref: message_ref.into(),
        })?;
        let revision = facts.revision.clone();
        let mut page = entries(facts, operation)?;
        let entry = page
            .content
            .pop()
            .ok_or_else(|| invalid_error("message is unavailable in the requested conversation"))?;
        if entry.message.message_ref != message_ref || !page.content.is_empty() {
            return invalid("history message identity mismatch");
        }
        Ok((entry, revision))
    }
    fn conversation_browse_page(
        &self,
        _conversation: &str,
        window: ConversationRecordWindow<'_>,
        excerpt_bytes: usize,
    ) -> CwrResult<ConversationBrowsePage> {
        validate_excerpt(excerpt_bytes)?;
        if window
            .first
            .unwrap_or_default()
            .max(window.last.unwrap_or_default())
            > MAX_CONVERSATION_BROWSE_PAGE_SIZE
        {
            return invalid(format!("conversation browse page must not exceed {MAX_CONVERSATION_BROWSE_PAGE_SIZE} messages"));
        }
        let page = self.history(window, "conversation_browse_page")?;
        let retention = if window.first.is_none() && window.last.is_some() {
            BrowseRetention::Last
        } else {
            BrowseRetention::First
        };
        self.project(page, excerpt_bytes, retention)
    }
    fn project(
        &self,
        page: ConversationHistoryPage,
        excerpt_bytes: usize,
        retention: BrowseRetention,
    ) -> CwrResult<ConversationBrowsePage> {
        let candidates = page
            .content
            .into_iter()
            .map(|mut e| {
                self.ensure_active()?;
                e.message = e.message.clone_for_external_delivery();
                Ok((e, 0, 0))
            })
            .collect::<CwrResult<Vec<_>>>()?;
        let (content, before, after) =
            bounded_browse_entries(self, candidates, excerpt_bytes, retention)?;
        Ok(ConversationBrowsePage {
            content,
            has_next_page: page.has_next_page || after,
            has_previous_page: page.has_previous_page || before,
            total_count: page.total_count,
            revision: page.revision,
        })
    }
    fn conversation_history_around(
        &self,
        _conversation: &str,
        message_ref: &str,
        before_count: usize,
        after_count: usize,
        excerpt_bytes: usize,
    ) -> CwrResult<ConversationBrowsePage> {
        validate_excerpt(excerpt_bytes)?;
        if before_count.saturating_add(after_count).saturating_add(1)
            > MAX_CONVERSATION_BROWSE_PAGE_SIZE
        {
            return invalid(format!("conversation browse page must not exceed {MAX_CONVERSATION_BROWSE_PAGE_SIZE} messages"));
        }
        let (anchor, revision) = self.message(message_ref, "conversation_history_around")?;
        let key = anchor.key.clone();
        let before = self.history(
            ConversationRecordWindow {
                before: Some(&key),
                last: Some(before_count),
                ..Default::default()
            },
            "conversation_history_around",
        )?;
        let after = self.history(
            ConversationRecordWindow {
                after: Some(&key),
                first: Some(after_count),
                ..Default::default()
            },
            "conversation_history_around",
        )?;
        let anchor_index = before.content.len();
        self.project(
            ConversationHistoryPage {
                content: before
                    .content
                    .into_iter()
                    .chain(std::iter::once(anchor))
                    .chain(after.content)
                    .collect(),
                has_previous_page: before.has_previous_page,
                has_next_page: after.has_next_page,
                total_count: before.total_count,
                revision,
            },
            excerpt_bytes,
            BrowseRetention::Anchor(anchor_index),
        )
    }
    fn conversation_message_excerpt(
        &self,
        _conversation: &str,
        message_ref: &str,
        offset: usize,
        payload_offset: usize,
        max: usize,
    ) -> CwrResult<ConversationBrowseEntry> {
        validate_excerpt(max)?;
        let (mut entry, _) = self.message(message_ref, "conversation_message_excerpt")?;
        entry.key.revision = None;
        entry.message = entry.message.clone_for_external_delivery();
        let payload = serialized_action_payload(&entry.message);
        if excerpt_would_split_character(&entry.message.text, offset, max)
            || payload
                .json
                .as_deref()
                .is_some_and(|p| excerpt_would_split_character(p, payload_offset, max))
        {
            return invalid("excerpt limit is too small to include the next UTF-8 character");
        }
        fit_browse_entry(
            self,
            &entry,
            &payload,
            BrowseFitOptions {
                text_offset: offset,
                payload_offset,
                max_text_bytes: max,
                max_payload_bytes: max.min(MAX_CONVERSATION_ACTION_EXCERPT_BYTES),
                budget: MAX_CONVERSATION_BROWSE_RESPONSE_BYTES - 1024,
            },
        )?
        .ok_or_else(|| invalid_error("message metadata exceeds the browse response size limit"))
    }
    fn conversation_message_links(
        &self,
        _conversation: &str,
        message_ref: &str,
        after: Option<&str>,
        revision: Option<&str>,
        limit: usize,
    ) -> CwrResult<ConversationMessageLinks> {
        if limit == 0 || limit > MAX_CONVERSATION_BROWSE_PAGE_SIZE {
            return invalid(format!(
                "reply page size must be between 1 and {MAX_CONVERSATION_BROWSE_PAGE_SIZE}"
            ));
        }
        let facts = self.facts(HistorySelection::Replies {
            message_ref: message_ref.into(),
            after_message_ref: after.map(str::to_owned),
            after_revision: revision.map(str::to_owned),
            first: limit,
        })?;
        let item = facts
            .items
            .into_iter()
            .next()
            .ok_or("missing reply anchor")?;
        let entry = decode(
            item,
            facts.canonical,
            &facts.revision,
            &facts.conversation_ref,
            "conversation_message_links",
        )?;
        let next_after_message_ref = facts
            .has_next_page
            .then(|| facts.reply_message_refs.last().cloned())
            .flatten();
        Ok(ConversationMessageLinks {
            message_ref: message_ref.into(),
            reply_to_message_ref: entry.message.reply_to_message_ref,
            reply_message_refs: facts.reply_message_refs,
            has_next_replies: facts.has_next_page,
            next_after_message_ref,
            revision: facts.revision,
            origin_invocation_ref: entry.origin_invocation_ref,
        })
    }
    fn conversation_history_search(
        &self,
        _conversation: &str,
        query: &str,
        after: Option<i64>,
        revision: Option<&str>,
        limit: usize,
        excerpt_bytes: usize,
    ) -> CwrResult<ConversationSearchPage> {
        if query.trim().is_empty()
            || query.chars().count() > MAX_CONVERSATION_SEARCH_QUERY_CHARACTERS
        {
            return invalid(
                "conversation search query must be non-empty and within the character limit",
            );
        }
        if limit == 0 || limit > MAX_CONVERSATION_BROWSE_PAGE_SIZE {
            return invalid(format!("conversation search page size must be between 1 and {MAX_CONVERSATION_BROWSE_PAGE_SIZE}"));
        }
        validate_excerpt(excerpt_bytes)?;
        let folded = caseless::default_case_fold_str(query);
        let mut position = after;
        let mut cursor_revision = revision.map(str::to_owned);
        let mut found = Vec::new();
        let revision = loop {
            self.ensure_active()?;
            let facts = self.facts(HistorySelection::ScanPositions {
                after_position: position,
                after_revision: cursor_revision.clone(),
                first: 25,
            })?;
            let next = facts.has_next_page;
            let current = facts.revision.clone();
            let mut facts = facts;
            let keys = std::mem::take(&mut facts.items);
            for key in keys {
                let item = self.point_record(key, &facts)?;
                let mut entry = decode(
                    item,
                    facts.canonical,
                    &facts.revision,
                    &facts.conversation_ref,
                    "conversation_history_search",
                )?;
                self.ensure_active()?;
                position = Some(entry.key.position);
                entry.message = entry.message.clone_for_external_delivery();
                let text = caseless::default_case_fold_str(&entry.message.text)
                    .find(&folded)
                    .map(|o| folded_byte_offset_to_source(&entry.message.text, o));
                let payload = if text.is_none() {
                    super::search::action_payload_search_offset(self, &entry.message, &folded)?
                } else {
                    None
                };
                if text.is_some() || payload.is_some() {
                    found.push((entry, text.unwrap_or(0), payload.unwrap_or(0)));
                    if found.len() > limit {
                        break;
                    }
                }
            }
            if found.len() > limit || !next {
                break current;
            }
            cursor_revision = Some(current);
        };
        let has_next_page = found.len() > limit;
        found.truncate(limit);
        let (content, _, dropped) =
            bounded_browse_entries(self, found, excerpt_bytes, BrowseRetention::First)?;
        let next_after_position = content.last().map(|e| e.key.position).or(after);
        Ok(ConversationSearchPage {
            content,
            has_next_page: has_next_page || dropped,
            next_after_position,
            revision,
        })
    }
}
fn validate_excerpt(bytes: usize) -> CwrResult<()> {
    if !(4..=MAX_CONVERSATION_EXCERPT_BYTES).contains(&bytes) {
        return invalid(format!("conversation excerpt size must be between 4 and {MAX_CONVERSATION_EXCERPT_BYTES} bytes"));
    }
    Ok(())
}

impl<T: HistorySource> HistoryProjection for T {}
