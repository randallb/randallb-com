use caseless::Caseless;
use serde::{Deserialize, Serialize};

use super::*;

pub const MAX_CONVERSATION_BROWSE_PAGE_SIZE: usize = 25;
pub const MAX_CONVERSATION_EXCERPT_BYTES: usize = 2048;
pub const MAX_CONVERSATION_ACTION_EXCERPT_BYTES: usize = 2048;
pub const MAX_CONVERSATION_BROWSE_RESPONSE_BYTES: usize = 32 * 1024;
pub const MAX_CONVERSATION_SEARCH_QUERY_CHARACTERS: usize = 256;

/// One published message and its exact originating invocation, when retained.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConversationHistoryEntry {
    pub key: ConversationPositionKey,
    pub message: ConversationMessage,
    /// None for user messages and legacy replies without an explicit link.
    /// This identifies the publisher, not invocations triggered by the message.
    pub origin_invocation_ref: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConversationHistoryPage {
    pub content: Vec<ConversationHistoryEntry>,
    pub has_next_page: bool,
    pub has_previous_page: bool,
    pub total_count: usize,
    pub revision: String,
}

/// Small, attribution-preserving view of one published message. Large text is
/// returned in UTF-8 safe excerpts, including action payload JSON.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConversationBrowseEntry {
    pub key: ConversationPositionKey,
    pub message_ref: String,
    pub author_actor_ref: String,
    pub addressed_actor_refs: Vec<String>,
    pub reply_to_message_ref: Option<String>,
    pub action_kind: Option<String>,
    pub action_activation: Option<String>,
    /// Identifies which structured action field the JSON excerpt came from.
    pub action_payload_field: Option<String>,
    /// A UTF-8 safe excerpt of the serialized action input or result JSON.
    pub action_payload_json_excerpt: Option<String>,
    pub action_payload_json_bytes: Option<usize>,
    pub action_payload_offset_bytes: Option<usize>,
    pub next_action_payload_offset_bytes: Option<usize>,
    pub action_outcome: Option<crate::v2026_09::conversation::ConversationActionOutcome>,
    pub origin_invocation_ref: Option<String>,
    pub text_excerpt: String,
    pub text_bytes: usize,
    pub excerpt_offset_bytes: usize,
    pub next_offset_bytes: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConversationBrowsePage {
    pub content: Vec<ConversationBrowseEntry>,
    pub has_next_page: bool,
    pub has_previous_page: bool,
    pub total_count: usize,
    pub revision: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConversationSearchPage {
    pub content: Vec<ConversationBrowseEntry>,
    pub has_next_page: bool,
    pub next_after_position: Option<i64>,
    pub revision: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConversationMessageLinks {
    pub message_ref: String,
    pub reply_to_message_ref: Option<String>,
    pub reply_message_refs: Vec<String>,
    pub has_next_replies: bool,
    pub next_after_message_ref: Option<String>,
    pub revision: String,
    pub origin_invocation_ref: Option<String>,
}

pub(crate) mod backend;
mod native;
mod search;

fn browse_entry(
    entry: &ConversationHistoryEntry,
    action_payload: &SerializedActionPayload,
    text_offset_bytes: usize,
    payload_offset_bytes: usize,
    max_text_bytes: usize,
    max_payload_bytes: usize,
) -> CwrResult<ConversationBrowseEntry> {
    let text = &entry.message.text;
    let text_bytes = text.len();
    let text_start = utf8_boundary_at_or_after(text, text_offset_bytes.min(text_bytes));
    let text_end = utf8_boundary_at_or_before(
        text,
        text_start.saturating_add(max_text_bytes).min(text_bytes),
    );
    if max_text_bytes > 0 && text_start < text_bytes && text_end == text_start {
        return invalid("text excerpt is too small to include the next UTF-8 character");
    }
    let (payload_excerpt, payload_bytes, payload_start, payload_end) =
        if let Some(payload_json) = action_payload.json.as_deref() {
            let payload_bytes = payload_json.len();
            let start =
                utf8_boundary_at_or_after(payload_json, payload_offset_bytes.min(payload_bytes));
            let end = utf8_boundary_at_or_before(
                payload_json,
                start.saturating_add(max_payload_bytes).min(payload_bytes),
            );
            if max_payload_bytes > 0 && start < payload_bytes && end == start {
                return invalid(
                    "action payload excerpt is too small to include the next UTF-8 character",
                );
            }
            (
                Some(payload_json[start..end].to_owned()),
                Some(payload_bytes),
                Some(start),
                Some(end),
            )
        } else {
            (None, None, None, None)
        };
    let action_kind = entry.message.action.as_ref().map(|action| match action {
        crate::v2026_09::conversation::ConversationActionEvent::Call { .. } => "call".to_owned(),
        crate::v2026_09::conversation::ConversationActionEvent::Result { .. } => {
            "result".to_owned()
        }
    });
    let action_activation = entry.message.action.as_ref().map(|action| match action {
        crate::v2026_09::conversation::ConversationActionEvent::Call { activation, .. }
        | crate::v2026_09::conversation::ConversationActionEvent::Result { activation, .. } => {
            activation.clone()
        }
    });
    Ok(ConversationBrowseEntry {
        key: entry.key.clone(),
        message_ref: entry.message.message_ref.clone(),
        author_actor_ref: entry.message.author_actor_ref.clone(),
        addressed_actor_refs: entry.message.addressed_actor_refs.clone(),
        reply_to_message_ref: entry.message.reply_to_message_ref.clone(),
        action_kind,
        action_activation,
        action_payload_field: action_payload.field.clone(),
        action_payload_json_excerpt: payload_excerpt,
        action_payload_json_bytes: payload_bytes,
        action_payload_offset_bytes: payload_start,
        next_action_payload_offset_bytes: payload_end.filter(|end| Some(*end) < payload_bytes),
        action_outcome: action_payload.outcome,
        origin_invocation_ref: entry.origin_invocation_ref.clone(),
        text_excerpt: text[text_start..text_end].to_owned(),
        text_bytes,
        excerpt_offset_bytes: text_start,
        next_offset_bytes: (text_end < text_bytes).then_some(text_end),
    })
}

#[derive(Clone, Copy)]
pub(crate) enum BrowseRetention {
    First,
    Last,
    Anchor(usize),
}

fn bounded_browse_entries(
    operation: &impl BrowseControl,
    candidates: Vec<(ConversationHistoryEntry, usize, usize)>,
    excerpt_bytes: usize,
    retention: BrowseRetention,
) -> CwrResult<(Vec<ConversationBrowseEntry>, bool, bool)> {
    let budget = MAX_CONVERSATION_BROWSE_RESPONSE_BYTES - 1024;
    let mut output: Vec<(usize, ConversationBrowseEntry, usize)> = Vec::new();
    let mut used = 0usize;
    let mut dropped_before = false;
    let mut dropped_after = false;
    match retention {
        BrowseRetention::First => {
            for index in 0..candidates.len().min(MAX_CONVERSATION_BROWSE_PAGE_SIZE) {
                if !try_add_browse_candidate(
                    operation,
                    index,
                    &candidates,
                    excerpt_bytes,
                    budget,
                    &mut used,
                    &mut output,
                )? {
                    dropped_after = true;
                    break;
                }
            }
            if output.len() < candidates.len() {
                dropped_after = true;
            }
        }
        BrowseRetention::Last => {
            for index in (0..candidates.len())
                .rev()
                .take(MAX_CONVERSATION_BROWSE_PAGE_SIZE)
            {
                if !try_add_browse_candidate(
                    operation,
                    index,
                    &candidates,
                    excerpt_bytes,
                    budget,
                    &mut used,
                    &mut output,
                )? {
                    dropped_before = true;
                    break;
                }
            }
            if output.len() < candidates.len() {
                dropped_before = true;
            }
        }
        BrowseRetention::Anchor(anchor_index) => {
            if anchor_index >= candidates.len() {
                return invalid("around-message anchor was not present in the result window");
            }
            if !try_add_browse_candidate(
                operation,
                anchor_index,
                &candidates,
                excerpt_bytes,
                budget,
                &mut used,
                &mut output,
            )? {
                return invalid(
                    "around-message anchor metadata exceeds the browse response size limit",
                );
            }
            let mut before_blocked = anchor_index == 0;
            let mut after_blocked = anchor_index + 1 >= candidates.len();
            let mut distance = 1;
            while output.len() < MAX_CONVERSATION_BROWSE_PAGE_SIZE {
                if !before_blocked {
                    if let Some(index) = anchor_index.checked_sub(distance) {
                        if !try_add_browse_candidate(
                            operation,
                            index,
                            &candidates,
                            excerpt_bytes,
                            budget,
                            &mut used,
                            &mut output,
                        )? {
                            before_blocked = true;
                            dropped_before = true;
                        }
                    } else {
                        before_blocked = true;
                    }
                }
                if !after_blocked {
                    let after_index = anchor_index.saturating_add(distance);
                    if after_index < candidates.len() {
                        if !try_add_browse_candidate(
                            operation,
                            after_index,
                            &candidates,
                            excerpt_bytes,
                            budget,
                            &mut used,
                            &mut output,
                        )? {
                            after_blocked = true;
                            dropped_after = true;
                        }
                    } else {
                        after_blocked = true;
                    }
                }
                if before_blocked && after_blocked {
                    break;
                }
                distance += 1;
            }
            if output.len() < candidates.len() {
                dropped_before |= output.iter().map(|(index, _, _)| *index).min().unwrap_or(0) > 0;
                dropped_after |= output
                    .iter()
                    .map(|(index, _, _)| *index)
                    .max()
                    .unwrap_or(anchor_index)
                    + 1
                    < candidates.len();
            }
        }
    }
    output.sort_by_key(|(index, _, _)| *index);
    Ok((
        output.into_iter().map(|(_, entry, _)| entry).collect(),
        dropped_before,
        dropped_after,
    ))
}

fn try_add_browse_candidate(
    operation: &impl BrowseControl,
    index: usize,
    candidates: &[(ConversationHistoryEntry, usize, usize)],
    excerpt_bytes: usize,
    budget: usize,
    used: &mut usize,
    output: &mut Vec<(usize, ConversationBrowseEntry, usize)>,
) -> CwrResult<bool> {
    let (entry, text_offset, payload_offset) = &candidates[index];
    let payload_excerpt_bytes = excerpt_bytes.min(MAX_CONVERSATION_ACTION_EXCERPT_BYTES);
    operation.ensure_active()?;
    let action_payload = serialized_action_payload(&entry.message);
    operation.ensure_active()?;
    let options = BrowseFitOptions {
        text_offset: *text_offset,
        payload_offset: *payload_offset,
        max_text_bytes: excerpt_bytes,
        max_payload_bytes: payload_excerpt_bytes,
        budget: budget.saturating_sub(*used),
    };
    let candidate = fit_browse_entry(operation, entry, &action_payload, options)?;
    let Some(candidate) = candidate else {
        let individually_fits = fit_browse_entry(
            operation,
            entry,
            &action_payload,
            BrowseFitOptions { budget, ..options },
        )?;
        if individually_fits.is_none() {
            return invalid("message metadata exceeds the browse response size limit");
        }
        return Ok(false);
    };
    let size = serde_json::to_vec(&candidate)
        .map_err(|error| invalid_error(format!("could not encode browse result: {error}")))?
        .len();
    *used = used.saturating_add(size + 1);
    output.push((index, candidate, size));
    Ok(true)
}

#[derive(Clone, Copy)]
struct BrowseFitOptions {
    text_offset: usize,
    payload_offset: usize,
    max_text_bytes: usize,
    max_payload_bytes: usize,
    budget: usize,
}

fn fit_browse_entry(
    operation: &impl BrowseControl,
    entry: &ConversationHistoryEntry,
    action_payload: &SerializedActionPayload,
    options: BrowseFitOptions,
) -> CwrResult<Option<ConversationBrowseEntry>> {
    let BrowseFitOptions {
        text_offset,
        payload_offset,
        max_text_bytes,
        max_payload_bytes,
        budget,
    } = options;
    let mut text_bytes = max_text_bytes.min(MAX_CONVERSATION_EXCERPT_BYTES);
    let mut payload_bytes = max_payload_bytes.min(MAX_CONVERSATION_ACTION_EXCERPT_BYTES);
    loop {
        operation.ensure_active()?;
        if excerpt_would_split_character(&entry.message.text, text_offset, text_bytes) {
            text_bytes = 0;
        }
        if action_payload.json.as_deref().is_some_and(|payload| {
            excerpt_would_split_character(payload, payload_offset, payload_bytes)
        }) {
            payload_bytes = 0;
        }
        let has_text = has_excerpt_content(&entry.message.text, text_offset);
        let has_payload = action_payload
            .json
            .as_deref()
            .is_some_and(|payload| has_excerpt_content(payload, payload_offset));
        if (has_text || has_payload)
            && (!has_text || text_bytes == 0)
            && (!has_payload || payload_bytes == 0)
        {
            return Ok(None);
        }
        let candidate = browse_entry(
            entry,
            action_payload,
            text_offset,
            payload_offset,
            text_bytes,
            payload_bytes,
        )?;
        let size = serde_json::to_vec(&candidate)
            .map_err(|error| invalid_error(format!("could not encode browse result: {error}")))?
            .len();
        if size <= budget {
            return Ok(Some(candidate));
        }
        if text_bytes == 0 && payload_bytes == 0 {
            return Ok(None);
        }
        text_bytes /= 2;
        payload_bytes /= 2;
    }
}

struct SerializedActionPayload {
    field: Option<String>,
    json: Option<String>,
    outcome: Option<crate::v2026_09::conversation::ConversationActionOutcome>,
}

fn serialized_action_payload(message: &ConversationMessage) -> SerializedActionPayload {
    use crate::v2026_09::conversation::ConversationActionEvent;
    match message.action.as_ref() {
        Some(ConversationActionEvent::Call { input, .. }) => SerializedActionPayload {
            field: Some("input".into()),
            json: serde_json::to_string(input).ok(),
            outcome: None,
        },
        Some(ConversationActionEvent::Result {
            result, outcome, ..
        }) => SerializedActionPayload {
            field: Some("result".into()),
            json: serde_json::to_string(result).ok(),
            outcome: *outcome,
        },
        None => SerializedActionPayload {
            field: None,
            json: None,
            outcome: None,
        },
    }
}

fn excerpt_would_split_character(text: &str, offset: usize, max_bytes: usize) -> bool {
    let start = utf8_boundary_at_or_after(text, offset.min(text.len()));
    text.get(start..)
        .and_then(|remaining| remaining.chars().next())
        .is_some_and(|character| character.len_utf8() > max_bytes)
}

fn has_excerpt_content(text: &str, offset: usize) -> bool {
    utf8_boundary_at_or_after(text, offset.min(text.len())) < text.len()
}

fn folded_byte_offset_to_source(text: &str, folded_offset: usize) -> usize {
    let mut folded_bytes = 0;
    for (source_offset, character) in text.char_indices() {
        let next_folded_bytes = folded_bytes
            + std::iter::once(character)
                .default_case_fold()
                .map(char::len_utf8)
                .sum::<usize>();
        if folded_offset < next_folded_bytes {
            return source_offset;
        }
        folded_bytes = next_folded_bytes;
    }
    text.len()
}

#[cfg(test)]
#[test]
fn lowercase_expansion_offset_maps_to_its_source_character() {
    let text = "aİz";
    let lowered = caseless::default_case_fold_str(text);
    let dot_offset = lowered.find('\u{0307}').unwrap();
    assert_eq!(folded_byte_offset_to_source(text, dot_offset), 1);
}

fn utf8_boundary_at_or_after(text: &str, offset: usize) -> usize {
    let mut offset = offset.min(text.len());
    while offset < text.len() && !text.is_char_boundary(offset) {
        offset += 1;
    }
    offset
}

fn utf8_boundary_at_or_before(text: &str, offset: usize) -> usize {
    let mut offset = offset.min(text.len());
    while offset > 0 && !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

pub(crate) trait BrowseControl {
    fn ensure_active(&self) -> CwrResult<()>;
}
impl BrowseControl for ConversationReadOperation<'_> {
    fn ensure_active(&self) -> CwrResult<()> {
        ConversationReadOperation::ensure_active(self)
    }
}
impl BrowseControl for crate::v2026_09::conversation::backend::CanonicalHistoryReader {
    fn ensure_active(&self) -> CwrResult<()> {
        Self::ensure_active(self)
    }
}
