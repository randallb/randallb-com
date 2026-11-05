use std::{hint::black_box, path::Path, time::Instant};

use rusqlite::{params, Connection};
use serde_json::{json, Value};

use super::{baseline, bindings, domain, model::*, queries, store::Store};

const CONVERSATIONS: usize = 100;
const ACTORS: usize = 32;
const SAMPLES: usize = 25;

pub fn run(directory: &Path, sizes: &[usize]) -> Result<Value> {
    ensure(
        !sizes.is_empty() && sizes.iter().all(|n| (100..=1_000_000).contains(n)),
        "sizes must be between 100 and 1000000",
    )?;
    std::fs::create_dir(directory)?;
    let mut results = Vec::new();
    for &count in sizes {
        eprintln!(
            "Benchmarking {count} messages (100 conversations, 32 actors, 1024-byte bodies)..."
        );
        results.push(measure(directory, count)?);
    }
    let report = json!({"schema_ref": schema("benchmark-report"), "sqlite_version": rusqlite::version(),
        "durability": {"journal_mode": "WAL", "synchronous": "FULL"},
        "samples": SAMPLES, "seed_batch_size": 100, "body_bytes": 1024,
        "scope": "Storage microbenchmark against CWR's current physical schema; baseline writes messages, timeline projections and summary. Includes canonical URL-to-ID lookup and expansion of returned URLs, with no binding cache. Excludes admission, activities, invocation scheduling and full runtime API costs. Warm reads; no speed threshold.",
        "results": results });
    std::fs::write(
        directory.join("benchmark.json"),
        serde_json::to_string_pretty(&report)?,
    )?;
    Ok(report)
}

fn measure(directory: &Path, count: usize) -> Result<Value> {
    let mut graph = Store::open(&directory.join(format!("graph-{count}.sqlite3")))?;
    let mut old = baseline::open(&directory.join(format!("current-{count}.sqlite3")))?;
    let workspace = reference("workspaces", 0);
    graph.create_workspace(&workspace)?;
    for id in 0..ACTORS {
        graph.create_actor(&reference("actors", id))?;
    }
    for id in 0..CONVERSATIONS {
        graph.create_conversation(
            &reference("conversations", id),
            &workspace,
            &format!("Conversation {id}"),
            i64::try_from(id)?,
        )?;
        baseline::conversation(&old, id)?;
    }
    let text = "x".repeat(1024);
    let graph_seed_ms = seed_graph(&mut graph, count, &text)?;
    let current_seed_ms = seed_current(&mut old, count, &text)?;
    let conversation = reference("conversations", 0);
    let graph_page = queries::timeline(&graph.connection, &conversation, Window::first(50))?;
    ensure(
        graph_page.items == baseline::page(&old, &conversation, -1, 50)?,
        "baseline and graph first pages differ",
    )?;
    if let Some(cursor) = graph_page.next_cursor.as_ref() {
        let interior = queries::timeline(
            &graph.connection,
            &conversation,
            Window {
                cursor: Some(cursor),
                limit: 50,
                order: Order::Forward,
            },
        )?;
        ensure(
            interior.items == baseline::page(&old, &conversation, cursor.position, 50)?,
            "baseline and graph interior pages differ",
        )?;
    }
    let actor = reference("actors", 0);
    let author_lookup = reverse(&graph.connection, &actor)?;
    ensure(
        author_lookup == baseline::by_author(&old, &actor)?,
        "baseline and graph author lookups differ",
    )?;
    let graph_read = sample(|| {
        black_box(queries::timeline(
            &graph.connection,
            &conversation,
            Window::first(50),
        )?);
        Ok(())
    })?;
    let current_read = sample(|| {
        black_box(baseline::page(&old, &conversation, -1, 50)?);
        Ok(())
    })?;
    let graph_reverse = sample(|| {
        black_box(reverse(&graph.connection, &actor)?);
        Ok(())
    })?;
    let current_reverse = sample(|| {
        black_box(baseline::by_author(&old, &actor)?);
        Ok(())
    })?;
    let mut id = count;
    let graph_write = sample_without_warmup(|| {
        with_input(id, &text, |input| graph.append(input))?;
        id += 1;
        Ok(())
    })?;
    id = count;
    let current_write = sample_without_warmup(|| {
        with_input(id, &text, |input| {
            baseline::append_committed(&mut old, input)
        })?;
        id += 1;
        Ok(())
    })?;
    let before_rebuild = graph.statistics()?;
    let started = Instant::now();
    let rebuilt = graph.rebuild_shortcuts()?;
    let rebuild_ms = started.elapsed().as_secs_f64() * 1000.0;
    let stats = graph.statistics()?;
    let current_bytes = baseline::bytes(&old)?;
    Ok(
        json!({"seed_messages": count, "final_messages": count+SAMPLES,
        "page_records": graph_page.items.len(), "author_lookup_records": author_lookup.len(),
        "graph": {"seed_ms": graph_seed_ms, "page_50": graph_read, "author_lookup_50": graph_reverse,
            "append_one_transaction": graph_write, "rebuild_shortcuts_ms": rebuild_ms, "rebuilt_edges": rebuilt,
            "statistics_before_rebuild": before_rebuild,
            "statistics": stats, "bytes_by_object": object_bytes(&graph.connection), "query_plans": queries::plans(&graph.connection)?},
        "current": {"seed_ms": current_seed_ms, "page_50": current_read, "author_lookup_50": current_reverse,
            "append_one_transaction": current_write, "database_bytes": current_bytes, "bytes_by_object": object_bytes(&old)},
        "equivalence_checks": ["first_page_contents", "interior_page_contents_when_present", "author_lookup_refs"] }),
    )
}

fn with_input<T>(id: usize, text: &str, run: impl FnOnce(&Append<'_>) -> Result<T>) -> Result<T> {
    run(&Append {
        message_ref: &reference("messages", id),
        conversation: &reference("conversations", id % CONVERSATIONS),
        author: &reference("actors", id % ACTORS),
        text,
        reply: (id >= CONVERSATIONS)
            .then(|| reference("messages", id - CONVERSATIONS))
            .as_deref(),
    })
}

fn seed_graph(graph: &mut Store, count: usize, text: &str) -> Result<f64> {
    let started = Instant::now();
    for start in (0..count).step_by(100) {
        graph.write(|tx| {
            for id in start..(start + 100).min(count) {
                with_input(id, text, |input| domain::append(tx, input))?;
            }
            Ok(())
        })?;
    }
    Ok(started.elapsed().as_secs_f64() * 1000.0)
}

fn seed_current(old: &mut Connection, count: usize, text: &str) -> Result<f64> {
    let started = Instant::now();
    for start in (0..count).step_by(100) {
        let tx = old.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        for id in start..(start + 100).min(count) {
            with_input(id, text, |input| baseline::append(&tx, input))?;
        }
        tx.commit()?;
    }
    Ok(started.elapsed().as_secs_f64() * 1000.0)
}

fn reverse(db: &Connection, actor: &str) -> Result<Vec<String>> {
    // Fixture messages are inserted in canonical-reference order. Local ID order
    // returns the same 50 refs here; it is not a general lexical URL ordering.
    let actor_id = bindings::resolve(db, actor)?;
    Ok(db
        .prepare_cached(
            "SELECT b.canonical_ref FROM edges e JOIN ref_bindings b ON b.ref_id=e.source_id
        WHERE e.target_id=?1 AND e.kind=?2 ORDER BY e.source_id LIMIT 50",
        )?
        .query_map(params![actor_id, AUTHOR], |r| r.get(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?)
}

fn sample(mut run: impl FnMut() -> Result<()>) -> Result<Value> {
    run()?;
    sample_without_warmup(run)
}

fn sample_without_warmup(mut run: impl FnMut() -> Result<()>) -> Result<Value> {
    let mut times = Vec::new();
    for _ in 0..SAMPLES {
        let started = Instant::now();
        run()?;
        times.push(started.elapsed().as_secs_f64() * 1_000_000.0);
    }
    times.sort_by(f64::total_cmp);
    Ok(json!({"median_us": times[SAMPLES/2], "p95_us": times[(SAMPLES*95).div_ceil(100)-1]}))
}

fn object_bytes(db: &Connection) -> Value {
    let result = (|| -> Result<Value> {
        let rows = db
            .prepare("SELECT name,sum(pgsize) FROM dbstat GROUP BY name ORDER BY name")?
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?
            .collect::<rusqlite::Result<std::collections::BTreeMap<_, _>>>()?;
        Ok(json!(rows))
    })();
    result.unwrap_or_else(|e| json!({"unavailable": e.to_string()}))
}
