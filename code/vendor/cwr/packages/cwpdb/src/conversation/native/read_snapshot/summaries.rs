//! Frozen finite summary reads. Query shape and indexing stay inside cwpdb.
mod canonical;
mod revisions;
#[cfg(test)]
mod tests;
use super::*;
use crate::conversation::summary_page::SummaryWindow;
use rusqlite::{params, params_from_iter, types::Value as SqlValue, OptionalExtension};
use sha2::{Digest, Sha256};
const MAX_CONVERSATION_SUMMARY_PAGE_SIZE: usize = 200;
const TIMELINE_ITEM_EDGE: &str = "conversation_timeline_item";
const TIMELINE_CHILD_EDGE: &str = "conversation_timeline_child";
const TIMELINE_MESSAGE_EDGE: &str = "conversation_timeline_message";
const TIMELINE_INVOCATION_EDGE: &str = "conversation_timeline_invocation";
fn invalid<T>(message: impl Into<String>) -> CwpdbResult<T> {
    Err(message.into().into())
}
fn invalid_error(message: impl Into<String>) -> Box<dyn std::error::Error + Send + Sync> {
    message.into().into()
}
struct SummaryReader<'a> {
    connection: &'a Connection,
    control: &'a ReadControl,
    owner_context_filter: Option<(String, bool)>,
}
impl SummaryReader<'_> {
    fn ensure_active(&self) -> CwpdbResult<()> {
        self.control.check()
    }
}
impl NativeReadSnapshot {
    /// Fetch only a selected preview source, keeping large payloads out of a window's wire envelope.
    pub fn summary_message_record(&self, conversation: &str, message: &str) -> CwpdbResult<Value> {
        self.control.check()?;
        let record = self.connection.query_row(
            "SELECT payload_json FROM messages WHERE message_ref=?1 AND conversation_ref=?2",
            params![message, conversation],
            |row| decode_summary_blob::<Value>(row.get_ref(0)?.as_blob()?),
        )?;
        if record["message_ref"] != message {
            return invalid("conversation summary message identity mismatch");
        }
        if serde_json::to_vec(&record)?.len() > MAX_WIRE_BYTES {
            return invalid("summary message exceeds wire budget");
        }
        self.control.check()?;
        Ok(record)
    }
    pub fn summary_row(
        &self,
        reference: &str,
        workspace: Option<(Option<&str>, &str)>,
    ) -> CwpdbResult<Option<SummaryWindowRow>> {
        self.control.check()?;
        let reader = SummaryReader {
            connection: &self.connection,
            control: &self.control,
            owner_context_filter: None,
        };
        let result = match workspace {
            Some((owner, actor)) => {
                reader.workspace_conversation_summary(reference, owner, actor)?
            }
            None => reader.root_conversation_summary(reference)?,
        };
        if serde_json::to_vec(&result)?.len() > MAX_WIRE_BYTES {
            return invalid("summary row exceeds wire budget");
        }
        self.control.check()?;
        Ok(result)
    }
    pub fn summary_window(
        &self,
        request: &SummaryWindowRequest,
    ) -> CwpdbResult<SummaryWindowFacts> {
        self.control.check()?;
        let reader = SummaryReader {
            connection: &self.connection,
            control: &self.control,
            owner_context_filter: request.owner_context_filter.clone(),
        };
        let result = reader.root_conversations_in_scope(
            SummaryWindow {
                after: request.after.as_ref(),
                before: request.before.as_ref(),
                first: request.first,
                last: request.last,
            },
            request.participant_actor_ref.as_deref(),
            request.archived,
        )?;
        if serde_json::to_vec(&result)?.len() > MAX_WIRE_BYTES {
            return invalid("summary window exceeds wire budget");
        }
        self.control.check()?;
        Ok(result)
    }
}
impl SummaryReader<'_> {
    pub fn root_conversation_summary(
        &self,
        reference: &str,
    ) -> CwpdbResult<Option<SummaryWindowRow>> {
        self.ensure_active()?;
        Ok(self
            .query_summaries(
                &format!("{SUMMARY_SELECT} AND conversation_ref = ?"),
                [reference],
            )?
            .pop())
    }

    /// A Workspace point read includes the participant's current owner context
    /// and legacy ownerless conversations, but not other people's conversations
    /// or retained conversations from removed packs.
    pub fn workspace_conversation_summary(
        &self,
        reference: &str,
        workspace_owner_context_ref: Option<&str>,
        participant_actor_ref: &str,
    ) -> CwpdbResult<Option<SummaryWindowRow>> {
        self.ensure_active()?;
        let owner = workspace_owner_context_ref.map(|ref_id| (ref_id.to_owned(), true));
        let mut sql = format!(
            "{SUMMARY_SELECT}{} AND root_conversations.conversation_ref = ?",
            scope_clause(Some(participant_actor_ref), None, owner.as_ref())
        );
        if owner.is_none() {
            sql.push_str(" AND root_conversations.owner_context_ref IS NULL");
        }
        let mut params = scope_params(Some(participant_actor_ref), None, owner.as_ref());
        params.push(SqlValue::Text(reference.to_owned()));
        let rows = self.query_summaries(&sql, params_from_iter(params))?;
        Ok(rows.into_iter().next())
    }

    pub(crate) fn root_conversations_in_scope(
        &self,
        window: SummaryWindow<'_>,
        participant_actor_ref: Option<&str>,
        archived: Option<bool>,
    ) -> CwpdbResult<SummaryWindowFacts> {
        self.ensure_active()?;
        validate_window(window)?;
        if let Some(after) = window.after {
            self.require_root_key(after, participant_actor_ref, archived)?;
        }
        if let Some(before) = window.before {
            self.require_root_key(before, participant_actor_ref, archived)?;
        }
        if let (Some(after), Some(before)) = (window.after, window.before) {
            if key_tuple(after) <= key_tuple(before) {
                return invalid("conversation page boundaries do not define a forward range");
            }
        }

        let total_count: i64 = self.connection.query_row(
            &format!(
                "SELECT COUNT(*) FROM root_conversations WHERE 1{}",
                scope_clause(
                    participant_actor_ref,
                    archived,
                    self.owner_context_filter.as_ref()
                )
            ),
            params_from_iter(scope_params(
                participant_actor_ref,
                archived,
                self.owner_context_filter.as_ref(),
            )),
            |row| row.get(0),
        )?;
        let total_count = usize::try_from(total_count)
            .map_err(|_| invalid_error("root conversation count is out of range"))?;
        let archived_count: i64 = self.connection.query_row(
            &format!(
                "SELECT COUNT(*) FROM root_conversations WHERE archived{}",
                scope_clause(
                    participant_actor_ref,
                    archived,
                    self.owner_context_filter.as_ref()
                )
            ),
            params_from_iter(scope_params(
                participant_actor_ref,
                archived,
                self.owner_context_filter.as_ref(),
            )),
            |row| row.get(0),
        )?;
        let archived_count = usize::try_from(archived_count)
            .map_err(|_| invalid_error("archived conversation count is out of range"))?;
        self.ensure_active()?;

        let mut summaries = if let Some(first) = window.first {
            self.query_forward(
                window.after,
                window.before,
                first.max(window.last.unwrap_or_default()),
                participant_actor_ref,
                archived,
            )?
        } else {
            self.query_backward(
                window.after,
                window.before,
                window.last.unwrap_or_default(),
                participant_actor_ref,
                archived,
            )?
        };
        let has_next_page = window.first.is_some_and(|first| summaries.len() > first);

        // Relay computes PageInfo against the cursor-filtered range, then applies first before last.
        let has_previous_page = if let Some(first) = window.first {
            summaries.truncate(first);
            window.last.is_some_and(|last| summaries.len() > last)
        } else {
            let has_previous_page = window.last.is_some_and(|last| summaries.len() > last);
            summaries.truncate(window.last.unwrap_or_default());
            summaries.reverse();
            has_previous_page
        };
        if let Some(last) = window.last {
            if summaries.len() > last {
                summaries.drain(..summaries.len() - last);
            }
        }

        let has_previous_page = if window.last.is_some() {
            has_previous_page
        } else if let Some(first) = summaries.first() {
            self.root_exists_newer_than(&first.key, window.after, participant_actor_ref, archived)?
        } else {
            false
        };
        let has_next_page = if window.first.is_some() {
            has_next_page
        } else if let Some(last) = summaries.last() {
            self.root_exists_older_than(&last.key, window.before, participant_actor_ref, archived)?
        } else {
            false
        };
        Ok(SummaryWindowFacts {
            membership_revision: self
                .summary_membership_revision(participant_actor_ref, archived)?,
            revision: self.summary_page_revision(participant_actor_ref, archived)?,
            summaries,
            has_next_page,
            has_previous_page,
            total_count,
            archived_count,
        })
    }

    fn query_forward(
        &self,
        after: Option<&SummaryWindowKey>,
        before: Option<&SummaryWindowKey>,
        first: usize,
        participant_actor_ref: Option<&str>,
        archived: Option<bool>,
    ) -> CwpdbResult<Vec<SummaryWindowRow>> {
        self.ensure_active()?;
        let take = sql_take(first)?;
        let mut sql = format!(
            "{SUMMARY_SELECT}{}",
            scope_clause(
                participant_actor_ref,
                archived,
                self.owner_context_filter.as_ref()
            )
        );
        let mut params = scope_params(
            participant_actor_ref,
            archived,
            self.owner_context_filter.as_ref(),
        );
        append_summary_bounds(&mut sql, &mut params, after, before);
        sql.push(' ');
        sql.push_str(ORDER_DESC);
        sql.push_str(" LIMIT ?");
        params.push(SqlValue::Integer(take));
        self.query_summaries(&sql, params_from_iter(params))
    }

    fn query_backward(
        &self,
        after: Option<&SummaryWindowKey>,
        before: Option<&SummaryWindowKey>,
        last: usize,
        participant_actor_ref: Option<&str>,
        archived: Option<bool>,
    ) -> CwpdbResult<Vec<SummaryWindowRow>> {
        self.ensure_active()?;
        let take = sql_take(last)?;
        let mut sql = format!(
            "{SUMMARY_SELECT}{}",
            scope_clause(
                participant_actor_ref,
                archived,
                self.owner_context_filter.as_ref()
            )
        );
        let mut params = scope_params(
            participant_actor_ref,
            archived,
            self.owner_context_filter.as_ref(),
        );
        append_summary_bounds(&mut sql, &mut params, after, before);
        sql.push(' ');
        sql.push_str(ORDER_ASC);
        sql.push_str(" LIMIT ?");
        params.push(SqlValue::Integer(take));
        self.query_summaries(&sql, params_from_iter(params))
    }

    fn require_root_key(
        &self,
        key: &SummaryWindowKey,
        participant_actor_ref: Option<&str>,
        archived: Option<bool>,
    ) -> CwpdbResult<()> {
        self.ensure_active()?;
        let mut params = scope_params(
            participant_actor_ref,
            archived,
            self.owner_context_filter.as_ref(),
        );
        params.push(SqlValue::Integer(key.creation_order));
        params.push(SqlValue::Text(key.conversation_ref.clone()));
        let exists: bool = self.connection.query_row(
            &format!("SELECT EXISTS(SELECT 1 FROM root_conversations WHERE 1{} AND creation_order = ? AND conversation_ref = ?)", scope_clause(participant_actor_ref, archived, self.owner_context_filter.as_ref())),
            params_from_iter(params),
            |row| row.get(0),
        )?;
        if !exists {
            return invalid("conversation page boundary is not in the root conversation relation");
        }
        Ok(())
    }

    fn root_exists_newer_than(
        &self,
        key: &SummaryWindowKey,
        after: Option<&SummaryWindowKey>,
        participant_actor_ref: Option<&str>,
        archived: Option<bool>,
    ) -> CwpdbResult<bool> {
        self.ensure_active()?;
        let mut sql = format!("SELECT EXISTS(SELECT 1 FROM root_conversations WHERE 1{} AND (creation_order, conversation_ref) > (?, ?)", scope_clause(participant_actor_ref, archived, self.owner_context_filter.as_ref()));
        let mut params = scope_params(
            participant_actor_ref,
            archived,
            self.owner_context_filter.as_ref(),
        );
        params.extend(summary_key_params(key));
        if let Some(after) = after {
            sql.push_str(" AND (creation_order, conversation_ref) < (?, ?)");
            params.extend(summary_key_params(after));
        }
        sql.push(')');
        let result = self
            .connection
            .query_row(&sql, params_from_iter(params), |row| row.get(0));
        let exists = result?;
        self.ensure_active()?;
        Ok(exists)
    }

    fn root_exists_older_than(
        &self,
        key: &SummaryWindowKey,
        before: Option<&SummaryWindowKey>,
        participant_actor_ref: Option<&str>,
        archived: Option<bool>,
    ) -> CwpdbResult<bool> {
        self.ensure_active()?;
        let mut sql = format!("SELECT EXISTS(SELECT 1 FROM root_conversations WHERE 1{} AND (creation_order, conversation_ref) < (?, ?)", scope_clause(participant_actor_ref, archived, self.owner_context_filter.as_ref()));
        let mut params = scope_params(
            participant_actor_ref,
            archived,
            self.owner_context_filter.as_ref(),
        );
        params.extend(summary_key_params(key));
        if let Some(before) = before {
            sql.push_str(" AND (creation_order, conversation_ref) > (?, ?)");
            params.extend(summary_key_params(before));
        }
        sql.push(')');
        let result = self
            .connection
            .query_row(&sql, params_from_iter(params), |row| row.get(0));
        let exists = result?;
        self.ensure_active()?;
        Ok(exists)
    }

    fn query_summaries<P: rusqlite::Params>(
        &self,
        sql: &str,
        params: P,
    ) -> CwpdbResult<Vec<SummaryWindowRow>> {
        let records = {
            let mut statement = self.connection.prepare(sql)?;
            let rows = statement.query_map(params, |row| {
                Ok((
                    SummaryWindowRow {
                        canonical_preview: false,
                        key: SummaryWindowKey {
                            creation_order: row.get(0)?,
                            conversation_ref: row.get(1)?,
                        },
                        title: row.get(2)?,
                        archived: row.get(3)?,
                        last_message_ref: row.get(4)?,
                        last_message_preview: row.get(5)?,
                        completed_work_refs: decode_summary_blob(&row.get::<_, Vec<u8>>(6)?)?,
                        revision: row.get::<_, i64>(7)?.to_string(),
                        completed_work: decode_summary_blob(&row.get::<_, Vec<u8>>(8)?)?,
                    },
                    row.get::<_, Option<String>>(9)?,
                ))
            })?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        let mut summaries = Vec::with_capacity(records.len());
        for (mut summary, timeline_root) in records {
            if let Some(timeline_root) = timeline_root {
                let last_message =
                    self.canonical_last_message_ref(&summary.key.conversation_ref, &timeline_root)?;
                summary.last_message_ref = last_message;
                summary.last_message_preview = None;
                summary.canonical_preview = true;
                let completed =
                    self.canonical_completed_work(&summary.key.conversation_ref, &timeline_root)?;
                summary.completed_work_refs = completed
                    .iter()
                    .map(|work| work.invocation_ref.clone())
                    .collect();
                summary.completed_work = completed.last().cloned().into_iter().collect();
                summary.revision = format!(
                    "{}:{}",
                    self.flat_timeline_revision(&timeline_root)?,
                    summary.revision
                );
            }
            summaries.push(summary);
        }
        self.ensure_active()?;
        Ok(summaries)
    }
}

fn decode_summary_blob<T: for<'de> Deserialize<'de>>(payload: &[u8]) -> rusqlite::Result<T> {
    serde_json::from_slice(payload).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            payload.len(),
            rusqlite::types::Type::Blob,
            Box::new(error),
        )
    })
}

const SUMMARY_SELECT: &str = "SELECT creation_order, conversation_ref, title, archived, last_message_ref, last_message_preview, completed_work_refs_json, revision, CAST((SELECT json_group_array(json_object('invocation_ref',ref,'sequence',seq)) FROM (SELECT r.canonical_ref AS ref,e.position AS seq FROM edges e JOIN ref_bindings r ON r.ref_id=e.target_id WHERE e.source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=root_conversations.conversation_ref) AND e.kind='completed_invocation' ORDER BY e.position DESC LIMIT 1)) AS BLOB), (SELECT timeline_connection_ref FROM conversations WHERE conversation_ref=root_conversations.conversation_ref) FROM root_conversations WHERE 1";
const ORDER_DESC: &str = "ORDER BY creation_order DESC";
const ORDER_ASC: &str = "ORDER BY creation_order ASC";
const PARTICIPANT_CLAUSE: &str = " AND EXISTS (
 SELECT 1 FROM edges participant
 JOIN ref_bindings actor ON actor.ref_id=participant.target_id
 WHERE participant.source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=root_conversations.conversation_ref)
 AND participant.kind='participant' AND actor.canonical_ref=?)";

fn scope_clause(
    participant_actor_ref: Option<&str>,
    archived: Option<bool>,
    context: Option<&(String, bool)>,
) -> String {
    let mut clause = participant_actor_ref.map_or(String::new(), |_| PARTICIPANT_CLAUSE.into());
    if archived.is_some() {
        clause.push_str(" AND root_conversations.archived=?");
    }
    if let Some((owner, include_legacy)) = context {
        clause.push_str(if owner.is_empty() && *include_legacy {
            " AND root_conversations.owner_context_ref IS NULL"
        } else if *include_legacy {
            " AND (root_conversations.owner_context_ref=? OR root_conversations.owner_context_ref IS NULL)"
        } else {
            " AND root_conversations.owner_context_ref=?"
        });
    }
    clause
}

fn scope_params(
    participant_actor_ref: Option<&str>,
    archived: Option<bool>,
    context: Option<&(String, bool)>,
) -> Vec<SqlValue> {
    let mut params = participant_actor_ref
        .map(|actor_ref| vec![SqlValue::Text(actor_ref.into())])
        .unwrap_or_default();
    if let Some(archived) = archived {
        params.push(SqlValue::Integer(i64::from(archived)));
    }
    if let Some((reference, include_legacy)) = context {
        if !(reference.is_empty() && *include_legacy) {
            params.push(SqlValue::Text(reference.clone()));
        }
    }
    params
}

fn summary_key_params(key: &SummaryWindowKey) -> [SqlValue; 2] {
    [
        SqlValue::Integer(key.creation_order),
        SqlValue::Text(key.conversation_ref.clone()),
    ]
}

fn append_summary_bounds(
    sql: &mut String,
    params: &mut Vec<SqlValue>,
    after: Option<&SummaryWindowKey>,
    before: Option<&SummaryWindowKey>,
) {
    if let Some(after) = after {
        sql.push_str(" AND (creation_order, conversation_ref) < (?, ?)");
        params.extend(summary_key_params(after));
    }
    if let Some(before) = before {
        sql.push_str(" AND (creation_order, conversation_ref) > (?, ?)");
        params.extend(summary_key_params(before));
    }
}

fn validate_window(window: SummaryWindow<'_>) -> CwpdbResult<()> {
    if window.first.is_none() && window.last.is_none() {
        return invalid("conversation page requires first or last");
    }
    if let Some(first) = window.first {
        validate_page_size(first)?;
    }
    if let Some(last) = window.last {
        validate_page_size(last)?;
    }
    Ok(())
}

fn sql_take(count: usize) -> CwpdbResult<i64> {
    i64::try_from(
        count
            .checked_add(1)
            .ok_or_else(|| invalid_error("conversation page size overflow"))?,
    )
    .map_err(Into::into)
}

fn validate_page_size(count: usize) -> CwpdbResult<()> {
    if count > MAX_CONVERSATION_SUMMARY_PAGE_SIZE {
        return invalid(format!(
            "conversation page size must not exceed {MAX_CONVERSATION_SUMMARY_PAGE_SIZE}"
        ));
    }
    Ok(())
}

fn key_tuple(key: &SummaryWindowKey) -> (i64, &str) {
    (key.creation_order, &key.conversation_ref)
}
