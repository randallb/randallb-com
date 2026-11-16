# Run the cwpDb spike

The Rust CLI example exercises a conversation workflow using three generic
SQLite tables: reference bindings, nodes, and directed edges. Nodes and edges
have JSONB properties and integer keys. It remains an isolated experiment owned by cwpdb. Its legacy baseline payload is frozen locally and does not depend on CWR protocol types. CWR
now uses the [production cwpDb module](../../../cwr-core/docs/cwpdb.md), with stable derived
relationship identities.

From the checkout root:

```sh
direnv exec . cargo test --locked -p cwpdb --example cwpdb_spike
direnv exec . cargo build --locked --release -p cwpdb --example cwpdb_spike
./target/release/examples/cwpdb_spike demo tmp/cwpdb-integer-demo
./target/release/examples/cwpdb_spike bench tmp/cwpdb-integer-bench 1000,10000,100000
```

Choose a new output directory on each run. The CLI refuses to reuse a demo or
benchmark directory. It prints JSON and saves `demo.json` or `benchmark.json`
beside the SQLite files. It never discovers or resets installed runtime data.

The demo checks message authorship and replies, forward pagination, reverse
traversal, archive/restore membership, property preservation, failed-write
rollback, conversation moves, derived-edge rebuilds, and closing/reopening the
database. Its report includes the conversation, workspace and actor references
for subsequent commands:

```sh
./target/release/examples/cwpdb_spike --help
./target/release/examples/cwpdb_spike stats tmp/cwpdb-integer-demo/graph.sqlite3
./target/release/examples/cwpdb_spike show DATABASE NODE_REF
./target/release/examples/cwpdb_spike timeline DATABASE CONVERSATION_REF
./target/release/examples/cwpdb_spike walk DATABASE ACTOR_REF authored_by in
./target/release/examples/cwpdb_spike append DATABASE CONVERSATION_REF ACTOR_REF 'Hello'
./target/release/examples/cwpdb_spike archive DATABASE CONVERSATION_REF
./target/release/examples/cwpdb_spike restore DATABASE CONVERSATION_REF
```

Pass the returned `next_cursor` JSON to `timeline` or `walk` as
`after 'CURSOR_JSON'` or `before 'CURSOR_JSON'`. The CLI reads up to 50 records;
the Rust tests also cover tied positions and backward traversal. Cursor scope
includes the starting node, relationship kind and direction. Pages are ordered
by `(position, edge ref_id)`. The cursor carries the edge's canonical URL, which
resolves back to its fixed local ID. Callers should treat cursors as opaque
coordinates; tied positions follow local ID order, not URL spelling.

## Storage choices exercised

- Nodes and edges have independent CWP references and `cwp://` schema URLs.
  `/cwpdb/spike-2/schemas/` names experimental schemas, not published contracts.
  This binary expects a fresh spike-2 database; previous spike-1 files remain
  available to the original binary and are not migrated or reset.
- `ref_bindings(ref_id INTEGER PRIMARY KEY, canonical_ref TEXT UNIQUE)` maps
  every identity and schema URL to a database-local integer. Node/edge primary
  keys, endpoints, schema references, and their indexes use those integers. CLI
  inputs, outputs, and cursors use full URLs. Arbitrary JSON properties remain
  intact, including strings that happen to contain URLs.
- Bindings are inserted in the record's transaction, shared by repeated uses,
  and protected against update/deletion. They remain after a node or edge is
  removed, so committed IDs are never reassigned. Uncommitted bindings roll back
  with failed writes. There is no in-memory binding cache.
- Four generic edge indexes support membership checks, reverse traversal and
  ordered paging in both directions. There are no archive-specific indexes. The
  index pair for unordered traversal can return endpoints without reading the
  edge payload. The CLI reports SQLite's query plans.
- The spike chooses one edge per `(source, kind, target)`. Relationship
  cardinality beyond that is owned by its conversation operations. This is not
  yet a general multigraph API.
- `conversation -> workspace`, `conversation -> message`, `message -> author`,
  `message -> reply`, and `conversation -> archive collection` are
  authoritative. `message -> workspace` is a derived shortcut; a move updates it
  in the same transaction, and a rebuild replaces that relationship kind from
  its sources. This prototype generates new shortcut identities on each rebuild
  and retains the old bindings. Repeated rebuilds therefore grow the dictionary;
  the benchmark includes that cost and reports size before and after one
  rebuild.
- Writes use immediate transactions, foreign keys, WAL and `synchronous=FULL`.
  Message position allocation happens inside the write transaction. Node
  property patches check the revision and preserve unmentioned properties.
  Nested values supplied by a patch replace that property; this is not a deep
  merge. Stored JSONB is rendered as ordinary JSON at the CLI boundary.

SQLite's JSONB encoding is internal to SQLite and distinct from PostgreSQL
JSONB. See the
[SQLite JSON documentation](https://www.sqlite.org/json1.html#jsonb).

## Interpreting the comparison

The benchmark creates fresh graph and pre-cutover CWR-schema databases for each
size. Its frozen `baseline_schema.sql` preserves the original schema after the
runtime cutover. It seeds 100 conversations, 32 actors, 1 KiB message bodies,
full-length CWP references and reply chains. Graph messages have membership,
author, reply and workspace-shortcut edges. Both stores use WAL/FULL, seed in
batches of 100, then measure 25 separate single-message transactions, pages of
up to 50 messages and author lookups of up to 50 references. The 1,000-message
fixture has only 10 messages per conversation; reports include the returned
counts. Reports contain median/p95 microseconds, seed time, shortcut-rebuild
time, checkpointed file size including indexes, and per-object sizes where
SQLite provides `dbstat`. The first/interior message pages and author results
must agree.

Integer-layout timings include incoming URL resolution, binding inserts on
writes, and expansion of returned URLs through joins. The author lookup orders
by local source ID; the synthetic messages are inserted in canonical-reference
order, so its first 50 results match the baseline's URL order. This does not
establish a general index for sorting arbitrary URLs. File sizes include the
binding table, its unique index, all graph indexes, and retained old bindings.

This is a storage microbenchmark. Current-schema writes include canonical
messages, timeline message/item projections and conversation summaries. They do
not include activities, admission, invocation scheduling, provider work or the
complete runtime API. After resolving input bindings, the graph timeline joins
membership, message properties, author/reply edges, and output URL bindings in
one SQL statement. Reads are warm and execution order is fixed, so use results
to find design costs, not as a universal speed claim.

Authorization, admission deduplication, invocation/wait transitions, full CWR
timeline and Relay page semantics, cancellation/deadlines, restart and packaged
Desktop behavior are exercised by the runtime cutover suites, separately from
this spike. Both use fresh databases and have no legacy migration path.

Source: [CLI example](../cwpdb_spike.rs),
[physical schema](schema.sql),
[behavior tests](tests.rs),
[binding tests](binding_tests.rs).
