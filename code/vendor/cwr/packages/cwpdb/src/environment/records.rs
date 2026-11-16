use super::*;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::json;

pub(super) fn read_entry(
    db: &Connection,
    workspace: &str,
    reference: &str,
) -> CwpdbResult<Option<Entry>> {
    let value:Option<(i64,String)>=db.query_row("SELECT n.revision,json(n.props) FROM edges e JOIN nodes n ON n.ref_id=e.target_id JOIN ref_bindings s ON s.ref_id=n.schema_id WHERE e.source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) AND e.kind=?2 AND e.target_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?3) AND n.kind=?4 AND s.canonical_ref=?5",params![workspace,MEMBERSHIP_KIND,reference,ENTRY_KIND,ENTRY_SCHEMA],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
    value
        .map(|(revision, value)| {
            let props: Value = serde_json::from_str(&value)?;
            if props
                .as_object()
                .is_none_or(|object| object.len() != 1 || !object.contains_key("record"))
            {
                return Err("Environment entry has an unknown format.".into());
            }
            let value: EntryValue = serde_json::from_value(
                props
                    .get("record")
                    .cloned()
                    .ok_or("Missing environment record.")?,
            )?;
            validation::validate_entry(&value)?;
            Ok(Entry {
                reference: reference.into(),
                revision,
                value,
            })
        })
        .transpose()
}
impl EnvironmentStore {
    pub fn entry(&self, workspace: &str, reference: &str) -> CwpdbResult<Option<Entry>> {
        self.store
            .lock()
            .map_err(|_| "environment store lock poisoned")?
            .graph_read(|db| read_entry(db, workspace, reference))
    }
    pub fn snapshot(&self, workspace: &str) -> CwpdbResult<Snapshot> {
        self.store.lock().map_err(|_|"environment store lock poisoned")?.graph_read(|db|{
   let reader=crate::edge_read::EdgeReader::new(db);
   let kinds=[crate::edge_read::RelationshipKind::Unordered(MEMBERSHIP_KIND)];
   let mut continuation=None;let mut entries=Vec::new();
   loop {
    let page=reader.query_edges(workspace,crate::edge_read::EdgeQuery{direction:crate::edge_read::Direction::Outgoing,kinds:&kinds,page_size:128,continuation:continuation.as_ref()})?;
    for edge in page.items {if let Some(entry)=read_entry(db,workspace,&edge.target_ref)?{entries.push(entry);}}
    continuation=page.continuation;if page.complete{break;}
   }
   let cleanup_pending:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM edges e JOIN nodes n ON n.ref_id=e.target_id WHERE e.source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) AND e.kind=?2 AND n.kind=?3)",params![workspace,CLEANUP_EDGE_KIND,CLEANUP_KIND],|r|r.get(0))?;
   Ok(Snapshot{entries,cleanup_pending})
  })
    }
    pub fn ensure_unique_name(
        &self,
        workspace: &str,
        name: &str,
        except: Option<&str>,
    ) -> CwpdbResult<()> {
        self.store
            .lock()
            .map_err(|_| "environment store lock poisoned")?
            .graph_read(|db| unique_name(db, workspace, name, except))
    }
    pub fn cleanup_candidates(&self) -> CwpdbResult<Vec<Cleanup>> {
        self.store.lock().map_err(|_|"environment store lock poisoned")?.graph_read(|db|{
   let mut statement=db.prepare_cached("SELECT r.canonical_ref,json_extract(n.props,'$.record.credential_id') FROM nodes n JOIN ref_bindings r ON r.ref_id=n.ref_id WHERE n.kind=?1")?;
   let rows=statement.query_map([CLEANUP_KIND],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))?.collect::<Result<Vec<_>,_>>()?;
   rows.into_iter().map(|(reference,credential_id)|{
    let in_use:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM nodes WHERE kind=?1 AND json_extract(props,'$.record.credential_id')=?2)",params![ENTRY_KIND,credential_id],|r|r.get(0))?;
    Ok(Cleanup{reference,credential_id,in_use})
   }).collect()
  })
    }
}
pub(super) fn unique_name(
    db: &Connection,
    workspace: &str,
    name: &str,
    except: Option<&str>,
) -> CwpdbResult<()> {
    let duplicate:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM edges e JOIN nodes n ON n.ref_id=e.target_id JOIN ref_bindings r ON r.ref_id=n.ref_id WHERE e.source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) AND e.kind=?2 AND n.kind=?3 AND json_extract(n.props,'$.record.name')=?4 AND (?5 IS NULL OR r.canonical_ref!=?5))",params![workspace,MEMBERSHIP_KIND,ENTRY_KIND,name,except],|r|r.get(0))?;
    if duplicate {
        return Err("This workspace already has an entry with that name.".into());
    }
    Ok(())
}
pub(super) fn cleanup_ref(id: &str) -> String {
    format!("cwp://org.coworkerprotocol.local/wlp/credential-cleanups/{id}")
}
pub(super) fn put_cleanup(
    tx: &rusqlite::Transaction<'_>,
    workspace: &str,
    id: &str,
) -> CwpdbResult<()> {
    let reference = cleanup_ref(id);
    crate::put_node_with_schema(
        tx,
        &reference,
        CLEANUP_KIND,
        CLEANUP_SCHEMA,
        &json!({"record":{"credential_id":id,"action":"delete"}}),
    )?;
    crate::put_edge(tx, workspace, CLEANUP_EDGE_KIND, &reference, 0, &json!({}))?;
    Ok(())
}
