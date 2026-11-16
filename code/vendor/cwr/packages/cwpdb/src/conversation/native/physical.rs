use super::*;
use rusqlite::OptionalExtension;
pub(super) enum DatabaseSchema {
    New,
    Current,
}

// Compare SQLite's persistent catalog against the schema installed by the
// current cwpDb DDL. This reads only schema metadata, not retained graph rows.
// The in-memory reference is built once per process and never writes to the
// runtime database.
type GraphCatalog = BTreeMap<(String, String), String>;

fn graph_catalog(connection: &Connection) -> CwpdbResult<GraphCatalog> {
    let mut statement = connection
        .prepare("SELECT type, name, sql FROM sqlite_master WHERE name NOT GLOB 'sqlite_*'")?;
    let catalog = statement
        .query_map([], |row| Ok(((row.get(0)?, row.get(1)?), row.get(2)?)))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(catalog)
}

fn expected_graph_catalog() -> CwpdbResult<&'static GraphCatalog> {
    static EXPECTED: std::sync::OnceLock<GraphCatalog> = std::sync::OnceLock::new();
    if let Some(catalog) = EXPECTED.get() {
        return Ok(catalog);
    }
    let reference = Connection::open_in_memory()?;
    reference.execute_batch(crate::PHYSICAL_SCHEMA_SQL)?;
    let catalog = graph_catalog(&reference)?;
    let _ = EXPECTED.set(catalog);
    Ok(EXPECTED
        .get()
        .expect("the graph schema catalog was initialized"))
}

fn physical_schema_error(path: &Path, detail: &str) -> Box<dyn std::error::Error + Send + Sync> {
    Box::<dyn std::error::Error+Send+Sync>::from(format!(
        "{}: unsupported cwpDb physical schema ({detail}); convert the store explicitly or reset it",
        path.display()
    ))
}

fn normalized_schema_sql(sql: &str) -> String {
    // cwpDb's explicit legacy upgrade creates the same triggers without the
    // optional IF NOT EXISTS spelling used by first initialization.
    let mut normalized = String::with_capacity(sql.len());
    let mut chars = sql.chars().peekable();
    let mut in_literal = false;
    while let Some(character) = chars.next() {
        if character == '\'' {
            normalized.push(character);
            if in_literal && chars.peek() == Some(&'\'') {
                normalized.push(chars.next().expect("escaped quote is present"));
            } else {
                in_literal = !in_literal;
            }
        } else if in_literal {
            normalized.push(character);
        } else if !character.is_whitespace() {
            normalized.push(character.to_ascii_lowercase());
        }
    }
    normalized.replace("createtriggerifnotexists", "createtrigger")
}

pub(super) fn inspect_physical_schema(
    connection: &Connection,
    path: &Path,
) -> CwpdbResult<DatabaseSchema> {
    let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version != 0 {
        return Err(format!("{}: unsupported CWR runtime database schema version {version}; existing installations require a manual reset",path.display()).into());
    }
    let actual = graph_catalog(connection)?;
    if actual.is_empty() {
        let leftover: Option<(String, String)> = connection
            .query_row("SELECT type, name FROM sqlite_master LIMIT 1", [], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .optional()?;
        return match leftover {
            Some((kind, name)) => Err(physical_schema_error(
                path,
                &format!("unexpected {kind} {name}"),
            )),
            None => Ok(DatabaseSchema::New),
        };
    }
    let expected = expected_graph_catalog()?;
    // Existing large stores can predate the two schedule query indexes. They
    // are a known pair of optional accelerators, not a data-layout change.
    // Accept that exact older catalog without building indexes at startup.
    let missing_schedule_pair = ["nodes_schedule_due", "nodes_schedule_terminal"]
        .iter()
        .all(|name| !actual.contains_key(&("index".into(), (*name).into())));
    for (kind, name) in actual.keys() {
        if !expected.contains_key(&(kind.clone(), name.clone())) {
            return Err(physical_schema_error(
                path,
                &format!("unexpected {kind} {name}"),
            ));
        }
    }
    for (kind, name) in expected.keys() {
        if !actual.contains_key(&(kind.clone(), name.clone())) {
            if missing_schedule_pair
                && kind == "index"
                && (name == "nodes_schedule_due" || name == "nodes_schedule_terminal")
            {
                continue;
            }
            return Err(physical_schema_error(
                path,
                &format!("missing {kind} {name}"),
            ));
        }
    }
    for ((kind, name), sql) in &actual {
        if normalized_schema_sql(sql)
            != normalized_schema_sql(&expected[&(kind.clone(), name.clone())])
        {
            return Err(physical_schema_error(
                path,
                &format!("different definition for {kind} {name}"),
            ));
        }
    }
    Ok(DatabaseSchema::Current)
}
