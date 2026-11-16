use super::*;
impl SummaryReader<'_> {
    // Pin ordering depends on visible membership and archive state, not message content.
    pub(super) fn summary_membership_revision(
        &self,
        participant_actor_ref: Option<&str>,
        archived: Option<bool>,
    ) -> CwpdbResult<String> {
        let sql = format!(
            "SELECT conversation_ref, archived FROM root_conversations WHERE 1{} ORDER BY creation_order DESC, conversation_ref DESC",
            scope_clause(participant_actor_ref, archived, self.owner_context_filter.as_ref())
        );
        let mut statement = self.connection.prepare(&sql)?;
        let rows = statement.query_map(
            params_from_iter(scope_params(
                participant_actor_ref,
                archived,
                self.owner_context_filter.as_ref(),
            )),
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, bool>(1)?)),
        )?;
        let mut digest = Sha256::new();
        digest.update(b"cwr-summary-membership-v1\0");
        for row in rows {
            self.ensure_active()?;
            let (reference, archived) = row?;
            digest.update(u64::try_from(reference.len())?.to_be_bytes());
            digest.update(reference.as_bytes());
            digest.update([u8::from(archived)]);
        }
        Ok(format!("membership:v1:{}", hex::encode(digest.finalize())))
    }

    pub(super) fn summary_page_revision(
        &self,
        participant_actor_ref: Option<&str>,
        archived: Option<bool>,
    ) -> CwpdbResult<String> {
        let base = self.connection.query_row(
            "SELECT CAST(COALESCE(json_extract(props,'$.summary_membership_revision'),0) AS TEXT)||':'||CAST(COALESCE(json_extract(props,'$.completion_order'),0) AS TEXT) FROM nodes WHERE kind='runtime'",
            [],
            |row| row.get::<_, String>(0),
        )?;
        let sql = format!(
            "WITH RECURSIVE reachable(conversation_ref,ref_id) AS (
               SELECT root_conversations.conversation_ref,root.ref_id
               FROM root_conversations
               JOIN conversations USING(conversation_ref)
               JOIN ref_bindings root
                 ON root.canonical_ref=conversations.timeline_connection_ref
               WHERE 1{}
               UNION
               SELECT parent.conversation_ref,child.target_id
               FROM reachable parent
               JOIN edges item ON item.source_id=parent.ref_id
                 AND item.kind='conversation_timeline_item'
               JOIN edges child ON child.source_id=item.target_id
                 AND child.kind='conversation_timeline_child'
             )
             SELECT reachable.conversation_ref,reference.canonical_ref,
                    json_extract(node.props,'$.revision')
             FROM reachable
             JOIN nodes node ON node.ref_id=reachable.ref_id
             JOIN ref_bindings reference ON reference.ref_id=node.ref_id
             WHERE node.kind='conversation_timeline_connection'
             ORDER BY reachable.conversation_ref,
                      json_extract(node.props,'$.store_sequence'),node.ref_id",
            scope_clause(
                participant_actor_ref,
                archived,
                self.owner_context_filter.as_ref()
            )
        );
        let rows = self
            .connection
            .prepare(&sql)?
            .query_map(
                params_from_iter(scope_params(
                    participant_actor_ref,
                    archived,
                    self.owner_context_filter.as_ref(),
                )),
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                },
            )?
            .collect::<Result<Vec<_>, _>>()?;
        if rows.is_empty() {
            return Ok(base);
        }
        let mut digest = Sha256::new();
        digest.update(b"cwr-canonical-summary-page-v1\0");
        for (conversation_ref, connection_ref, revision) in rows {
            for reference in [conversation_ref, connection_ref] {
                digest.update(u64::try_from(reference.len())?.to_be_bytes());
                digest.update(reference.as_bytes());
            }
            digest.update(u64::try_from(revision)?.to_be_bytes());
        }
        Ok(format!(
            "{base}:canonical:v1:{}",
            hex::encode(digest.finalize())
        ))
    }
}
