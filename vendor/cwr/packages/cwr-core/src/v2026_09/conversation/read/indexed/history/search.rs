use super::*;

// Return the serialized byte length and the first decoded-string match without
// repeatedly serializing every nested subtree. Check the read deadline at each
// node so large structured results cannot hold a browse request indefinitely.
pub(super) fn action_payload_search_offset(
    operation: &impl BrowseControl,
    message: &ConversationMessage,
    folded_query: &str,
) -> CwrResult<Option<usize>> {
    use crate::v2026_09::conversation::ConversationActionEvent;
    let Some(action) = message.action.as_ref() else {
        return Ok(None);
    };
    let value = match action {
        ConversationActionEvent::Call { input, .. } => input,
        ConversationActionEvent::Result { result, .. } => result,
    };
    Ok(serialized_json_search_offset(operation, value, folded_query)?.1)
}

fn serialized_json_search_offset(
    operation: &impl BrowseControl,
    value: &serde_json::Value,
    folded_query: &str,
) -> CwrResult<(usize, Option<usize>)> {
    operation.ensure_active()?;
    match value {
        serde_json::Value::String(text) => {
            let encoded = serde_json::to_string(text)?;
            let offset = caseless::default_case_fold_str(text)
                .find(folded_query)
                .map(|matched| {
                    let source = folded_byte_offset_to_source(text, matched);
                    serde_json::to_string(&text[..source])
                        .map(|prefix| prefix.len().saturating_sub(1))
                })
                .transpose()?;
            Ok((encoded.len(), offset))
        }
        serde_json::Value::Array(values) => {
            let mut bytes = 1usize;
            for (index, value) in values.iter().enumerate() {
                operation.ensure_active()?;
                if index > 0 {
                    bytes += 1; // comma
                }
                let (child_bytes, matched) =
                    serialized_json_search_offset(operation, value, folded_query)?;
                if let Some(matched) = matched {
                    return Ok((0, Some(bytes + matched)));
                }
                bytes += child_bytes;
            }
            Ok((bytes + 1, None))
        }
        serde_json::Value::Object(values) => {
            let mut bytes = 1usize;
            for (index, (key, value)) in values.iter().enumerate() {
                operation.ensure_active()?;
                if index > 0 {
                    bytes += 1; // comma
                }
                let encoded_key = serde_json::to_string(key)?;
                if let Some(matched) = caseless::default_case_fold_str(key).find(folded_query) {
                    let source = folded_byte_offset_to_source(key, matched);
                    let prefix = serde_json::to_string(&key[..source])?;
                    return Ok((0, Some(bytes + prefix.len().saturating_sub(1))));
                }
                bytes += encoded_key.len() + 1; // key and colon
                let (child_bytes, matched) =
                    serialized_json_search_offset(operation, value, folded_query)?;
                if let Some(matched) = matched {
                    return Ok((0, Some(bytes + matched)));
                }
                bytes += child_bytes;
            }
            Ok((bytes + 1, None))
        }
        _ => {
            let encoded = serde_json::to_string(value)?;
            Ok((
                encoded.len(),
                caseless::default_case_fold_str(&encoded).find(folded_query),
            ))
        }
    }
}
