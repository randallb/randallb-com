//! Isolated cwpDb experiment. Run with --help; no runtime storage path is changed.
#[path = "cwpdb_spike/baseline.rs"]
mod baseline;
#[path = "cwpdb_spike/bench.rs"]
mod bench;
#[cfg(test)]
#[path = "cwpdb_spike/binding_tests.rs"]
mod binding_tests;
#[path = "cwpdb_spike/bindings.rs"]
mod bindings;
#[path = "cwpdb_spike/demo.rs"]
mod demo;
#[path = "cwpdb_spike/domain.rs"]
mod domain;
#[path = "cwpdb_spike/model.rs"]
mod model;
#[path = "cwpdb_spike/queries.rs"]
mod queries;
#[path = "cwpdb_spike/store.rs"]
mod store;
#[cfg(test)]
#[path = "cwpdb_spike/tests.rs"]
mod tests;

use model::*;
use serde_json::{json, Value};
use std::path::Path;
use store::Store;

const USAGE: &str = "cwpDb spike — isolated SQLite graph storage

  demo NEW_DIRECTORY                   Run a checked conversation workflow
  bench NEW_DIRECTORY [1000,10000,...]  Compare with current CWR storage (default 1k,10k,100k)
  stats DATABASE                       Counts, schema, size, and query plans
  show DATABASE NODE_REF               Read a node as ordinary JSON
  timeline DATABASE CONVERSATION [after|before CURSOR_JSON]
  walk DATABASE REF KIND out|in [after|before CURSOR_JSON]
  append DATABASE CONVERSATION ACTOR TEXT [REPLY_REF]
  archive|restore DATABASE CONVERSATION
  move DATABASE CONVERSATION WORKSPACE
  rebuild DATABASE                     Rebuild derived workspace-message edges

Pages contain at most 50 records; pass the returned next_cursor as CURSOR_JSON.
Demo and benchmark refuse existing directories. All other commands require an
existing spike database. Schema URLs under /cwpdb/spike-2/ are experimental.";

fn main() {
    if let Err(error) = run() {
        eprintln!("cwpDb spike: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.is_empty() || args == ["--help"] || args == ["help"] {
        println!("{USAGE}");
        return Ok(());
    }
    let result = match args[0].as_str() {
        "demo" if args.len() == 2 => {
            prepare_parent(&args[1])?;
            demo::run(Path::new(&args[1]))?
        }
        "bench" if (2..=3).contains(&args.len()) => {
            let sizes = args
                .get(2)
                .map(String::as_str)
                .unwrap_or("1000,10000,100000")
                .split(',')
                .map(str::parse)
                .collect::<std::result::Result<Vec<usize>, _>>()?;
            ensure(
                sizes
                    .iter()
                    .collect::<std::collections::BTreeSet<_>>()
                    .len()
                    == sizes.len(),
                "benchmark sizes must be distinct",
            )?;
            prepare_parent(&args[1])?;
            bench::run(Path::new(&args[1]), &sizes)?
        }
        _ => database_command(&args)?,
    };
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}

fn prepare_parent(directory: &str) -> Result<()> {
    if let Some(parent) = Path::new(directory)
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)?;
    }
    Ok(())
}

fn database_command(args: &[String]) -> Result<Value> {
    ensure(args.len() >= 2, USAGE)?;
    let path = Path::new(&args[1]);
    ensure(
        path.is_file(),
        "database does not exist; start with demo NEW_DIRECTORY",
    )?;
    let mut graph = Store::open(path)?;
    match (args[0].as_str(), args.len()) {
        ("stats", 2) => Ok(
            json!({"statistics": graph.statistics()?, "query_plans": queries::plans(&graph.connection)?}),
        ),
        ("show", 3) => Ok(serde_json::to_value(store::node(
            &graph.connection,
            &args[2],
        )?)?),
        ("timeline", 3 | 5) => {
            let (cursor, order) = boundary(&args[3..])?;
            Ok(serde_json::to_value(queries::timeline(
                &graph.connection,
                &args[2],
                Window {
                    cursor: cursor.as_ref(),
                    limit: 50,
                    order,
                },
            )?)?)
        }
        ("walk", 5 | 7) => {
            let direction = match args[4].as_str() {
                "out" => Direction::Out,
                "in" => Direction::In,
                _ => return Err("direction must be out or in".into()),
            };
            let (cursor, order) = boundary(&args[5..])?;
            Ok(serde_json::to_value(queries::walk(
                &graph.connection,
                &args[2],
                &args[3],
                direction,
                Window {
                    cursor: cursor.as_ref(),
                    limit: 50,
                    order,
                },
            )?)?)
        }
        ("append", 5 | 6) => {
            let reference = format!("{ROOT}/messages/{}", uuid::Uuid::new_v4().simple());
            graph.append(&Append {
                message_ref: &reference,
                conversation: &args[2],
                author: &args[3],
                text: &args[4],
                reply: args.get(5).map(String::as_str),
            })?;
            Ok(json!({"message_ref": reference}))
        }
        ("archive" | "restore", 3) => {
            graph.archive(&args[2], args[0] == "archive")?;
            Ok(json!({"conversation": args[2], "archived": args[0] == "archive"}))
        }
        ("move", 4) => {
            graph.move_conversation(&args[2], &args[3])?;
            Ok(json!({"conversation": args[2], "workspace": args[3]}))
        }
        ("rebuild", 2) => Ok(json!({"rebuilt_edges": graph.rebuild_shortcuts()?})),
        _ => Err(USAGE.into()),
    }
}

fn boundary(args: &[String]) -> Result<(Option<Cursor>, Order)> {
    match args {
        [] => Ok((None, Order::Forward)),
        [mode, cursor] => {
            let order = match mode.as_str() {
                "after" => Order::Forward,
                "before" => Order::Backward,
                _ => return Err("expected after or before".into()),
            };
            Ok((Some(serde_json::from_str(cursor)?), order))
        }
        _ => Err(USAGE.into()),
    }
}
