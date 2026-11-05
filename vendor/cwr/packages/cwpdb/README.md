# cwpDb

`cwpdb` owns the reusable SQLite property-graph layout used by CWR. It maps
canonical CWP references to immutable database-local integers, stores nodes and
relationships with JSON properties, and leaves domain schemas, views, retention,
and authorization with the application using it.

`NativeStore` is the small bridge for an application that needs its own tables
beside that graph. A `BatchRequest` runs its parameterized statements in one
immediate transaction. A `QueryRequest` is read-only and returns JSON scalar
rows, capped at 1,000 by default and 10,000 at most. Application data may retain
expiring CWP references as ordinary text values, or use `put_disposable_node`
and `put_disposable_edge` when those records need indexed graph relationships.
`remove_disposables` reclaims only unused disposable identities. It refuses
removal when any affected record or relationship has a durable identity,
including relationships that index other relationships. Durable graph work and
schema bindings remain protected.

Deno callers can use `GraphBatchRequest` through `cwpdb_graph_batch` to compose
up to 256 typed operations in one immediate transaction: `put_node`,
`insert_disposable_node`, and `remove_disposables` (at most 256 references per
operation). These call the shared Rust graph primitives. Insert-once nodes
return 1 for a new record or 0 for a compatible duplicate, without updating
properties or renewing retention. A durable identity collision is an error; all
operations roll back together on failure. Applications can index node properties
and expose read-only SQL views over the graph without duplicating record
storage. The closure-based `graph_transaction` remains Rust-only.

The `cdylib` exports a Deno-FFI-friendly JSON boundary:

- `cwpdb_open(path, out_handle, out_error)`
- `cwpdb_open_readonly(path, out_handle, out_error)`
- `cwpdb_execute_batch(handle, request_json, out_response, out_error)`
- `cwpdb_graph_batch(handle, request_json, out_response, out_error)`
- `cwpdb_query(handle, request_json, out_response, out_error)`
- `cwpdb_set_max_database_bytes(handle, bytes, out_response, out_error)`
- `cwpdb_checkpoint_wal(handle, out_response, out_error)`
- `cwpdb_close(handle)`
- `cwpdb_string_free(pointer)`

Calls return `0` on success. Responses and errors are UTF-8 strings allocated by
the library and must be freed with `cwpdb_string_free`; handles must be closed
once. SQL parameters and query results accept JSON scalar values only: null,
boolean, number, and string. The library does not expose binary values at this
boundary.

`cwpdb_open_readonly` requires an existing regular database file, never
initializes schema or creates files, and uses a short busy timeout for optional
diagnostic reads.

Disposable writers can reserve database pages with
`cwpdb_set_max_database_bytes`. It returns the effective page-rounded byte
ceiling as JSON and disables cache spilling, so a transaction does not
repeatedly write a page to WAL. Callers reserve WAL headroom separately and
checkpoint before each transaction. `cwpdb_checkpoint_wal` returns JSON with
`busy`, `log_frames`, and `checkpointed_frames`; a busy checkpoint must stop
writes when the caller cannot safely accommodate more WAL. Both calls use the
same response/error ownership as query and batch calls.

Rust callers can use `ref_read::get_ref` for an authoritative resource read
returning its canonical ref, domain kind, schema/value, and opaque revision.
Missing records (including binding-only and deleted refs), edge refs, and
storage failures have distinct errors. It reads existing graph data without
migration. This internal primitive requires an already-authorized canonical ref;
domain authorization and schema validation remain with the caller. It does not
expose a new native or network entrypoint. Use a fresh SQLite snapshot for
read-after-write visibility.

Rust domains can create an `edge_read::EdgeReader` for one connection and
already-authorized caller, then call `query_edges` with explicit direction,
selected domain-registered `RelationshipKind` contracts, a positive page size,
and an optional continuation. This first primitive accepts unordered kinds only;
selecting an ordered kind, including a mixed selection, fails before reading.
The domain remains responsible for registration, authorization of all returned
relationships and endpoints on each page, and schema validation.

Pages contain full edge records in canonical edge-ref order, an opaque
in-process continuation, and `complete`. Continuations are bound to the reader
(and hence its connection and caller scope) and exact query parameters; they
cannot be transferred to another reader or serialized as public wire tokens.
Kind filters are sets. A missing or binding-only starting ref returns an empty
page without creating a binding; external and edge endpoints are preserved. Each
page uses one SQLite read, with no snapshot guarantee across pages. A
caller-owned read transaction can hold a stable snapshot for a complete report.
This adds no native/network endpoint or domain authorization mechanism.

The workspace-environment report is the first consumer. It reads every
membership page and endpoint resource inside one read transaction, validates
entry values and checks secret availability, then sorts by name and ref.
`RefRevision::legacy_counter` is a transitional adapter for that existing
numeric revision output; new APIs should keep revisions opaque.
