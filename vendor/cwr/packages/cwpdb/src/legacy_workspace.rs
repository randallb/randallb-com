//! Fixed legacy Desktop workspace read shape. Callers cannot select SQL or
//! physical keys. Reads join one already-open, read-only source snapshot.
use crate::CwpdbResult;
use rusqlite::{Connection, OptionalExtension};
const ROW_LIMIT: usize = 10_000;
#[derive(Default)]
pub struct Data {
    pub instances: Vec<(String, String, String, Option<String>)>,
    pub versions: Vec<(String, String, String, String)>,
    pub active: Vec<(String, String)>,
    pub registry: Vec<(String, String)>,
}
#[derive(Debug)]
pub enum Error {
    NoLegacyTables,
    WorkLimit,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "legacy workspace read: {self:?}")
    }
}
impl std::error::Error for Error {}
fn table(db: &Connection, name: &str) -> CwpdbResult<bool> {
    Ok(db
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1",
            [name],
            |r| r.get::<_, i64>(0),
        )
        .optional()?
        .is_some())
}
pub fn read(db: &Connection) -> CwpdbResult<Data> {
    let mut data = Data::default();
    let mut present = false;
    if table(db, "blank_loop_instances_v1")? {
        present = true;
        data.instances=db.prepare("SELECT workspace,id,name,folders FROM blank_loop_instances_v1 ORDER BY workspace LIMIT 10001")?.query_map([],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)))?.collect::<Result<_,_>>()?;
    }
    if table(db, "blank_loop_versions_v1")? {
        present = true;
        data.versions=db.prepare("SELECT workspace,version,files,folders FROM blank_loop_versions_v1 ORDER BY workspace,version LIMIT 10001")?.query_map([],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)))?.collect::<Result<_,_>>()?;
    }
    if table(db, "blank_loop_active_v1")? {
        present = true;
        data.active = db
            .prepare(
                "SELECT workspace,version FROM blank_loop_active_v1 ORDER BY workspace LIMIT 10001",
            )?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<Result<_, _>>()?;
    }
    if table(db, "workloop_workspaces_v1")? {
        present = true;
        data.registry = db
            .prepare(
                "SELECT workspace,id FROM workloop_workspaces_v1 ORDER BY workspace LIMIT 10001",
            )?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<Result<_, _>>()?;
    }
    if !present {
        return Err(Error::NoLegacyTables.into());
    }
    if [
        data.instances.len(),
        data.versions.len(),
        data.active.len(),
        data.registry.len(),
    ]
    .into_iter()
    .sum::<usize>()
        > ROW_LIMIT
    {
        return Err(Error::WorkLimit.into());
    }
    Ok(data)
}
pub fn data_version(db: &Connection) -> CwpdbResult<i64> {
    Ok(db.query_row("PRAGMA data_version", [], |r| r.get(0))?)
}
