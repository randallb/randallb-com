# CWR performance storage

`MetricObservation::from_value` validates the versioned content-free record before
retention or writes. `PerformanceStore::write` appends observations and their
invocation/model-run/conversation projections through named storage operations.
CWR supplies facts; it supplies no SQL, native connection or transaction callback.

A batch commits at most eight observations atomically. A failure reports the
number not committed and the number already evicted; retrying an arbitrary batch
does not promise deduplication. These best-effort metrics are disposable and never
own customer invocation history. They retain fourteen receipt days, at most
250,000 observations, a 56 MiB main database and a 64 MiB database/WAL/shared-memory
budget. Oversized records fail before retention; a blocked WAL refuses additional
writes. Disposable identity cleanup preserves externally retained durable refs.

`NativeStore::metric_records` returns at most the requested 1–10,000 records and
an explicit truncation flag. SQL views remain private implementation projections.
The owning runtime must authorize any externally exposed diagnostic reads; this
local private store does not establish caller authority.

The cwpdb tests exercise partial batch failures, partial retention failures,
record limits, durable-identity preservation, database reopening, bounded reads
and compaction projections. CWR tests retain producer/privacy/queue/shutdown
behavior. This migration establishes no complete CWR storage or hosted customer
acceptance by itself.

Desktop reads invocation timing through `NativeStore::invocation_performance` /
`cwpdb_invocation_performance`, passing only runtime, invocation and a page limit.
The projection owns the materialized invocation-edge query and returns named
content-free fields with explicit truncation. Its physical30,050-observation
fixture and query-plan regression live in `packages/cwpdb/tests/performance_fixture.ts`;
the Desktop worker integration exercises that exact SQL through the named FFI.
