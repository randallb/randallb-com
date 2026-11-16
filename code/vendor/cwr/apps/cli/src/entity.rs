//! Explicit local developer surface; does not start a CWR workspace runtime.
use cwr::{cwpdb::NativeStore, entities::EntityCatalog, CwrResult};
use std::{ffi::OsString, fs, path::Path};

pub const USAGE: &str = "usage: cwr entity types <graph.toml> | cwr entity create <graph.toml> <store.sqlite> <definition-ref> <entity-ref> <value.json> | cwr entity get <graph.toml> <store.sqlite> <definition-ref> <entity-ref>";

pub fn run(arguments: &[OsString]) -> CwrResult<()> {
    match arguments {
        [command, graph] if command == "types" => {
            let catalog = EntityCatalog::load(Path::new(graph))?;
            println!("{}", serde_json::to_string_pretty(&catalog.types())?);
        }
        [command, graph, database, definition, reference, value] if command == "create" => {
            let catalog = EntityCatalog::load(Path::new(graph))?;
            let value = serde_json::from_slice(&fs::read(value)?)?;
            let mut store = NativeStore::open(Path::new(database))?;
            let record = catalog.create(&mut store, text(definition)?, text(reference)?, &value)?;
            println!("{}", serde_json::to_string_pretty(&record)?);
        }
        [command, graph, database, definition, reference] if command == "get" => {
            let catalog = EntityCatalog::load(Path::new(graph))?;
            let store = NativeStore::open_read_only(Path::new(database))?;
            let record = catalog.get(&store, text(definition)?, text(reference)?)?;
            println!("{}", serde_json::to_string_pretty(&record)?);
        }
        _ => return Err(USAGE.into()),
    }
    Ok(())
}

fn text(value: &OsString) -> CwrResult<&str> {
    value.to_str().ok_or_else(|| "expected UTF-8 ref".into())
}
