use super::*;
impl SummaryReader<'_> {
    pub(super) fn flat_timeline_revision(&self, root_connection_ref: &str) -> CwpdbResult<String> {
        let mut statement = self.connection.prepare(
            "WITH RECURSIVE reachable(ref_id) AS (
               SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1
               UNION
               SELECT child.target_id FROM reachable parent
               JOIN edges item ON item.source_id=parent.ref_id AND item.kind=?2
               JOIN edges child ON child.source_id=item.target_id AND child.kind=?3
             )
             SELECT reference.canonical_ref,json_extract(node.props,'$.revision')
             FROM reachable
             JOIN nodes node ON node.ref_id=reachable.ref_id
             JOIN ref_bindings reference ON reference.ref_id=node.ref_id
             WHERE node.kind='conversation_timeline_connection'
             ORDER BY json_extract(node.props,'$.store_sequence'),node.ref_id",
        )?;
        let rows = statement
            .query_map(
                params![root_connection_ref, TIMELINE_ITEM_EDGE, TIMELINE_CHILD_EDGE],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
            )?
            .collect::<Result<Vec<_>, _>>()?;
        if rows.is_empty() || rows[0].0 != root_connection_ref {
            return invalid("conversation timeline root is unavailable");
        }
        let mut digest = Sha256::new();
        digest.update(b"cwr-canonical-flat-timeline-v1\0");
        for (reference, revision) in rows {
            digest.update(u64::try_from(reference.len())?.to_be_bytes());
            digest.update(reference.as_bytes());
            digest.update(u64::try_from(revision)?.to_be_bytes());
        }
        Ok(format!("canonical:v1:{}", hex::encode(digest.finalize())))
    }

    pub(super) fn canonical_last_message_ref(
        &self,
        conversation_ref: &str,
        root_connection_ref: &str,
    ) -> CwpdbResult<Option<String>> {
        let flat = flat_timeline_cte("?1");
        let sql = format!(
            "{flat}
             SELECT message.message_ref FROM flat
             JOIN edges content ON content.source_id=flat.item_id
               AND content.kind='{TIMELINE_MESSAGE_EDGE}'
             JOIN ref_bindings reference ON reference.ref_id=content.target_id
             JOIN messages message ON message.message_ref=reference.canonical_ref
               AND message.conversation_ref=?2
             ORDER BY flat.position DESC LIMIT 1"
        );
        let message = self
            .connection
            .query_row(
                &sql,
                params![root_connection_ref, conversation_ref],
                |row| row.get(0),
            )
            .optional()?;
        self.ensure_active()?;
        Ok(message)
    }

    pub(super) fn canonical_completed_work(
        &self,
        conversation_ref: &str,
        root_connection_ref: &str,
    ) -> CwpdbResult<Vec<SummaryCompletion>> {
        let filter = canonical_invocation_filter("reference.canonical_ref");
        let mut statement = self.connection.prepare(&format!(
            "SELECT invocation_ref,position FROM (
               SELECT reference.canonical_ref AS invocation_ref,
                      completion.position AS position,
                      completion.ref_id AS edge_id
               FROM edges completion
               JOIN ref_bindings reference ON reference.ref_id=completion.target_id
               WHERE completion.source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1)
                 AND completion.kind='completed_invocation' AND {filter}
               ORDER BY completion.position DESC,completion.ref_id DESC
               LIMIT 500
             ) ORDER BY position,edge_id"
        ))?;
        let records = statement
            .query_map(params![conversation_ref, root_connection_ref], |row| {
                Ok(SummaryCompletion {
                    invocation_ref: row.get(0)?,
                    sequence: row.get(1)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        self.ensure_active()?;
        Ok(records)
    }
}
pub(super) fn canonical_invocation_filter(column: &str) -> String {
    canonical_content_filter(column, TIMELINE_INVOCATION_EDGE)
}

fn canonical_content_filter(column: &str, content_edge: &str) -> String {
    format!(
        "{column} IN (
           WITH RECURSIVE connections(ref_id) AS (
             SELECT ref_id FROM ref_bindings WHERE canonical_ref=?2
             UNION
             SELECT child.target_id FROM connections parent
             JOIN edges item ON item.source_id=parent.ref_id AND item.kind='{TIMELINE_ITEM_EDGE}'
             JOIN edges child ON child.source_id=item.target_id AND child.kind='{TIMELINE_CHILD_EDGE}'
           )
           SELECT content_ref.canonical_ref FROM connections
           JOIN edges item ON item.source_id=connections.ref_id AND item.kind='{TIMELINE_ITEM_EDGE}'
           JOIN edges content ON content.source_id=item.target_id AND content.kind='{content_edge}'
           JOIN ref_bindings content_ref ON content_ref.ref_id=content.target_id
         )"
    )
}

pub(super) fn flat_timeline_cte(root_parameter: &str) -> String {
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
