use super::*;
use std::io::{self, Write};

const MAX_INVOCATION_EXCERPT_BYTES: usize = 8192;

/// A bounded preview; use the report's execution timeline and message browse
/// actions to read retained payloads beyond the preview.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct InvocationRecordExcerpt {
    pub action_name: String,
    pub requester_actor_ref: String,
    pub target_actor_ref: String,
    pub state: ActionInvocationState,
    pub conversation_ref: Option<String>,
    pub parent_invocation_ref: Option<String>,
    pub source_message_ref: Option<String>,
    pub json_excerpt: String,
    pub truncated: bool,
}

impl InvocationRecordExcerpt {
    pub(crate) fn new(record: &ActionInvocationRecord) -> CwrResult<Self> {
        let (json_excerpt, truncated) = json_excerpt(record, MAX_INVOCATION_EXCERPT_BYTES)?;
        Ok(Self {
            action_name: record.action_name.clone(),
            requester_actor_ref: record.requester_actor_ref.clone(),
            target_actor_ref: record.target_actor_ref.clone(),
            state: record.state,
            conversation_ref: record.conversation_ref.clone(),
            parent_invocation_ref: record.parent_invocation_ref.clone(),
            source_message_ref: record.source_message_ref.clone(),
            json_excerpt,
            truncated,
        })
    }
}

fn json_excerpt(value: &impl Serialize, limit: usize) -> CwrResult<(String, bool)> {
    let mut writer = ExcerptWriter {
        bytes: Vec::with_capacity(limit),
        limit,
        truncated: false,
    };
    if let Err(error) = serde_json::to_writer(&mut writer, value) {
        if !writer.truncated {
            return Err(error.into());
        }
    }
    // The serializer may have stopped part way through a UTF-8 character.
    let valid = std::str::from_utf8(&writer.bytes)
        .map_or_else(|error| error.valid_up_to(), |text| text.len());
    writer.bytes.truncate(valid);
    Ok((String::from_utf8(writer.bytes)?, writer.truncated))
}

struct ExcerptWriter {
    bytes: Vec<u8>,
    limit: usize,
    truncated: bool,
}

impl Write for ExcerptWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let remaining = self.limit.saturating_sub(self.bytes.len());
        self.bytes
            .extend_from_slice(&buffer[..buffer.len().min(remaining)]);
        if buffer.len() > remaining {
            self.truncated = true;
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "invocation excerpt is complete",
            ));
        }
        Ok(buffer.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::ser::SerializeSeq;

    #[test]
    fn stops_serializing_after_the_budget_and_keeps_valid_utf8() {
        struct StopEarly;
        impl Serialize for StopEarly {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                let mut sequence = serializer.serialize_seq(None)?;
                sequence.serialize_element(&"💡".repeat(10_000))?;
                panic!("serializer must stop before visiting the rest of a large record");
            }
        }
        let (excerpt, truncated) = json_excerpt(&StopEarly, 8192).unwrap();
        assert!(truncated);
        assert!(excerpt.len() <= 8192);
        assert!(serde_json::to_string(&vec!["💡".repeat(10_000)])
            .unwrap()
            .starts_with(&excerpt));
        let (small, truncated) = json_excerpt(&serde_json::json!({"input": "ok"}), 8192).unwrap();
        assert!(!truncated);
        assert_eq!(
            serde_json::from_str::<Value>(&small).unwrap(),
            serde_json::json!({"input": "ok"})
        );
    }
}
