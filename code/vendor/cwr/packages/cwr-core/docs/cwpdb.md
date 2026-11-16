# CWR runtime storage with cwpDb

CWR retains conversations, messages, activities, admissions, invocations, waits,
working-conversation links and upstream declarations in cwpDb. The runtime and
Desktop use this store directly. The [CLI spike](../../cwpdb/examples/cwpdb_spike/README.md) remains a
separate experiment and comparison fixture.

The shared Rust implementation lives in `packages/cwpdb`; CWR retains its
`cwr::cwpdb` namespace as a re-export. Desktop's `wlpdb.ts` owns native
connection and library lifetimes through a JSON C ABI. Both use the same SQLite
graph primitives while application code owns schemas and permissions.

## Physical storage

Three SQLite tables hold the graph:

| Table          | Content                                                                            |
| -------------- | ---------------------------------------------------------------------------------- |
| `ref_bindings` | Immutable canonical CWP URL to positive `INTEGER PRIMARY KEY` mapping              |
| `nodes`        | Integer identity and schema, kind, revision, JSONB properties                      |
| `edges`        | Integer identity, schema and endpoints, kind, position, revision, JSONB properties |

Keys are local to one database. Inputs, outputs, exports and public cursors keep
canonical CWP references. A durable binding remains after its record is removed;
its integer cannot acquire a different meaning. Bindings created by a failed
transaction roll back with that transaction.

Nodes use `cwp://org.coworkerprotocol/cwpdb/schemas/{kind}/2026-09` contracts.
The physical store and runtime state each have their own schema URL. Startup
rejects unsupported store/state contracts, and writer startup rejects unfamiliar
schemas for known records before recovery or scheduling. These are internal
storage contracts, not replacements for the published CWP wire schemas.

Relationship identity is a SHA-256 digest of length-delimited kind, source and
target references. Rebuilding or removing and recreating a relationship reuses
its binding. One `(source, kind, target)` relationship is allowed. Domain code
retains responsibility for cardinality, authorization and state transitions.

Four generic edge indexes support forward and reverse traversal and ordered
paging. A node-kind index supports loading retained records. No domain-specific
SQL tables or indexes are required. Connection-local views provide the runtime's
read shapes; they do not store another copy of the graph.

## Workloop product storage

WLP DB reuses these three physical tables in a separate product-owned SQLite
file. Workspace environment records use WLP schema URLs and retain entry
revisions, permanent references, membership edges, and credential cleanup work.
Secret values remain in Keychain. These records are not CWR conversation state
and are preserved when Desktop runtime state is reset. See
[workspace environment](../../../apps/workloop-desktop/docs/workspace-environment.md).

## Runtime records and relationships

Conversation properties hold title and summary metadata. Membership, parentage,
participants and archive state are edges. Archive membership points to the
runtime's workspace scope. Message and activity membership has a stable
position. Root-conversation listing uses a separately ordered derived membership
edge.

Message, activity, admission, invocation, wait and upstream-declaration nodes
retain their typed wire record under `props.record`. Those JSON records retain
canonical reference strings and arbitrary request/provider data. Indexed
relationships use integer keys; this does not compress arbitrary JSON strings.
Known reference fields also have derived lookup edges. Timeline edges and their
invocation/source/parent lookups are maintained in the same transaction as the
records they project. Public message timelines reuse the canonical message node.

Property updates preserve unmentioned outer properties with JSON Merge Patch.
When supplied, `record` replaces that entire domain record, so removed optional
fields cannot linger. The current typed records continue to reject unknown
fields and unsupported enum values. Extensions belong alongside `record`;
changing executable record semantics requires an understood schema contract.
Deletion removes the record and relationships indexing it, while preserving its
reference binding.

Writes use immediate transactions, foreign keys, WAL and `synchronous=FULL`. The
runtime's mutation journal writes only changed records and restores cached state
if SQLite fails. Read operations use a separate connection and one WAL snapshot,
with the existing cancellation and deadline limits. Timeline and summary
pagination continue to use the public cursor and visibility rules.

## Fresh installation

An empty database initializes cwpDb directly. The former SQL migration chain and
explicit legacy JSON import entry point have been removed. An older database is
rejected with a manual-reset instruction and is not converted or deleted.
Existing installations must be handled separately by their operator. Export and
import of conversations through the current runtime API remain supported.

## Verification and measurement

From the repository root:

```sh
direnv exec . cargo test --workspace --locked
direnv exec . cargo clippy --workspace --all-targets --locked -- -D warnings
direnv exec . deno task app:check
direnv exec . cargo run --release --locked -p cwr-core --example cwpdb_runtime_bench -- tmp/cwpdb-runtime-run 1000
```

The physical spike and its legacy baseline now live in cwpdb; run it with `cargo test --locked -p cwpdb --example cwpdb_spike`. The runtime benchmark remains a CWR public-API driver and uses a finite cwpdb benchmark helper for checkpoint/integrity/version inspection, with no SQL or native handle in the driver.

The runtime benchmark requires a new output directory. It exercises admission,
claim, progress, completion, duplicate ingress, read pages and reopening through
real runtime APIs, using WAL/FULL and 1 KiB messages. It writes `benchmark.json`
and the database in that directory. It excludes provider processes and host
publication, so it does not measure total agent-turn latency. The earlier
message-only spike measures a different workload and cannot predict the size of
a complete runtime database.

In the September 21, 2026 comparison, three alternating release runs each
completed 1,000 turns. The previous store was rebuilt from the saved pre-cutover
conversation module. Medians across those runs were:

| Measurement                        | Previous store | cwpDb runtime |
| ---------------------------------- | -------------: | ------------: |
| Checkpointed database with indexes |      43.60 MiB |     47.22 MiB |
| Complete workload                  |         4.46 s |        7.67 s |
| 50-conversation list               |       0.215 ms |      0.176 ms |
| Completed-turn timeline            |       0.075 ms |      0.186 ms |
| Reopen                             |          94 ms |        228 ms |

This runtime fixture uses about 8% more disk and takes about 72% more time
through the storage APIs. The cutover provides the generic graph layout and
removes the SQL migration chain; the isolated message-spike savings do not
establish a runtime-wide size reduction or speedup.

Source: [graph storage](../../cwpdb/src/lib.rs),
[physical schema](../../cwpdb/src/schema.sql),
[runtime mapping](../src/v2026_09/conversation/sqlite/sync.rs),
[read views](../../cwpdb/src/conversation/native/read_snapshot/read_views.sql),
[runtime benchmark](../examples/cwpdb_runtime_bench.rs).

## Desktop native access

`openWlpDb(libraryPath, databasePath)` returns a session with `store` and an
idempotent `close()`. `store.executeBatch` executes parameterized statements in
one transaction; failures roll back the entire batch. `store.query` accepts
read-only SELECT, WITH, or EXPLAIN statements and returns columns, scalar rows,
and a truncation flag. Close the session to close SQLite before unloading the
library. These calls are synchronous; run sustained storage work in a worker.

The runtime build produces `libcwpdb.dylib` on macOS and the application bundle
includes it beside `cwp-runtime`. Source and bundle callers use
`cwpdbLibraryPath(resources)` to resolve it. Read-only sessions require an
existing database and do not initialize schemas. The caller chooses the path and
remains responsible for authorization, application migrations, and retention.
This API does not migrate Gmail, GroupMe, or SMS permission stores.

Graph references are durable by default. Disposable nodes and edges explicitly
opt into reclaimable identities; removal only reclaims unused disposable
bindings. Existing databases upgrade with every old reference kept durable.

Run `cargo test --locked -p cwpdb` for native transactions and graph invariants,
and `deno test -P apps/workloop-desktop/wlpdb_test.ts` for the real Deno FFI
path.
