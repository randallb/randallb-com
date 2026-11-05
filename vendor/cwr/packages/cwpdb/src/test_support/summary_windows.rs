//! Four large canonical sources for testing summary windows without payload aggregation.
use crate::{CwpdbResult, NativeStore};
use serde_json::json;
use std::path::Path;

pub fn large_canonical_sources(path: &Path, runtime: &str) -> CwpdbResult<()> {
    let mut store = NativeStore::open(path)?;
    store.graph_transaction(|tx| {
        let root = format!("{runtime}/cwpdb");
        let metadata = crate::node_props(tx, &root)?.ok_or("fixture runtime missing")?;
        if metadata["runtime_root_ref"] != runtime { return Err("fixture runtime identity mismatch".into()); }
        crate::put_node(tx, &root, "runtime", &json!({"runtime_state_version":"cwp://org.coworkerprotocol/cwpdb/schemas/runtime-state/2026-09/conversation-timeline-v1"}))?;
        for i in 0..4 {
            let conversation = format!("{runtime}/large/conversations/{i}");
            let message = format!("{conversation}/message");
            let timeline = format!("{conversation}/timeline");
            let item = format!("{timeline}/item");
            if crate::node_props(tx, &conversation)?.is_some() { return Err("large summary fixture already exists".into()); }
            crate::put_node(tx, &conversation, "conversation", &json!({"creation_order":i,"title":format!("Large {i}"),"sidebar_eligible":true,"commit_revision":1,"last_message_ref":message,"last_message_preview":"legacy preview","completed_work_refs":[]}))?;
            let text = format!("Source {i} {}", "x".repeat(7 * 1024 * 1024));
            crate::put_node(tx, &message, "message", &json!({"record":{"message_ref":message,"author_actor_ref":format!("{runtime}/actors/customer"),"addressed_actor_refs":[],"text":text}}))?;
            crate::put_node(tx, &timeline, "conversation_timeline_connection", &json!({"revision":1,"store_sequence":i}))?;
            crate::put_node(tx, &item, "conversation_timeline_item", &json!({}))?;
            for (source, kind, target, position) in [(&root,"conversation",&conversation,i),(&root,"root_conversation",&conversation,i),(&conversation,"message",&message,0),(&conversation,"conversation_timeline_root",&timeline,0),(&timeline,"conversation_timeline_item",&item,0),(&item,"conversation_timeline_message",&message,0)] {
                crate::put_edge(tx, source, kind, target, position, &json!({}))?;
            }
        }
        Ok(())
    })
}
