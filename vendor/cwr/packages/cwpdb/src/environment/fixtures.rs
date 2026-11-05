//! Typed environment-only retained-state fixtures. No SQL or handles cross this API.
use super::*;
use rusqlite::{params, OptionalExtension};
use serde_json::json;
#[derive(Clone, Copy)]
pub enum Relationship {
    Entry,
    Cleanup,
}
impl Relationship {
    fn kind(self) -> &'static str {
        match self {
            Self::Entry => MEMBERSHIP_KIND,
            Self::Cleanup => CLEANUP_EDGE_KIND,
        }
    }
}
pub enum Corruption<'a> {
    FutureNodeSchema(&'a str),
    FutureRelationshipSchema {
        workspace: &'a str,
        relationship: Relationship,
    },
    RelationshipIdentity {
        workspace: &'a str,
        relationship: Relationship,
    },
}
enum RestoreKind {
    NodeSchema,
    EdgeSchema,
    EdgeIdentity,
}
pub struct Restore {
    kind: RestoreKind,
    selector: String,
    relationship: Option<Relationship>,
    original: i64,
}
impl EnvironmentStore {
    pub fn fixture_seed_entry(
        &self,
        workspace: &str,
        reference: &str,
        value: &EntryValue,
    ) -> CwpdbResult<()> {
        self.store
            .lock()
            .map_err(|_| "environment store lock poisoned")?
            .graph_transaction(|tx| {
                crate::put_node_with_schema(
                    tx,
                    reference,
                    ENTRY_KIND,
                    ENTRY_SCHEMA,
                    &json!({"record":value}),
                )?;
                crate::put_edge(tx, workspace, MEMBERSHIP_KIND, reference, 0, &json!({}))?;
                Ok(())
            })
    }
    pub fn fixture_seed_unrelated_entry(
        &self,
        workspace: &str,
        reference: &str,
        value: &EntryValue,
    ) -> CwpdbResult<()> {
        self.store
            .lock()
            .map_err(|_| "environment store lock poisoned")?
            .graph_transaction(|tx| {
                crate::put_node_with_schema(
                    tx,
                    reference,
                    ENTRY_KIND,
                    ENTRY_SCHEMA,
                    &json!({"record":value}),
                )?;
                crate::put_edge(tx, workspace, "unrelated", reference, 0, &json!({}))?;
                Ok(())
            })
    }
    pub fn fixture_seed_missing_member(&self, workspace: &str, reference: &str) -> CwpdbResult<()> {
        self.store
            .lock()
            .map_err(|_| "environment store lock poisoned")?
            .graph_transaction(|tx| {
                crate::put_edge(tx, workspace, MEMBERSHIP_KIND, reference, 0, &json!({}))?;
                Ok(())
            })
    }
    pub fn fixture_seed_wrong_kind_member(
        &self,
        workspace: &str,
        reference: &str,
    ) -> CwpdbResult<()> {
        self.store
            .lock()
            .map_err(|_| "environment store lock poisoned")?
            .graph_transaction(|tx| {
                crate::put_node_with_schema(tx, reference, "other", ENTRY_SCHEMA, &json!({}))?;
                crate::put_edge(tx, workspace, MEMBERSHIP_KIND, reference, 0, &json!({}))?;
                Ok(())
            })
    }
    pub fn fixture_entry_record(&self, reference: &str) -> CwpdbResult<Option<(Value, i64)>> {
        self.store.lock().map_err(|_|"environment store lock poisoned")?.graph_read(|db|{
   let row:Option<(String,i64)>=db.query_row("SELECT json(n.props),n.revision FROM nodes n JOIN ref_bindings r ON r.ref_id=n.ref_id WHERE r.canonical_ref=?1 AND n.kind=?2",params![reference,ENTRY_KIND],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
   row.map(|(value,revision)|Ok((serde_json::from_str(&value)?,revision))).transpose()
  })
    }
    pub fn fixture_retired_entry_binding(&self, reference: &str) -> CwpdbResult<bool> {
        self.store.lock().map_err(|_|"environment store lock poisoned")?.graph_read(|db|Ok(db.query_row("SELECT EXISTS(SELECT 1 FROM ref_bindings WHERE canonical_ref=?1 AND claim_kind=?2)",params![reference,format!("node:{ENTRY_KIND}")],|r|r.get(0))?))
    }
    pub fn fixture_add_unknown_entry_property(&self, reference: &str) -> CwpdbResult<()> {
        self.store.lock().map_err(|_|"environment store lock poisoned")?.graph_transaction(|tx|{
   let changed=tx.execute("UPDATE nodes SET props=jsonb_set(props,'$.record.unexpected',jsonb('true')),revision=revision+1 WHERE ref_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) AND kind=?2",params![reference,ENTRY_KIND])?;
   if changed!=1{return Err("environment fixture entry missing".into());}Ok(())
  })
    }
    pub fn fixture_corrupt(&self, corruption: Corruption<'_>) -> CwpdbResult<Restore> {
        self.store.lock().map_err(|_|"environment store lock poisoned")?.graph_transaction(|tx|{
   let unknown=crate::bind(tx,"cwp://example/unsupported/environment-fixture")?;
   match corruption {
    Corruption::FutureNodeSchema(reference)=>{
     let original=tx.query_row("SELECT schema_id FROM nodes WHERE ref_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) AND kind IN (?2,?3,?4)",params![reference,WORKSPACE_KIND,ENTRY_KIND,CLEANUP_KIND],|r|r.get(0))?;
     tx.execute("UPDATE nodes SET schema_id=?1 WHERE ref_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?2)",params![unknown,reference])?;
     Ok(Restore{kind:RestoreKind::NodeSchema,selector:reference.into(),relationship:None,original})
    }
    Corruption::FutureRelationshipSchema{workspace,relationship}|Corruption::RelationshipIdentity{workspace,relationship}=>{
     let identity=matches!(corruption,Corruption::RelationshipIdentity{..});
     let (schema,id):(i64,i64)=tx.query_row("SELECT schema_id,ref_id FROM edges WHERE source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) AND kind=?2",params![workspace,relationship.kind()],|r|Ok((r.get(0)?,r.get(1)?)))?;
     let sql=if identity{"UPDATE edges SET ref_id=?1 WHERE source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?2) AND kind=?3"}else{"UPDATE edges SET schema_id=?1 WHERE source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?2) AND kind=?3"};
     tx.execute(sql,params![unknown,workspace,relationship.kind()])?;
     Ok(Restore{kind:if identity{RestoreKind::EdgeIdentity}else{RestoreKind::EdgeSchema},selector:workspace.into(),relationship:Some(relationship),original:if identity{id}else{schema}})
    }
   }
  })
    }
    pub fn fixture_restore(&self, restore: Restore) -> CwpdbResult<()> {
        self.store.lock().map_err(|_|"environment store lock poisoned")?.graph_transaction(|tx|{
   match restore.kind {
    RestoreKind::NodeSchema=>{tx.execute("UPDATE nodes SET schema_id=?1 WHERE ref_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?2)",params![restore.original,restore.selector])?;}
    kind=>{
     let sql=if matches!(kind,RestoreKind::EdgeIdentity){"UPDATE edges SET ref_id=?1 WHERE source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?2) AND kind=?3"}else{"UPDATE edges SET schema_id=?1 WHERE source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?2) AND kind=?3"};
     tx.execute(sql,params![restore.original,restore.selector,restore.relationship.ok_or("invalid fixture restoration")?.kind()])?;
    }
   }Ok(())
  })
    }
    pub fn fixture_legacy_identity_layout(&self) -> CwpdbResult<()> {
        self.store.lock().map_err(|_|"environment store lock poisoned")?.graph_transaction(|tx|{
   tx.execute_batch("DROP TRIGGER ref_bindings_no_canonical_update;DROP TRIGGER ref_bindings_no_ref_id_update;DROP TRIGGER ref_bindings_no_disposable_downgrade;DROP TRIGGER ref_bindings_no_durable_delete;DROP INDEX ref_bindings_disposable;ALTER TABLE ref_bindings DROP COLUMN disposable;CREATE TRIGGER ref_bindings_no_update BEFORE UPDATE ON ref_bindings BEGIN SELECT RAISE(ABORT,'reference bindings are immutable'); END;CREATE TRIGGER ref_bindings_no_delete BEFORE DELETE ON ref_bindings BEGIN SELECT RAISE(ABORT,'reference bindings are immutable'); END;")?;Ok(())
  })
    }
}

impl EnvironmentStore {
    pub fn fixture_reject_membership_writes(&self) -> CwpdbResult<()> {
        self.store.lock().map_err(|_|"environment store lock poisoned")?.graph_transaction(|tx| {
            tx.execute_batch("CREATE TRIGGER fixture_environment_membership_failure BEFORE INSERT ON edges WHEN NEW.kind='environment_entry' BEGIN SELECT RAISE(ABORT,'injected environment membership failure'); END;")?; Ok(())
        })
    }
    pub fn fixture_allow_membership_writes(&self) -> CwpdbResult<()> {
        self.store
            .lock()
            .map_err(|_| "environment store lock poisoned")?
            .graph_transaction(|tx| {
                tx.execute_batch("DROP TRIGGER fixture_environment_membership_failure;")?;
                Ok(())
            })
    }
    pub fn fixture_contains_secret(&self, secret: &str) -> CwpdbResult<bool> {
        if secret.is_empty() {
            return Err("nonempty fixture secret required".into());
        }
        let path = self
            .store
            .lock()
            .map_err(|_| "environment store lock poisoned")?
            .graph_read(|db| Ok(db.path().map(str::to_owned)))?;
        let Some(path) = path else { return Ok(false) };
        for suffix in ["", "-wal", "-shm"] {
            match std::fs::read(format!("{path}{suffix}")) {
                Ok(bytes) => {
                    if bytes
                        .windows(secret.len())
                        .any(|window| window == secret.as_bytes())
                    {
                        return Ok(true);
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        Ok(false)
    }
}
