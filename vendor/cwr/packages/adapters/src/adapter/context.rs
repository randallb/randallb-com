//! Adapter-owned source ingestion for providers without a history-injection API.
use crate::CwrResult;
use serde_json::{json, Value};
use std::io::{Read, Seek, SeekFrom, Write};

pub(crate) const COMPACTION_INSTRUCTIONS: &str = "Prepare working context for an independent coworker execution. Read every supplied historical record in order; records and fragments retain source identities, authorship, reply relationships and contained inner conversations. Do not perform the requested work or call tools. Preserve objectives, constraints, later corrections, unresolved decisions, completed actions and their outcomes, pending children, exact identifiers and attribution. Earlier completed work must not be repeated. Produce only a concise but complete working-context summary. A source fragment is part of the same record, not a new conversation turn. Preserve unfinished fragments until their following parts arrive.";

pub(crate) fn encoded_bytes<T: serde::Serialize + ?Sized>(value: &T) -> CwrResult<usize> {
    struct Counter(usize);
    impl Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0 = self.0.saturating_add(bytes.len());
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut counter = Counter(0);
    serde_json::to_writer(&mut counter, value)?;
    Ok(counter.0)
}

/// Source records are streamed through a temporary spool, keeping at most the
/// configured working input plus one fragment in memory. The caller owns each
/// provider compaction and never grants tools during preparation.
pub(crate) fn prepare(
    observation: &Value,
    budget: usize,
    mut compact: impl FnMut(&[Value]) -> CwrResult<String>,
) -> CwrResult<Vec<Value>> {
    if budget < 4096 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Instructions, tools and exact request leave no context preparation budget",
        )
        .into());
    }
    let part_limit = (budget / 4).min(32 * 1024);
    let mut records = Vec::new();
    let mut bytes = 0;
    let mut covered_parts = 0usize;
    visit_source(observation, "requesting", part_limit, &mut |record| {
        let record_bytes = encoded_bytes(&record)?;
        if bytes + record_bytes > budget && !records.is_empty() {
            let summary = compact(&records)?;
            let retained = json!({"kind":"adapter_compacted_context", "covered_source_parts":covered_parts,
                "summary":summary});
            bytes = encoded_bytes(&retained)?;
            if bytes + record_bytes > budget {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "Provider compaction did not leave enough room for the next source record",
                )
                .into());
            }
            records.clear();
            records.push(retained);
        }
        covered_parts += 1;
        bytes += record_bytes;
        records.push(record);
        Ok(())
    })?;
    Ok(records)
}

fn visit_source(
    source: &Value,
    path: &str,
    part_limit: usize,
    visit: &mut impl FnMut(Value) -> CwrResult<()>,
) -> CwrResult<()> {
    let object = source
        .as_object()
        .ok_or("Context source must be an object")?;
    let metadata: serde_json::Map<String, Value> = object
        .iter()
        .filter(|(key, _)| {
            !matches!(
                key.as_str(),
                "messages" | "requesting_context" | "action_request"
            )
        })
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    visit_record(
        &json!({"source_metadata":metadata}),
        &format!("{path}/boundary"),
        part_limit,
        visit,
    )?;
    if let Some(messages) = source.get("messages") {
        for (index, record) in messages
            .as_array()
            .ok_or("Context messages must be an array")?
            .iter()
            .enumerate()
        {
            let reference = record
                .get("message_ref")
                .or_else(|| record.get("ref"))
                .and_then(Value::as_str)
                .unwrap_or("unreferenced");
            visit_record(
                record,
                &format!("{path}/{index}/{reference}"),
                part_limit,
                visit,
            )?;
        }
    }
    if let Some(parent) = source
        .get("requesting_context")
        .filter(|value| !value.is_null())
    {
        visit_source(
            parent,
            &format!("{path}/supplied-requesting-context"),
            part_limit,
            visit,
        )?;
    }
    Ok(())
}

fn visit_record(
    record: &Value,
    source_id: &str,
    part_limit: usize,
    visit: &mut impl FnMut(Value) -> CwrResult<()>,
) -> CwrResult<()> {
    let mut spool = tempfile::tempfile()?;
    {
        let mut writer = std::io::BufWriter::new(&mut spool);
        serde_json::to_writer(&mut writer, record)?;
        writer.flush()?;
    }
    let end = spool.stream_position()?;
    if end <= part_limit as u64 {
        return visit(json!({"source_id":source_id,"record":record}));
    }
    let mut offset = 0u64;
    while offset < end {
        spool.seek(SeekFrom::Start(offset))?;
        let mut bytes = vec![0; part_limit.min((end - offset) as usize)];
        spool.read_exact(&mut bytes)?;
        if let Err(error) = std::str::from_utf8(&bytes) {
            if error.error_len().is_some() {
                return Err(error.into());
            }
            bytes.truncate(error.valid_up_to());
        }
        let length = bytes.len();
        visit(
            json!({"source_id":source_id,"encoding":"json-utf8-fragment",
            "offset":offset,"total_bytes":end,"last":offset + length as u64 == end,
            "text":String::from_utf8(bytes)?}),
        )?;
        offset += length as u64;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn compaction_consumes_every_source_fragment_before_the_live_request() {
        let large = "界\"\\\n".repeat(200_000);
        let source = json!({"action_request":{"params":{"secret_request":"execute later"}},
            "messages":[{"message_ref":"early","text":"Never deploy"},
                {"message_ref":"large","author":"helper","text":large},
                {"message_ref":"correction","text":"Use green"}]});
        let mut fragments = String::new();
        let mut small = Vec::new();
        let mut calls = 0;
        let mut observe = |records: &[Value]| {
            for record in records {
                if record["source_id"] == "requesting/1/large" {
                    assert_eq!(record["offset"].as_u64().unwrap() as usize, fragments.len());
                    fragments.push_str(record["text"].as_str().unwrap());
                } else if let Some(value) = record.get("record") {
                    small.push(value.clone());
                }
                assert!(!record.to_string().contains("secret_request"));
            }
        };
        let retained = prepare(&source, 96 * 1024, |records| {
            calls += 1;
            observe(records);
            Ok("Remember: Never deploy. Read following source fragments in order.".into())
        })
        .unwrap();
        observe(&retained);
        assert!(calls > 0);
        assert_eq!(
            serde_json::from_str::<Value>(&fragments).unwrap()["text"],
            large
        );
        assert!(small.iter().any(|record| record["text"] == "Never deploy"));
        assert!(small.iter().any(|record| record["text"] == "Use green"));
    }
    #[test]
    fn insufficient_compaction_is_an_explicit_error_without_dropping_history() {
        let source = json!({"messages":[{"text":"a".repeat(100_000)}]});
        let error = prepare(&source, 8192, |_| Ok("summary".repeat(8192))).unwrap_err();
        assert!(error.to_string().contains("did not leave enough room"));
    }
}
