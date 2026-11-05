//! Last addressed recipients use canonical first placements in the same view.
use super::*;
use rusqlite::{params, OptionalExtension};
impl NativeReadSnapshot {
    pub fn last_addressed_actor_refs(
        &self,
        conversation: &str,
        root: &str,
        author: &str,
    ) -> CwpdbResult<Vec<String>> {
        self.control.check()?;
        valid_ref(conversation)?;
        valid_ref(root)?;
        valid_ref(author)?;
        let messages = canonical_messages_relation();
        let sql = format!("SELECT json_extract(CAST(messages.payload_json AS TEXT),'$.addressed_actor_refs') FROM {messages} WHERE json_extract(CAST(messages.payload_json AS TEXT),'$.author_actor_ref')=?3 AND json_array_length(json_extract(CAST(messages.payload_json AS TEXT),'$.addressed_actor_refs'))=1 ORDER BY messages.position DESC LIMIT 1");
        let refs: Option<String> = self
            .connection
            .query_row(&sql, params![conversation, root, author], |row| row.get(0))
            .optional()?;
        self.control.check()?;
        Ok(refs
            .map(|refs| serde_json::from_str(&refs))
            .transpose()?
            .unwrap_or_default())
    }
}

const TIMELINE_ITEM_EDGE: &str = "conversation_timeline_item";
const TIMELINE_CHILD_EDGE: &str = "conversation_timeline_child";
const TIMELINE_MESSAGE_EDGE: &str = "conversation_timeline_message";

fn flat_timeline_cte(root_parameter: &str) -> String {
    format!(
        "WITH RECURSIVE walk(item_id,path) AS (
           SELECT item.target_id,printf('%020d',item.position)
           FROM ref_bindings root
           JOIN edges item ON item.source_id=root.ref_id AND item.kind='{TIMELINE_ITEM_EDGE}'
           WHERE root.canonical_ref={root_parameter}
           UNION ALL
           SELECT nested.target_id,walk.path||'/'||printf('%020d',nested.position)
           FROM walk
           JOIN edges child ON child.source_id=walk.item_id AND child.kind='{TIMELINE_CHILD_EDGE}'
           JOIN edges nested ON nested.source_id=child.target_id AND nested.kind='{TIMELINE_ITEM_EDGE}'
         ), flat(position,item_id) AS (
           SELECT ROW_NUMBER() OVER (ORDER BY path)-1,item_id FROM walk
           WHERE NOT EXISTS(SELECT 1 FROM edges source JOIN nodes content ON content.ref_id=source.target_id
             WHERE source.source_id=walk.item_id AND source.kind='{TIMELINE_MESSAGE_EDGE}'
               AND json_type(content.props,'$.streaming')='object')
         )"
    )
}

/// One row per visible canonical message, ordered by its first placement in
/// the flattened authoritative timeline. Repeated placements remain distinct
/// timeline items but do not duplicate compatibility history records.
fn canonical_messages_relation() -> String {
    let flat = flat_timeline_cte("?2");
    format!(
        "({flat}, first_placements(message_ref,position) AS (
           SELECT reference.canonical_ref,MIN(flat.position)
           FROM flat
           JOIN edges content ON content.source_id=flat.item_id
             AND content.kind='{TIMELINE_MESSAGE_EDGE}'
           JOIN ref_bindings reference ON reference.ref_id=content.target_id
           GROUP BY reference.canonical_ref
         )
         SELECT first_placements.position,message.message_ref,
           message.payload_json,message.conversation_ref
         FROM first_placements
         JOIN messages message
           ON message.conversation_ref=?1
          AND message.message_ref=first_placements.message_ref) AS messages"
    )
}
