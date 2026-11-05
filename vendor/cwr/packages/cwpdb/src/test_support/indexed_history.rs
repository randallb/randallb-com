//! Finite retained history construction for downstream protocol compatibility checks.
use crate::{CwpdbResult, NativeStore};
use serde_json::{json, Value};
use std::{
    cell::RefCell,
    path::{Path, PathBuf},
};

pub enum RecordKind {
    Message,
    Activity,
}
pub enum AnchorKind {
    Message,
    Turn,
}
enum Change {
    Header {
        reference: String,
        position: i64,
        title: String,
        parent: Option<String>,
        revision: i64,
    },
    Record {
        kind: RecordKind,
        position: i64,
        payload: Value,
        visible: bool,
    },
    Anchor {
        position: i64,
        kind: AnchorKind,
        reference: String,
    },
    PublicActivity {
        position: i64,
        payload: Value,
    },
    Reply {
        message: String,
        parent: String,
    },
}
/// An atomic batch of named fixture operations. No native transaction escapes.
pub struct IndexedFixture {
    path: PathBuf,
    runtime: String,
    conversation: String,
    changes: RefCell<Vec<Change>>,
}
impl IndexedFixture {
    pub fn new(path: &Path, runtime: &str, conversation: &str) -> Self {
        Self {
            path: path.into(),
            runtime: runtime.into(),
            conversation: conversation.into(),
            changes: RefCell::new(Vec::new()),
        }
    }
    pub fn conversation(
        &self,
        reference: &str,
        position: i64,
        title: &str,
        parent: Option<&str>,
        revision: i64,
    ) {
        self.changes.borrow_mut().push(Change::Header {
            reference: reference.into(),
            position,
            title: title.into(),
            parent: parent.map(str::to_owned),
            revision,
        });
    }
    pub fn record(&self, kind: RecordKind, position: i64, payload: &Value, visible: bool) {
        self.changes.borrow_mut().push(Change::Record {
            kind,
            position,
            payload: payload.clone(),
            visible,
        });
    }
    pub fn anchor(&self, position: i64, kind: AnchorKind, reference: &str) {
        self.changes.borrow_mut().push(Change::Anchor {
            position,
            kind,
            reference: reference.into(),
        });
    }
    pub fn public_activity(&self, position: i64, payload: &Value) {
        self.changes.borrow_mut().push(Change::PublicActivity {
            position,
            payload: payload.clone(),
        });
    }
    pub fn reply(&self, message: &str, parent: &str) {
        self.changes.borrow_mut().push(Change::Reply {
            message: message.into(),
            parent: parent.into(),
        });
    }
    pub fn commit(self) -> CwpdbResult<()> {
        let mut store = NativeStore::open(&self.path)?;
        store.graph_transaction(|tx| {
            let root = format!("{}/cwpdb", self.runtime);
            let metadata = crate::node_props(tx, &root)?.ok_or("fixture runtime missing")?;
            if metadata["runtime_root_ref"] != self.runtime {
                return Err("fixture runtime identity mismatch".into());
            }
            for change in self.changes.into_inner() {
                apply_change(tx, &root, &self.conversation, change)?;
            }
            Ok(())
        })
    }
}
pub fn commit_revision(
    path: &Path,
    runtime: &str,
    conversation: &str,
    revision: i64,
) -> CwpdbResult<()> {
    let mut store = NativeStore::open(path)?;
    store.graph_transaction(|tx| {
        let metadata =
            crate::node_props(tx, &format!("{runtime}/cwpdb"))?.ok_or("fixture runtime missing")?;
        if metadata["runtime_root_ref"] != runtime {
            return Err("fixture runtime identity mismatch".into());
        }
        crate::put_node(
            tx,
            conversation,
            "conversation",
            &json!({"commit_revision":revision}),
        )?;
        Ok(())
    })
}

fn apply_change(
    tx: &rusqlite::Transaction<'_>,
    root: &str,
    conversation: &str,
    change: Change,
) -> CwpdbResult<()> {
    match change {
        Change::Header {
            reference,
            position,
            title,
            parent,
            revision,
        } => {
            crate::put_node(
                tx,
                &reference,
                "conversation",
                &json!({"creation_order":position,"title":title,"commit_revision":revision}),
            )?;
            crate::put_edge(tx, root, "conversation", &reference, position, &json!({}))?;
            if let Some(parent) = parent {
                crate::put_edge(tx, &reference, "parent", &parent, 0, &json!({}))?;
            } else {
                crate::put_edge(
                    tx,
                    root,
                    "root_conversation",
                    &reference,
                    position,
                    &json!({}),
                )?;
            }
        }
        Change::Record {
            kind,
            position,
            payload,
            visible,
        } => {
            let kind = match kind {
                RecordKind::Message => "message",
                RecordKind::Activity => "activity",
            };
            let reference = payload[format!("{kind}_ref")]
                .as_str()
                .ok_or("fixture record identity missing")?;
            crate::put_node(tx, reference, kind, &json!({"record":payload}))?;
            crate::put_edge(tx, conversation, kind, reference, position, &json!({}))?;
            let message = (kind == "activity")
                .then(|| payload["event"]["message_ref"].as_str())
                .flatten();
            if let Some(message) = message {
                crate::put_edge(
                    tx,
                    reference,
                    "field:event.message_ref",
                    message,
                    0,
                    &json!({}),
                )?;
            }

            if visible {
                crate::put_edge(
                    tx,
                    conversation,
                    "timeline_message",
                    reference,
                    position,
                    &json!({}),
                )?;
            }
        }
        Change::Anchor {
            position,
            kind,
            reference,
        } => {
            let kind = match kind {
                AnchorKind::Message => "message",
                AnchorKind::Turn => "turn",
            };
            crate::put_edge(
                tx,
                conversation,
                "timeline_item",
                &reference,
                position,
                &json!({"item_kind":kind}),
            )?;
        }
        Change::PublicActivity { position, payload } => {
            let reference = payload["activity_ref"]
                .as_str()
                .ok_or("fixture activity identity missing")?;
            crate::put_edge(
                tx,
                conversation,
                "timeline_activity",
                reference,
                position,
                &json!({"record":payload}),
            )?;
            let edge = crate::derived_ref("timeline_activity", &[conversation, reference]);
            for (field, kind) in [
                ("invocation_ref", "timeline_invocation"),
                ("source_message_ref", "timeline_source"),
                ("parent_invocation_ref", "timeline_parent"),
            ] {
                if let Some(source) = payload["event"][field].as_str() {
                    crate::put_edge(tx, source, kind, &edge, position, &json!({}))?;
                }
            }
        }
        Change::Reply { message, parent } => {
            tx.execute("UPDATE nodes SET props=jsonb_set(props,'$.record.reply_to_message_ref',?2) WHERE kind='message' AND ref_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1)", rusqlite::params![message,parent])?;
        }
    }
    Ok(())
}
