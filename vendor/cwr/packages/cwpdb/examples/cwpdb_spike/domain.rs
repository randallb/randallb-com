use rusqlite::{params, Connection};
use serde_json::json;

use super::{
    bindings::{self, RefId},
    model::*,
    store::*,
};

impl Store {
    pub fn create_workspace(&mut self, reference: &str) -> Result<()> {
        self.write(|tx| {
            insert_node(tx, reference, "workspace", &json!({}))?;
            insert_node(
                tx,
                &archive_ref(reference),
                "collection",
                &json!({"name": "Archive"}),
            )?;
            Ok(())
        })
    }

    pub fn create_actor(&mut self, reference: &str) -> Result<()> {
        self.write(|tx| insert_node(tx, reference, "actor", &json!({})).map(|_| ()))
    }

    pub fn create_conversation(
        &mut self,
        reference: &str,
        workspace: &str,
        title: &str,
        position: i64,
    ) -> Result<()> {
        self.write(|tx| {
            let workspace_id = require_kind(tx, workspace, "workspace")?;
            let conversation_id =
                insert_node(tx, reference, "conversation", &json!({"title": title}))?;
            insert_edge_ids(
                tx,
                conversation_id,
                WORKSPACE,
                workspace_id,
                position,
                &json!({}),
            )?;
            Ok(())
        })
    }

    pub fn append(&mut self, input: &Append<'_>) -> Result<()> {
        self.write(|tx| append(tx, input))
    }

    pub fn archive(&mut self, conversation: &str, archived: bool) -> Result<()> {
        self.write(|tx| {
            let conversation_id = require_kind(tx, conversation, "conversation")?;
            let workspace = target(tx, conversation, WORKSPACE)?;
            let collection = archive_ref(&workspace);
            let collection_id = bindings::resolve(tx, &collection)?;
            let exists = edge_exists(tx, conversation_id, COLLECTION, collection_id)?;
            if archived && !exists {
                insert_edge_ids(
                    tx,
                    conversation_id,
                    COLLECTION,
                    collection_id,
                    0,
                    &json!({}),
                )?;
            } else if !archived && exists {
                tx.execute(
                    "DELETE FROM edges WHERE source_id=?1 AND kind=?2 AND target_id=?3",
                    params![conversation_id, COLLECTION, collection_id],
                )?;
            }
            Ok(())
        })
    }

    pub fn move_conversation(&mut self, conversation: &str, workspace: &str) -> Result<()> {
        self.write(|tx| {
            let conversation_id = require_kind(tx, conversation, "conversation")?;
            let workspace_id = require_kind(tx, workspace, "workspace")?;
            let old = target(tx, conversation, WORKSPACE)?;
            if old == workspace { return Ok(()); }
            tx.execute("UPDATE edges SET target_id=?1,revision=revision+1 WHERE source_id=?2 AND kind=?3",
                params![workspace_id, conversation_id, WORKSPACE])?;
            tx.execute("UPDATE edges SET target_id=?1,revision=revision+1 WHERE source_id=?2 AND kind=?3 AND target_id=?4",
                params![bindings::resolve(tx, &archive_ref(workspace))?, conversation_id, COLLECTION, bindings::resolve(tx, &archive_ref(&old))?])?;
            tx.execute("UPDATE edges SET target_id=?1,revision=revision+1 WHERE kind=?2 AND source_id IN (
                SELECT target_id FROM edges WHERE source_id=?3 AND kind=?4)",
                params![workspace_id, SHORTCUT, conversation_id, CONTAINS])?;
            Ok(())
        })
    }

    pub fn rebuild_shortcuts(&mut self) -> Result<usize> {
        self.write(|tx| {
            tx.execute("DELETE FROM edges WHERE kind=?1", [SHORTCUT])?;
            // Bound temporary memory: the SELECT is streamed; inserts use a different edge kind.
            let mut query = tx.prepare(
                "SELECT m.target_id,w.target_id,m.position FROM edges m
                JOIN edges w ON w.source_id=m.source_id AND w.kind=?1 WHERE m.kind=?2",
            )?;
            let mut rows = query.query(params![WORKSPACE, CONTAINS])?;
            let mut count = 0;
            while let Some(row) = rows.next()? {
                shortcut(tx, row.get(0)?, row.get(1)?, row.get(2)?)?;
                count += 1;
            }
            Ok(count)
        })
    }
}

pub fn append(tx: &Connection, input: &Append<'_>) -> Result<()> {
    let conversation = require_kind(tx, input.conversation, "conversation")?;
    let author = require_kind(tx, input.author, "actor")?;
    ensure(
        !input.text.trim().is_empty(),
        "message text must not be blank",
    )?;
    let reply_id = if let Some(reply) = input.reply {
        let id = require_kind(tx, reply, "message")?;
        ensure(
            edge_exists(tx, conversation, CONTAINS, id)?,
            "reply must be in the same conversation",
        )?;
        Some(id)
    } else {
        None
    };
    let workspace = target_id(tx, conversation, WORKSPACE)?;
    let position: i64 = tx.query_row(
        "SELECT COALESCE(MAX(position),-1)+1 FROM edges WHERE source_id=?1 AND kind=?2",
        params![conversation, CONTAINS],
        |r| r.get(0),
    )?;
    let message = insert_node(
        tx,
        input.message_ref,
        "message",
        &json!({"text": input.text}),
    )?;
    insert_edge_ids(tx, conversation, CONTAINS, message, position, &json!({}))?;
    insert_edge_ids(tx, message, AUTHOR, author, position, &json!({}))?;
    if let Some(reply) = reply_id {
        insert_edge_ids(tx, message, REPLY, reply, position, &json!({}))?;
    }
    shortcut(tx, message, workspace, position)
}

fn shortcut(tx: &Connection, message: RefId, workspace: RefId, position: i64) -> Result<()> {
    insert_edge_ids(
        tx,
        message,
        SHORTCUT,
        workspace,
        position,
        &json!({"derived_from": [CONTAINS, WORKSPACE]}),
    )?;
    Ok(())
}
