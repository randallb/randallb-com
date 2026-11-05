# Local turn timing records

The [CWR Metrics policy](../../../../memos/policy/cwr/metrics.md) governs
what these records may contain and how CWR treats them. This file documents the
current local implementation.

The runtime writes best-effort JSONL records to `metrics/` beside its configured
conversation admission state file. The same background writer also mirrors the
records into `metrics.sqlite3` beside that state file. It is a generic CWR
metrics database built on the shared cwpDb machinery, so applications can query
it through the same local binding without treating JSONL as an API. Recording
never uses the conversation activity acknowledgement path, and prerelease
network export uses the configured Workloop PostHog destination by default (see
below).

Each metric event is a disposable `cwr_metric_observation` cwpDb node. Indexed
edges connect it to the invocation, and where known, the model run and
conversation. Its `invocation_performance` view exposes an event timeline for
one invocation without adding nested durations together. Its
`compaction_performance` view selects preparation and live compaction starts,
completions, and failures with their recorded context counts and elapsed
duration. Views return raw rows so a reader can show overlap, missing values,
and incomplete starts honestly. Metric references begin as disposable graph
bindings; retention removes expired observation nodes and edges, then reclaims
their unused bindings. A binding promoted by another application graph user is
durable and is never deleted by metrics retention. The database reserves 56 MiB
for SQLite pages and 8 MiB for its write-ahead log, with a 250,000-record
budget; a full writer drops excess samples and reports the count in later
records. Retention and budget deletes run in small checkpointed batches; their
pages are reused by SQLite rather than triggering a large live `VACUUM`.
Deleting `metrics.sqlite3` remains safe when a full reset is useful. Historic
JSONL files remain untouched; new records only use SQLite. Workloop Desktop
exposes the raw invocation timing observations in each work card's
**Performance** details. It opens the file read-only in a short-lived worker,
then refreshes after the runtime's activity settles; a missing, locked, or
replaced metrics database is shown as unavailable rather than delaying a turn or
freezing the UI.

A bounded queue holds 256 records; the background writer records only the
existing SQLite graph store. Queue overflow or write errors increment
`dropped_records_total`, included in later records and logged by the writer.
Timing storage errors do not fail a turn. The final owner closes and drains the
queue at shutdown, waiting at most one second. A crash, forced exit or timeout
can lose queued records; incomplete samples are not zero-duration work.

Each schema-version-2 record carries runtime, conversation, and invocation
references; parent invocation; attempt number; model-run reference; actor; model
configuration; same-invocation continuation and private-history-supplied flags;
thread-context byte count; process ID; application package version; and receipt
timestamp. No message text, prompts, tool arguments, outputs, or provider
credentials are included. The package version is not a build revision.

Runtime checkpoints:

- `attempt_started`: queue time for the first attempt only (wall-clock admission
  to claim), and monotonic time spent completing the persisted claim. Queue time
  is null on resumed attempts. `claim_persistence_ms` includes reconstructing
  the claimed invocation and cloning the graph, not just storage writes.
  `metrics_metadata_ms` measures preparing the metric context, including its
  context-byte count.
- `runtime_setup`: `stage`, `duration_ms`, and cumulative `attempt_elapsed_ms`
  for the invocation-start observer, route selection, action-only message
  handling, lifecycle-operation installation, and invocation retention. Each
  record follows the operation even when it returns an error. Later stages are
  absent on early exits; a running or killed operation has no completion record.
  Retention includes acquiring the runtime lock and persisting the invocation.
- `adapter_dispatch`: elapsed from claim to entering adapter launch, including
  the setup stages above. Do not add these stages to that cumulative duration.
- `adapter_process_failed`: emitted when a launched adapter exits before
  returning a protocol result. It records the coarse phase (`awaiting_result`),
  exit code or signal, stdout state, and stderr byte count and truncation state.
  It never records stderr content.
- `attempt_ended`: monotonic attempt duration, outcome, and total turn
  wall-clock duration when a terminal ledger record exists. Yielded attempts are
  separate from completed turns. Total time includes waits and prior attempts.

Adapter checkpoints appear under `measurements.timing`. Codex checkpoints
explicitly record `provider_process_reused: false` for the current
process-per-attempt implementation. The process transport records the adapter OS
spawn duration. Codex records adapter preparation, provider OS spawn, startup
completion, successful turn acknowledgement, first activity, first text, and
provider end. All these durations use local monotonic clocks; do not subtract
timestamps across processes to derive exact subphases.

At startup completion, `provider_startup` spans provider spawn initiation to
just before sending `turn/start`. Its breakdown includes `context_preparation`
(source hashing and spooling), initialization, thread setup, context injection,
and host acknowledgements. `host_acknowledgement` accumulates all synchronous
startup activity callbacks, including provider identity and repeated history
checkpoint saves. `context_injection` excludes those callbacks and includes
history delivery. These two durations are disjoint; both are included in
`provider_startup`. Live-turn callbacks do not contribute to startup
acknowledgement time. On failure, `provider_ended` retains measured
acknowledgements even if context injection never completed. The OS spawn
duration is in the preceding provider-spawn checkpoint. Explicit native Codex
compaction between history-delivery stages is included in `context_injection`
and `provider_startup`; automatic compaction after the live `turn/start` is not.
Automatic compaction latency contributes to live-turn duration and, when it
precedes output, the first-activity and first-text intervals. Do not interpret
those intervals as inference-only latency or compare preparation compaction
directly with live automatic compaction. Some small reader/setup overhead is
outside the individual spans. Runtime `attempt_elapsed_at_receive_ms` is an
observed elapsed time from claim, including transport delay; it is not an exact
provider timestamp.

`first_activity` and `first_text` start at sending `turn/start`. Empty assistant
items and reasoning items do not count. First text is the first nonempty visible
text delta, or completed text when no delta was delivered. Frames are
timestamped by the provider reader before queueing, so earlier activity
acknowledgements do not inflate arrival time. The checkpoint is emitted after
output forwarding. It is not model TTFT or time to UI paint. Activity may be a
tool operation or text; the two timings overlap. First-output checkpoints are
omitted when output was never observed. Failed startup includes completed stage
timings in `provider_ended` when the adapter can report them; a killed process
may only have the runtime end record.

To investigate recurring setup costs, compare attempts for the same actor and
similar context sizes, keeping resumed attempts separate.
`invocation_continuation_supplied` means an interrupted attempt of the same
invocation supplied its checkpoint. `private_history_supplied` means CWR passed
settled history from an earlier invocation; it does not confirm that the
provider accepted a fork. Both are false when neither state was supplied.
Version 1 records called the first flag `thread_reused`; their new view columns
are null because they did not record private-history selection. Provider
processes currently start afresh on each attempt. The measurements begin at
runtime claim; development-shim builds, binary hashing, admission, and waits
before claim are not part of `attempt_elapsed_ms`.

Join records by runtime, invocation, attempt, and model-run reference. Keep
outcomes and missing checkpoints in analyses. Do not sum overlapping durations
or count individual failed tools as failed turns. Existing conversation activity
still owns per-tool durations; this initial recorder does not duplicate tool
logs.

## CLI connection startup diagnostics

Set `CWR_STARTUP_TIMING=1` when running `cwr exec` or `cwr actions` to measure
work before invocation admission. The CLI buffers at most 64 content-free spans
and writes one JSON diagnostic to stderr at command exit, including on errors.
Normal stdout and exit status are unchanged. Redirect stderr to a local file to
retain a sample. This opt-in trace is separate from the runtime's JSONL store. A
killed process may never flush it. Contention or capacity limits drop spans
instead of blocking startup; `dropped_spans` reports that loss.

The `cli_startup_timing` record includes schema version, CLI process ID, receipt
time, command success, and total command elapsed time. Each span has a static
stage name, `start_ms` since entry to `main`, `duration_ms`, and `completed`. An
early return marks the active span incomplete, preserving its elapsed time. No
arguments, paths, environment values, or host payloads are recorded.

- `cli_preparation`: packaged module configuration and argument collection.
- `workspace_selection`: locating the selected workspace manifest.
- `client_open`: the entire bridge connection and initial host setup.
- `bridge_spawn`: locating and spawning the bridge executable; this does not
  mean the bridge is ready to handle requests.
- `bridge_first_event`: reader startup to receiving the first complete frame,
  including bridge initialization and upstream work, before JSON decoding.
- `workspace_inspection`: sending inspection through receiving its response;
  this includes any bridge initialization before the request can be serviced.
- `actor_selection`: sending selection through receiving the selected actor and
  catalog.
- `initial_snapshot`: waiting for and applying the initial conversation snapshot
  and source readiness after actor selection.
- `snapshot_decode`: decoding and validating the first conversation snapshot on
  the reader thread, which may overlap actor selection or snapshot waiting.
- `submission_to_admission`: sending a text or action request through receiving
  its admission or intrinsic response. Execution after admission is excluded.

`client_open` contains its connection subspans. Reader spans overlap main-thread
waits, so do not sum them. Total command time includes execution and shutdown.
Time before `main` (including the repository build shim and OS executable
loading) is outside this clock. Compare successive shell `true` invocations to
measure recurring connection costs without model latency.

## Prerelease PostHog export

Canary, alpha, beta, and delta runtimes export by default to Workloop project
598787 through `https://b.getworkloop.com`. The public, write-only ingestion
token and host live in `packages/configuration/workloop_analytics.json`, shared
with the bundled desktop JS SDK. `POSTHOG_HOST` and `POSTHOG_PROJECT_TOKEN`
override these defaults. The host may be a Workloop ingestion proxy and must use
HTTPS; a bare hostname is treated as HTTPS. Packaged Desktop sets the release
channel from its bundled train file; source previews use `local` and do not
export. `CWR_POSTHOG_HOST` and `CWR_POSTHOG_TOKEN` override the shared settings.
Set `WORKLOOP_RELEASE_CHANNEL` to `canary`, `alpha`, `beta`, or `delta` and
optionally `WORKLOOP_BUILD_SHA` to the build revision (otherwise recorded as
`unknown`).

Desktop compares an analytics configuration fingerprint before attaching to a
resident runtime and drains/replaces residents with stale channel, opt-out or
destination settings.

Set `CWR_POSTHOG_ENABLED=0` to disable export. No opt-in is needed for
prerelease builds. Stable/production and unspecified channels never export.
Local collection continues independently. The destination owner manages
engineering access, 14-day event retention, and deletion in the configured
PostHog project.

Export uses an explicit schema of numeric measurements, approved
outcome/checkpoint labels, version, attempt and context-size fields. Actor
references, process IDs, model configuration names and arbitrary strings remain
local. A random analytics-only identifier is stored in the outbox database and
used as `distinct_id`. Runtime, conversation, invocation, model-run and
source-message references are hashed with that local random salt before export;
hostnames and original references stay local. Events also include
`$process_person_profile=false`, `$geoip_disable=true`, `release_channel`,
`build_sha`, and `export_dropped_total`. Original UTC occurrence timestamps and
random event UUIDs are saved before upload. Adapter process exits become
synthetic `$exception` events with the fixed type `CwrAdapterProcessFailure` and
a fixed message. Other observations use `cwr_metric`. No exception messages,
stack traces, stderr, conversation content, tool payloads or file paths are
added. This covers CWR adapter process errors; it does not install desktop
JavaScript error capture. See
[PostHog's exception schema](https://posthog.com/docs/error-tracking/issues-and-exceptions).

A separate thread owns `posthog-outbox/queue.sqlite3` beside runtime state. The
private directory and database reject symlinks and shared file permissions. The
outbox retains at most 10,000 records, 8 MiB of event payloads, and 14 days;
SQLite pages are capped at 16 MiB. Quarantined events count toward these limits.
The queue is separate from conversation state and may be deleted safely while
CWR is stopped. Memory admission is best effort: full queues, failed storage,
and forced exits can lose samples. Durable pending events survive a restart.

The uploader sends up to 250 events and less than 1 MB per `/batch/` request,
every five minutes or at 250 events. Pending events are retried on startup.
Failures retain the same event IDs and timestamps, with exponential backoff and
jitter up to ten minutes. HTTP 400/413/422 batches are split to quarantine
individual invalid events. Other failures, including authorization errors,
retain the batch. Successful HTTP acknowledgement removes those events. A crash
between server acceptance and local acknowledgement can resend an event with its
original UUID. Redirects are disabled. Request and response bodies are never
logged. A project/host change cannot send the old project's queue to the new
destination.

To manually flush pending metrics using the same configuration:

```sh
cargo run -p cwr-core --example flush_metrics -- /absolute/path/to/runtime-state-file
```

The command reports failure while retaining unacknowledged events. Normal
shutdown waits up to 750 ms for the exporter to drain records and attempt a
request, within the existing one-second metrics shutdown budget. A stalled
network or storage operation may exceed that budget and lose queued records. To
stop exports, set `CWR_POSTHOG_ENABLED=0` and restart the runtime. Delete
`posthog-outbox/` while stopped to remove retained export data. Local metrics
are independent. Uploaded data must be deleted in the chosen PostHog project
using its project controls. No live PostHog ingestion is exercised by the unit
tests.
