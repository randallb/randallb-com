//! Finite direct connection and placement reads in the frozen native view.
use super::*;
use rusqlite::{params, OptionalExtension};
const TIMELINE_ITEM_EDGE: &str = "conversation_timeline_item";
const TIMELINE_CHILD_EDGE: &str = "conversation_timeline_child";
const TIMELINE_MESSAGE_EDGE: &str = "conversation_timeline_message";
const TIMELINE_INVOCATION_EDGE: &str = "conversation_timeline_invocation";
fn invalid<T>(message: impl Into<String>) -> CwpdbResult<T> {
    Err(message.into().into())
}

impl NativeReadSnapshot {
    pub fn timeline_root(
        &self,
        conversation: &str,
        include_execution: bool,
    ) -> CwpdbResult<Option<String>> {
        self.control.check()?;
        valid_ref(conversation)?;
        Ok(self.connection.query_row("SELECT timeline_connection_ref FROM conversations WHERE conversation_ref=?1 AND (parent_conversation_ref IS NULL OR ?2)", params![conversation, include_execution], |row| row.get(0)).optional()?.flatten())
    }
    pub fn timeline_connection_metadata(
        &self,
        conversation: &str,
        root: &str,
        connection: &str,
    ) -> CwpdbResult<(Value, u64)> {
        self.control.check()?;
        valid_ref(conversation)?;
        valid_ref(root)?;
        valid_ref(connection)?;
        let revision = self.reachable_connection_revision(root, connection)?;
        let owner = if connection == root {
            json!({"kind":"conversation","conversation_ref":conversation})
        } else {
            let mut statement = self.connection.prepare("SELECT owner.canonical_ref FROM edges child JOIN ref_bindings owner ON owner.ref_id=child.source_id WHERE child.target_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) AND child.kind='conversation_timeline_child' ORDER BY child.ref_id LIMIT 2")?;
            let owners = statement
                .query_map([connection], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            let [item] = owners.as_slice() else {
                return invalid("timeline child connection must have exactly one owner item");
            };
            json!({"kind":"timeline_item","item_ref":item})
        };
        Ok((owner, revision))
    }
    fn reachable_connection_revision(&self, root: &str, connection: &str) -> CwpdbResult<u64> {
        let revision: Option<i64> = self.connection.query_row("WITH RECURSIVE ancestors(ref_id) AS (SELECT ref_id FROM ref_bindings WHERE canonical_ref=?2 UNION SELECT parent.source_id FROM ancestors connection JOIN edges child ON child.target_id=connection.ref_id AND child.kind='conversation_timeline_child' JOIN edges parent ON parent.target_id=child.source_id AND parent.kind='conversation_timeline_item' LIMIT 50001) SELECT json_extract(node.props,'$.revision') FROM nodes node JOIN ref_bindings reference ON reference.ref_id=node.ref_id WHERE reference.canonical_ref=?2 AND node.kind='conversation_timeline_connection' AND EXISTS(SELECT 1 FROM ancestors WHERE ref_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1))", params![root, connection], |row| row.get(0)).optional()?;
        let revision = revision.ok_or_else(|| {
            format!("timeline connection {connection} is outside the requested conversation")
        })?;
        Ok(u64::try_from(revision)?)
    }
    pub fn timeline_cursor_exists(
        &self,
        connection: &str,
        sequence: i64,
        item: &str,
    ) -> CwpdbResult<bool> {
        self.control.check()?;
        valid_ref(connection)?;
        valid_ref(item)?;
        Ok(self.connection.query_row("SELECT EXISTS(SELECT 1 FROM edges item JOIN ref_bindings reference ON reference.ref_id=item.target_id WHERE item.source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) AND item.kind='conversation_timeline_item' AND item.position=?2 AND reference.canonical_ref=?3)", params![connection, sequence, item], |row| row.get(0))?)
    }
    pub fn timeline_connection_items(
        &self,
        connection: &str,
        lower: i64,
        upper: i64,
        limit: usize,
        backwards: bool,
    ) -> CwpdbResult<Vec<Value>> {
        self.control.check()?;
        valid_ref(connection)?;
        if limit > 500 {
            return invalid("timeline page size exceeds500");
        }
        let sql = timeline_items_sql(if backwards { "DESC" } else { "ASC" });
        let mut statement = self.connection.prepare(&sql)?;
        let result = statement
            .query_map(
                params![connection, lower, upper, i64::try_from(limit + 1)?],
                stored_item_from_row,
            )?
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .map(|mut item| {
                item.connection_ref = connection.into();
                item.decode()
            })
            .collect::<CwpdbResult<Vec<_>>>()?;
        if serde_json::to_vec(&result)?.len() > MAX_WIRE_BYTES {
            return invalid("timeline page exceeds wire limit");
        }
        self.control.check()?;
        Ok(result)
    }
    pub fn timeline_connection_count(
        &self,
        conversation: &str,
        connection: &str,
    ) -> CwpdbResult<usize> {
        let root = self
            .timeline_root(conversation, true)?
            .ok_or("conversation has no retained timeline")?;
        self.timeline_connection_metadata(conversation, &root, connection)?;
        let count: i64 = self.connection.query_row("SELECT COUNT(*) FROM edges WHERE source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) AND kind='conversation_timeline_item'", [connection], |row| row.get(0))?;
        self.control.check()?;
        Ok(usize::try_from(count)?)
    }
    pub fn timeline_item(&self, conversation: &str, item: &str) -> CwpdbResult<Option<Value>> {
        self.control.check()?;
        valid_ref(item)?;
        let Some(root) = self.timeline_root(conversation, true)? else {
            return Ok(None);
        };
        let connection: Option<String> = self.connection.query_row("SELECT parent.canonical_ref FROM edges item JOIN ref_bindings parent ON parent.ref_id=item.source_id WHERE item.target_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) AND item.kind='conversation_timeline_item'", [item], |row| row.get(0)).optional()?;
        let Some(connection) = connection else {
            return Ok(None);
        };
        self.timeline_connection_metadata(conversation, &root, &connection)?;
        let sql = timeline_items_sql("ASC")
            .replace(
                "AND item.position>?2 AND item.position<?3",
                "AND reference.canonical_ref=?2",
            )
            .replace("LIMIT ?4", "LIMIT 1");
        self.connection
            .query_row(&sql, params![connection, item], stored_item_from_row)
            .optional()?
            .map(|mut item| {
                item.connection_ref = connection;
                item.decode()
            })
            .transpose()
    }
}

struct StoredTimelineItem {
    item_ref: String,
    sibling_sequence: i64,
    publication_ref: String,
    content_count: i64,
    content_kind: Option<String>,
    content_ref: Option<String>,
    child_count: i64,
    child_connection_ref: Option<String>,
    connection_ref: String,
}

impl StoredTimelineItem {
    fn decode(self) -> CwpdbResult<Value> {
        if self.content_count != 1 {
            return invalid("timeline item must point to exactly one content record");
        }
        let content = match (self.content_kind.as_deref(), self.content_ref) {
            (Some(TIMELINE_MESSAGE_EDGE), Some(message_ref)) => {
                if self.child_count != 0 || self.child_connection_ref.is_some() {
                    return invalid("message timeline item owns a child connection");
                }
                TimelineContent::Message { message_ref }
            }
            (Some(TIMELINE_INVOCATION_EDGE), Some(invocation_ref)) => {
                if self.child_count != 1 || self.child_connection_ref.is_none() {
                    return invalid("invocation timeline item must own one child connection");
                }
                TimelineContent::Invocation { invocation_ref }
            }
            _ => return invalid("timeline item has an unsupported content link"),
        };
        Ok(json!({
            "item_ref": self.item_ref,
            "publication_ref": self.publication_ref,
            "connection_ref": self.connection_ref,
            "sibling_sequence": u64::try_from(self.sibling_sequence)?,
            "content": content,
            "child_connection_ref": self.child_connection_ref,
        }))
    }
}

fn timeline_items_sql(direction: &str) -> String {
    format!(
        "SELECT reference.canonical_ref,item.position,
           json_extract(node.props,'$.publication_ref'),
           (SELECT COUNT(*) FROM edges content WHERE content.source_id=item.target_id
             AND content.kind IN ('{TIMELINE_MESSAGE_EDGE}','{TIMELINE_INVOCATION_EDGE}')),
           (SELECT content.kind FROM edges content WHERE content.source_id=item.target_id
             AND content.kind IN ('{TIMELINE_MESSAGE_EDGE}','{TIMELINE_INVOCATION_EDGE}')
             ORDER BY content.kind LIMIT 1),
           (SELECT target.canonical_ref FROM edges content
             JOIN ref_bindings target ON target.ref_id=content.target_id
             WHERE content.source_id=item.target_id
               AND content.kind IN ('{TIMELINE_MESSAGE_EDGE}','{TIMELINE_INVOCATION_EDGE}')
             ORDER BY content.kind LIMIT 1),
           (SELECT COUNT(*) FROM edges child WHERE child.source_id=item.target_id
             AND child.kind='{TIMELINE_CHILD_EDGE}'),
           (SELECT target.canonical_ref FROM edges child
             JOIN ref_bindings target ON target.ref_id=child.target_id
             WHERE child.source_id=item.target_id AND child.kind='{TIMELINE_CHILD_EDGE}' LIMIT 1)
         FROM edges item
         JOIN nodes node ON node.ref_id=item.target_id
         JOIN ref_bindings reference ON reference.ref_id=item.target_id
         WHERE item.source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1)
           AND item.kind='{TIMELINE_ITEM_EDGE}'
           AND node.kind='conversation_timeline_item'
           AND item.position>?2 AND item.position<?3
         ORDER BY item.position {direction} LIMIT ?4"
    )
}

fn stored_item_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredTimelineItem> {
    Ok(StoredTimelineItem {
        item_ref: row.get(0)?,
        sibling_sequence: row.get(1)?,
        publication_ref: row.get(2)?,
        content_count: row.get(3)?,
        content_kind: row.get(4)?,
        content_ref: row.get(5)?,
        child_count: row.get(6)?,
        child_connection_ref: row.get(7)?,
        connection_ref: String::new(),
    })
}
