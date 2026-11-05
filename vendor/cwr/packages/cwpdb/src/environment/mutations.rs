use super::*;
use rusqlite::OptionalExtension;
use serde_json::json;
fn stale<T>() -> CwpdbResult<T> {
    Err("Workspace settings changed. Reload before saving.".into())
}
impl EnvironmentStore {
    pub fn reserve_cleanup(&self, workspace: &str, id: &str) -> CwpdbResult<()> {
        self.store
            .lock()
            .map_err(|_| "environment store lock poisoned")?
            .graph_transaction(|tx| records::put_cleanup(tx, workspace, id))
    }
    pub fn finish_cleanup(&self, id: &str) -> CwpdbResult<()> {
        self.store
            .lock()
            .map_err(|_| "environment store lock poisoned")?
            .graph_transaction(|tx| crate::remove(tx, &records::cleanup_ref(id)))
    }
    pub fn save(&self, request: Save<'_>) -> CwpdbResult<()> {
        validation::validate_entry(request.value)?;
        self.store.lock().map_err(|_|"environment store lock poisoned")?.graph_transaction(|tx|{
   let current=records::read_entry(tx,request.workspace,request.reference)?;
   if current.as_ref().map(|entry|entry.revision)!=request.expected_revision{return stale();}
   let actual:Option<i64>=tx.query_row("SELECT revision FROM nodes WHERE ref_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1)",[request.reference],|r|r.get(0)).optional()?;
   if actual!=request.expected_revision{return stale();}
   if let Some(current)=&current {if current.value.kind!=request.value.kind{return stale();}}
   records::unique_name(tx,request.workspace,&request.value.name,Some(request.reference))?;
   let props=json!({"record":request.value});
   let previous_credential=current.as_ref().and_then(|entry|entry.value.credential_id.clone());
   match current {
    Some(current)=>{crate::replace_entity(tx,request.reference,ENTRY_KIND,ENTRY_SCHEMA,&crate::ref_read::RefRevision::from_storage_counter(current.revision),&props)?;}
    None=>{crate::insert_entity(tx,request.reference,ENTRY_KIND,ENTRY_SCHEMA,&props)?;}
   }
   crate::put_edge(tx,request.workspace,MEMBERSHIP_KIND,request.reference,0,&json!({}))?;
   if let Some(old)=previous_credential.as_deref().filter(|old|Some(*old)!=request.value.credential_id.as_deref()){records::put_cleanup(tx,request.workspace,old)?;}
   if let Some(id)=&request.value.credential_id {crate::remove(tx,&records::cleanup_ref(id))?;}
   Ok(())
  })
    }
    pub fn remove(
        &self,
        workspace: &str,
        reference: &str,
        expected_revision: i64,
    ) -> CwpdbResult<()> {
        self.store
            .lock()
            .map_err(|_| "environment store lock poisoned")?
            .graph_transaction(|tx| {
                let current = records::read_entry(tx, workspace, reference)?
                    .ok_or("This environment entry no longer exists.")?;
                if current.revision != expected_revision {
                    return stale();
                }
                crate::remove(tx, reference)?;
                if let Some(id) = current.value.credential_id {
                    records::put_cleanup(tx, workspace, &id)?;
                }
                Ok(())
            })
    }
}
