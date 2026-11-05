//! Storage-only baseline: the pre-cutover schema, with message and timeline writes.
//! This does not measure admission, activities, scheduling, or full runtime API cost.
use std::{path::Path, time::Duration};

use rusqlite::{params, Connection, TransactionBehavior};
use serde::{Deserialize, Serialize};

use super::model::*;

// Frozen payload emitted by this historical storage-only workload. Keep its
// bytes independent from evolving CWR protocol DTOs; no runtime dependency.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BaselineMessage {
    message_ref: String,
    author_actor_ref: String,
    addressed_actor_refs: Vec<String>,
    text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reply_to_message_ref: Option<String>,
}

pub fn open(path: &Path) -> Result<Connection> {
    let db = Connection::open(path)?;
    db.execute_batch(include_str!("baseline_schema.sql"))?;
    db.execute_batch("PRAGMA journal_mode=WAL")?;
    db.busy_timeout(Duration::from_secs(5))?;
    db.pragma_update(None, "foreign_keys", "ON")?;
    db.pragma_update(None, "synchronous", "FULL")?;
    Ok(db)
}

pub fn conversation(db: &Connection, id: usize) -> Result<()> {
    db.execute("INSERT INTO conversations(conversation_ref,position,creation_order,title,participant_actor_refs_json)
        VALUES (?1,?2,?2,?3,'[]')", params![reference("conversations", id), i64::try_from(id)?, format!("Conversation {id}")])?;
    Ok(())
}

pub fn append(db: &Connection, input: &Append<'_>) -> Result<()> {
    let position: i64 = db.query_row(
        "SELECT COALESCE(MAX(position),-1)+1 FROM messages WHERE conversation_ref=?1",
        [input.conversation],
        |r| r.get(0),
    )?;
    let message = BaselineMessage {
        message_ref: input.message_ref.into(),
        author_actor_ref: input.author.into(),
        text: input.text.into(),
        addressed_actor_refs: Vec::new(),
        reply_to_message_ref: input.reply.map(str::to_owned),
    };
    let payload = serde_json::to_vec(&message)?;
    db.prepare_cached("INSERT INTO messages(message_ref,conversation_ref,position,payload_json) VALUES (?1,?2,?3,?4)")?
        .execute(params![input.message_ref, input.conversation, position, payload])?;
    db.prepare_cached("INSERT INTO timeline_messages(message_ref,conversation_ref,position,payload_json) VALUES (?1,?2,?3,?4)")?
        .execute(params![input.message_ref, input.conversation, position, payload])?;
    db.prepare_cached("INSERT INTO timeline_items(conversation_ref,position,item_kind,item_ref) VALUES (?1,?2,'message',?3)")?
        .execute(params![input.conversation, position, input.message_ref])?;
    db.prepare_cached("UPDATE conversations SET last_message_ref=?1,last_message_preview=?2,revision=revision+1 WHERE conversation_ref=?3")?
        .execute(params![input.message_ref, input.text.chars().take(120).collect::<String>(), input.conversation])?;
    Ok(())
}

pub fn append_committed(db: &mut Connection, input: &Append<'_>) -> Result<()> {
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    append(&tx, input)?;
    tx.commit()?;
    Ok(())
}

pub fn page(db: &Connection, conversation: &str, after: i64, limit: usize) -> Result<Vec<Message>> {
    let payloads = db.prepare_cached("SELECT m.payload_json FROM timeline_items i JOIN timeline_messages m
        ON m.conversation_ref=i.conversation_ref AND m.message_ref=i.item_ref
        WHERE i.conversation_ref=?1 AND i.position>?2 AND i.item_kind='message' ORDER BY i.position LIMIT ?3")?
        .query_map(params![conversation, after, i64::try_from(limit)?], |r| r.get::<_, Vec<u8>>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    payloads
        .into_iter()
        .map(|p| {
            let m: BaselineMessage = serde_json::from_slice(&p)?;
            Ok(Message {
                message_ref: m.message_ref,
                author_actor_ref: m.author_actor_ref,
                reply_to_message_ref: m.reply_to_message_ref,
                text: m.text,
            })
        })
        .collect()
}

pub fn by_author(db: &Connection, actor: &str) -> Result<Vec<String>> {
    Ok(db.prepare_cached("SELECT message_ref FROM messages WHERE json_extract(CAST(payload_json AS TEXT),'$.author_actor_ref')=?1
        ORDER BY message_ref LIMIT 50")?.query_map([actor], |r| r.get(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?)
}

pub fn bytes(db: &Connection) -> Result<i64> {
    db.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")?;
    let pages: i64 = db.query_row("PRAGMA page_count", [], |r| r.get(0))?;
    let size: i64 = db.query_row("PRAGMA page_size", [], |r| r.get(0))?;
    Ok(pages * size)
}

#[cfg(test)]
mod payload_tests {
    use super::*;
    #[test]
    fn historical_baseline_payload_bytes_and_reply_shape_remain_fixed() {
        let mut message = BaselineMessage {
            message_ref: "message:fixture".into(),
            author_actor_ref: "actor:fixture".into(),
            addressed_actor_refs: Vec::new(),
            text: "hello".into(),
            reply_to_message_ref: None,
        };
        assert_eq!(
            serde_json::to_string(&message).unwrap(),
            r#"{"message_ref":"message:fixture","author_actor_ref":"actor:fixture","addressed_actor_refs":[],"text":"hello"}"#
        );
        message.reply_to_message_ref = Some("message:parent".into());
        let bytes = serde_json::to_vec(&message).unwrap();
        let decoded: BaselineMessage = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            decoded.reply_to_message_ref.as_deref(),
            Some("message:parent")
        );
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()["reply_to_message_ref"],
            "message:parent"
        );
    }
}
