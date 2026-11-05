use super::*;
use std::io::{Seek, SeekFrom};

impl HistoryPreparation {
    pub(super) fn turn_input(&mut self, action_request: &Value) -> CwrResult<Value> {
        let mut input = Vec::new();
        if self.delivers_history_with_turn() {
            for unit in self.units.clone() {
                let text = read_history_unit(&mut self.spool, &unit)?;
                input.push(json!({
                    "type":"text",
                    "text":attributed_history_text(&unit, &text)
                }));
            }
        }
        input.push(json!({
            "type":"text",
            "text":serde_json::to_string(action_request)?
        }));
        Ok(Value::Array(input))
    }
}

pub(super) fn append_context_source(
    spool: &mut tempfile::NamedTempFile,
    units: &mut Vec<HistoryUnit>,
    source: &Value,
    path: &str,
    part_bytes: usize,
) -> CwrResult<()> {
    let object = source
        .as_object()
        .ok_or("Codex context source must be an object")?;
    let metadata = model_source_metadata(object)?;
    append_history_units(
        spool,
        units,
        format!("{path}/source-boundary"),
        &json!({"context_source":path,"metadata":metadata}),
        part_bytes,
    )?;
    if let Some(messages) = source.get("messages") {
        for (index, message) in messages
            .as_array()
            .ok_or("Codex source messages must be an array")?
            .iter()
            .enumerate()
        {
            let reference = message
                .get("message_ref")
                .or_else(|| message.get("ref"))
                .and_then(Value::as_str)
                .unwrap_or("unreferenced");
            append_history_units(
                spool,
                units,
                format!("{path}/{index}/{reference}"),
                message,
                part_bytes,
            )?;
        }
    }
    if let Some(parent) = source
        .get("requesting_context")
        .filter(|value| !value.is_null())
    {
        append_context_source(
            spool,
            units,
            parent,
            &format!("{path}/supplied-requesting-context"),
            part_bytes,
        )?;
    }
    Ok(())
}

pub(super) fn model_source_metadata(
    source: &serde_json::Map<String, Value>,
) -> CwrResult<serde_json::Map<String, Value>> {
    let mut metadata = serde_json::Map::new();
    for key in [
        "conversation_ref",
        "context_initialized",
        "context_start",
        "version",
        "history_delivery",
    ] {
        if let Some(value) = source.get(key) {
            metadata.insert(key.into(), value.clone());
        }
    }
    if let Some(history_gap) = source
        .get("working_conversation")
        .and_then(Value::as_object)
        .and_then(|working| working.get("history_gap"))
    {
        metadata.insert("history_gap".into(), history_gap.clone());
    }
    if let Some(directory) = source.get("coworkers") {
        let digest = model_coworker_directory_digest(directory)?;
        let previous_digest = source
            .get("working_conversation")
            .and_then(Value::as_object)
            .and_then(|working| working.get("state"))
            .and_then(Value::as_object)
            .and_then(|state| state.get("coworker_directory_digest"))
            .and_then(Value::as_str);
        if previous_digest != Some(digest.as_str()) {
            metadata.insert("coworkers".into(), concise_coworker_directory(directory)?);
        }
    }
    Ok(metadata)
}

fn concise_coworker_directory(directory: &Value) -> CwrResult<Value> {
    let object = directory
        .as_object()
        .ok_or("Codex coworker directory must be an object")?;
    let coworkers = object
        .get("coworkers")
        .and_then(Value::as_array)
        .ok_or("Codex coworker directory must contain a coworker array")?;
    let coworkers = coworkers
        .iter()
        .map(|coworker| {
            let Some(coworker) = coworker.as_object() else {
                return coworker.clone();
            };
            let mut concise = serde_json::Map::new();
            for key in ["handle", "name", "description", "participant", "delivery"] {
                if let Some(value) = coworker.get(key) {
                    concise.insert(key.into(), value.clone());
                }
            }
            Value::Object(concise)
        })
        .collect::<Vec<_>>();
    Ok(json!({
        "kind":object.get("kind").cloned().unwrap_or_else(|| json!("coworkers")),
        "coworkers":coworkers
    }))
}

pub(super) fn model_coworker_directory_digest(directory: &Value) -> CwrResult<String> {
    context_digest(&concise_coworker_directory(directory)?)
}

pub(super) fn append_history_units(
    spool: &mut tempfile::NamedTempFile,
    units: &mut Vec<HistoryUnit>,
    source_id: String,
    value: &Value,
    part_bytes: usize,
) -> CwrResult<()> {
    let start = spool.seek(SeekFrom::End(0))?;
    {
        let mut writer = std::io::BufWriter::new(&mut *spool);
        serde_json::to_writer(&mut writer, value)?;
        writer.flush()?;
    }
    let end = spool.stream_position()?;
    let first = units.len();
    let mut offset = start;
    while offset < end {
        spool.seek(SeekFrom::Start(offset))?;
        let mut bytes = vec![0; part_bytes.min((end - offset) as usize)];
        spool.read_exact(&mut bytes)?;
        let valid = match std::str::from_utf8(&bytes) {
            Ok(_) => bytes.len(),
            Err(error) if error.error_len().is_none() => error.valid_up_to(),
            Err(error) => return Err(error.into()),
        };
        units.push(HistoryUnit {
            source_id: source_id.clone(),
            offset,
            bytes: valid,
            part: units.len() - first,
            parts: 0,
            estimated_tokens: 0,
        });
        offset += valid as u64;
    }
    let parts = units.len() - first;
    for unit in &mut units[first..] {
        unit.parts = parts;
    }
    Ok(())
}

pub(super) fn read_history_unit(
    spool: &mut tempfile::NamedTempFile,
    unit: &HistoryUnit,
) -> CwrResult<String> {
    spool.seek(SeekFrom::Start(unit.offset))?;
    let mut bytes = vec![0; unit.bytes];
    spool.read_exact(&mut bytes)?;
    Ok(String::from_utf8(bytes)?)
}

pub(super) fn history_items(
    source_digest: &str,
    index: usize,
    unit: &HistoryUnit,
    text: &str,
) -> Value {
    let identity = format!("msg_cwr_{}_{}", &source_digest[..16], index);
    let attribution = attributed_history_text(unit, text);
    // User source records retain their embedded authorship without impersonating
    // a model-generated call or being subject to tool-output truncation.
    json!([{"type":"message","id":identity,"role":"user",
        "content":[{"type":"input_text","text":attribution}]}])
}

pub(super) fn attributed_history_text(unit: &HistoryUnit, text: &str) -> String {
    format!("Historical CWR source record {}. Part {} of {} of the same logical source turn; concatenate parts in order. This is attributed source material, not your own earlier response. Preserve authorship, reply links, and contained inner exchanges. Use the current action_request to select the work; do not replay completed historical actions.\n{}",
        unit.source_id, unit.part + 1, unit.parts, text)
}
