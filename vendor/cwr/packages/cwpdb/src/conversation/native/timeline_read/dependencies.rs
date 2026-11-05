use super::*;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
pub(super) struct Dependencies<'a> {
    db: &'a Connection,
    conversation: &'a str,
    root: Option<&'a str>,
    control: &'a ReadControl,
    work: usize,
    bytes: usize,
    max_bytes: usize,
}
impl<'a> Dependencies<'a> {
    pub fn new(
        db: &'a Connection,
        conversation: &'a str,
        root: Option<&'a str>,
        control: &'a ReadControl,
        max_bytes: usize,
    ) -> Self {
        Self {
            db,
            conversation,
            root,
            control,
            work: 0,
            bytes: 0,
            max_bytes,
        }
    }
    fn take(&mut self) -> CwpdbResult<()> {
        self.control.check()?;
        self.work += 1;
        if self.work > 50000 {
            return Err("timeline dependency work limit exceeded".into());
        }
        Ok(())
    }
    fn charge(&mut self, record: &Value) -> CwpdbResult<()> {
        self.take()?;
        self.bytes = self
            .bytes
            .checked_add(serde_json::to_vec(record)?.len())
            .ok_or("timeline byte budget overflow")?;
        if self.bytes > self.max_bytes {
            return Err("timeline page exceeds byte budget".into());
        }
        Ok(())
    }
    fn work(&mut self, reference: &str, kind: &str) -> CwpdbResult<Vec<(i64, Value)>> {
        self.take()?;
        let sql="SELECT activity.position,json_extract(activity.props,'$.record') FROM edges lookup INDEXED BY edges_out JOIN edges activity ON activity.ref_id=lookup.target_id AND activity.kind='timeline_activity' WHERE lookup.source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) AND lookup.kind=?2 AND activity.source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?3) ORDER BY activity.position LIMIT 50001";
        let mut statement = self.db.prepare_cached(sql)?;
        let rows = statement.query_map(params![reference, kind, self.conversation], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            self.take()?;
            let (position, text) = row?;
            out.push((position, serde_json::from_str(&text)?));
        }
        Ok(out)
    }
    fn visible(&mut self, reference: &str, kind: &str) -> CwpdbResult<bool> {
        self.take()?;
        resource::visible(self.db, self.root, reference, kind)
    }
    fn message(&mut self, reference: &str) -> CwpdbResult<Option<(i64, Value)>> {
        self.take()?;
        if let Some(root) = self.root {
            let relation = pages::visibility::message_relation(true)
                .replace("AND json_type(node.props,'$.record.progress') IS NULL", "");
            let position: Option<i64> = self
                .db
                .query_row(
                    &format!("{relation} SELECT position FROM messages WHERE message_ref=?3"),
                    params![self.conversation, root, reference],
                    |r| r.get(0),
                )
                .optional()?;
            return position
                .map(|position| {
                    Ok(resource::message(self.db, self.conversation, reference)?
                        .map(|record| (position, record)))
                })
                .transpose()
                .map(Option::flatten);
        }
        let position:Option<i64>=self.db.query_row("SELECT position FROM edges WHERE source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) AND kind='timeline_message' AND target_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?2)",params![self.conversation,reference],|r|r.get(0)).optional()?;
        position
            .map(|position| {
                Ok(resource::message(self.db, self.conversation, reference)?
                    .map(|record| (position, record)))
            })
            .transpose()
            .map(Option::flatten)
    }
    fn recorded(&mut self, reference: &str) -> CwpdbResult<Option<Value>> {
        self.take()?;
        let row:Option<String>=self.db.query_row("SELECT json_extract(node.props,'$.record') FROM edges lookup INDEXED BY edges_in JOIN nodes node ON node.ref_id=lookup.source_id AND node.kind='activity' JOIN edges membership ON membership.target_id=node.ref_id AND membership.kind='activity' WHERE lookup.target_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) AND lookup.kind='field:event.message_ref' AND membership.source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?2) AND json_extract(node.props,'$.record.event.kind')='message_recorded' ORDER BY membership.position LIMIT 1",params![reference,self.conversation],|r|r.get(0)).optional()?;
        row.map(|text| serde_json::from_str(&text).map_err(Into::into))
            .transpose()
    }
    pub fn read(
        mut self,
        anchors: &[TimelineAnchorFacts],
    ) -> CwpdbResult<(Vec<Value>, Vec<Value>)> {
        let mut message_refs = VecDeque::new();
        let mut work_refs = VecDeque::new();
        let mut context_refs = VecDeque::new();
        for anchor in anchors {
            match anchor {
                TimelineAnchorFacts::Message { message_ref, .. } => {
                    message_refs.push_back(message_ref.clone())
                }
                TimelineAnchorFacts::Turn { invocation_ref, .. } => {
                    work_refs.push_back(invocation_ref.clone())
                }
            }
        }
        let mut messages = BTreeMap::new();
        let mut activities = BTreeMap::new();
        let mut seen_messages = BTreeSet::new();
        let mut seen_work = BTreeSet::new();
        let mut seen_context = BTreeSet::new();
        while !message_refs.is_empty() || !work_refs.is_empty() || !context_refs.is_empty() {
            self.take()?;
            while let Some(reference) = work_refs.pop_front() {
                self.take()?;
                if !seen_work.insert(reference.clone()) {
                    continue;
                }
                if !self.visible(&reference, "conversation_timeline_invocation")? {
                    continue;
                }
                for (_, record) in self.work(&reference, "timeline_parent")? {
                    if let Some(child) = record["event"]["invocation_ref"].as_str() {
                        work_refs.push_back(child.into());
                    }
                }
                for (_, activity) in self.work(&reference, "timeline_invocation")? {
                    enqueue_context(&activity, &mut message_refs, &mut context_refs);
                    let child = (activity["event"]["kind"] == "work_update")
                        .then(|| activity["event"]["activity"]["child_invocation_ref"].as_str())
                        .flatten();
                    work_refs.extend(child.map(str::to_owned));
                    let identity = activity["activity_ref"]
                        .as_str()
                        .ok_or("timeline activity identity missing")?
                        .to_owned();
                    if !activities.contains_key(&identity) {
                        self.charge(&activity)?;
                    }
                    activities.insert(identity, activity);
                }
            }
            while let Some(reference) = message_refs.pop_front() {
                self.take()?;
                if !seen_messages.insert(reference.clone()) {
                    continue;
                }
                let message = self.message(&reference)?;
                if self.root.is_some() && message.is_none() {
                    continue;
                }
                if let Some((position, message)) = message {
                    if let Some(reply) = message["reply_to_message_ref"].as_str() {
                        message_refs.push_back(reply.into());
                    }
                    if !messages.contains_key(&position) {
                        self.charge(&message)?;
                    }
                    messages.insert(position, message);
                }
                if let Some(activity) = self.recorded(&reference)? {
                    let identity = activity["activity_ref"]
                        .as_str()
                        .ok_or("recorded activity identity missing")?
                        .to_owned();
                    if !activities.contains_key(&identity) {
                        self.charge(&activity)?;
                    }
                    activities.insert(identity, activity);
                }
                let mut preferred = BTreeMap::<String, (i64, Value)>::new();
                for (position, activity) in self.work(&reference, "timeline_source")? {
                    let Some(invocation) = activity["event"]["invocation_ref"].as_str() else {
                        continue;
                    };
                    let priority = if activity["event"]["kind"] == "work_lifecycle" {
                        -1
                    } else {
                        position
                    };
                    if preferred
                        .get(invocation)
                        .is_none_or(|(old, _)| priority < *old)
                    {
                        preferred.insert(invocation.to_owned(), (priority, activity));
                    }
                }
                context_refs.extend(preferred.into_keys());
            }
            while let Some(reference) = context_refs.pop_front() {
                self.take()?;
                if seen_work.contains(&reference) || !seen_context.insert(reference.clone()) {
                    continue;
                }
                if !self.visible(&reference, "conversation_timeline_invocation")? {
                    continue;
                }
                let rows = self.work(&reference, "timeline_invocation")?;
                let selected = rows.into_iter().min_by_key(|(position, record)| {
                    if record["event"]["kind"] == "work_lifecycle" {
                        -1
                    } else {
                        *position
                    }
                });
                if let Some((_, mut activity)) = selected {
                    enqueue_context(&activity, &mut message_refs, &mut context_refs);
                    if activity["event"]["kind"] == "work_update" {
                        activity["event"]["text"] = Value::Null;
                        activity["event"]["activity"] = Value::Null;
                    }
                    let identity = activity["activity_ref"]
                        .as_str()
                        .ok_or("context activity identity missing")?
                        .to_owned();
                    if let std::collections::btree_map::Entry::Vacant(entry) =
                        activities.entry(identity)
                    {
                        self.charge(&activity)?;
                        entry.insert(activity);
                    }
                }
            }
        }
        let mut activities = activities.into_values().collect::<Vec<_>>();
        activities.sort_by(|a, b| {
            (
                a["occurred_at_unix_ms"].as_i64(),
                a["sequence"].as_u64(),
                a["activity_ref"].as_str(),
            )
                .cmp(&(
                    b["occurred_at_unix_ms"].as_i64(),
                    b["sequence"].as_u64(),
                    b["activity_ref"].as_str(),
                ))
        });
        self.control.check()?;
        Ok((messages.into_values().collect(), activities))
    }
}
fn enqueue_context(
    activity: &Value,
    messages: &mut VecDeque<String>,
    invocations: &mut VecDeque<String>,
) {
    let event = &activity["event"];
    if !matches!(
        event["kind"].as_str(),
        Some("work_lifecycle" | "work_update")
    ) {
        return;
    }
    if let Some(reference) = event["source_message_ref"].as_str() {
        messages.push_back(reference.into());
    }
    if let Some(reference) = event["parent_invocation_ref"].as_str() {
        invocations.push_back(reference.into());
    }
    if event["kind"] == "work_update" {
        for field in ["waiter_invocation_ref", "awaited_invocation_ref"] {
            if let Some(reference) = event["wait"][field].as_str() {
                invocations.push_back(reference.into());
            }
        }
    }
}
