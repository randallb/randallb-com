# Process adapter protocol

Protocol ref: `cwp://org.coworkerprotocol/2026-09/protocols/process_adapter`

A trusted host launches one adapter attempt with JSONL on stdin and stdout.
Stderr carries diagnostics. The host validates the invocation and its encoded
control envelope before starting the process. Launch configuration and authority
remain the host's responsibility.

After control validation, the host records `cwr.execution_started` before
launching the adapter. Failure to retain that marker prevents launch. It uses
host activity sequence 0; adapter activity frames continue to start at 1. The
record distinguishes an interrupted execution that may have performed effects
from a request that never started.

The host starts with `invoke`, containing `protocol` and `invocation`. The
invocation supplies its identity, exact action input and output contract,
selected integration parameters, available host operations, execution and
response modes, and supported adapter continuation. Conversation context has a
separate stream so its total size does not determine the size of the control
envelope.

## Context delivery

An invocation with context carries this transport marker in `invocation.thread`:

```json
{ "context_stream": { "format": "json", "version": 1 } }
```

The host immediately follows `invoke` with `context_chunk` frames and one
`context_end` frame:

```json
{"type":"context_chunk","invocation_ref":"cwp://example.test/invocations/one","sequence":1,"data":"7b7d"}
{"type":"context_end","invocation_ref":"cwp://example.test/invocations/one","chunks":1}
```

`data` is hex-encoded bytes from the UTF-8 JSON representation of the complete
source context. Each chunk contains at most 65,536 decoded bytes. Sequence
numbers start at 1 and increase by one. The end frame names the same invocation
and gives the number of chunks. The receiver rejects foreign identities, missing
or repeated chunks, invalid encoding, incomplete JSON, and a missing end frame.
An invocation with an empty context object may carry `{}` directly.

Chunk boundaries are transport boundaries. They may occur inside a character,
message body, or returned result; they do not create new conversation turns. The
decoded context preserves each source turn's identity, author, order, reply
relationship, and complete body. Explicitly included inner conversations remain
contained in their original source records. Public commentary and explicitly
published progress from inner work appear separately as canonical messages in
the root shared conversation; that publication does not expose the inner
provider history.

An opted-in Codex conversational invocation may instead carry a selected set of
shared messages. Its `history_delivery` metadata identifies the absolute
selected positions, source message, archive boundary, and omitted bodies. The
addressed source remains complete. Newly published progress since the retained
checkpoint is selected as ordinary messages. After a private-history gap, so is
published progress replying to the prior request. Optional oversized records
carry an explicit browse reference. The stream contains the complete selected
context; it does not imply complete archive coverage. Earlier records remain
available through participant-scoped browsing. Existing adapters retain
complete-body delivery.

The Rust `read_adapter_message` function consumes this stream before returning
`AdapterHostMessage::Invoke`. It decodes into the adapter's source context
without building a second encoded copy of the whole history. An adapter can read
the stream through stdin without access to the host's filesystem. Live execution
starts after complete context delivery and adapter initialization.

Retained invocations identify their canonical context snapshot and its source
boundary. Recovery uses that recorded source when later messages have arrived.
Independent invocations keep their admission boundary. Continuing conversational
invocations bind their boundary at first execution so queued requests include
intervening updates; subsequent retries keep that boundary.

`conversation_continuity` defaults to false in the invocation envelope. The
runtime enables it for conversational requests through Codex and Claude. Named
actions and other adapters use independent history. The source context contains
`working_conversation.ref`, optionally `previous_invocation_ref`,
`checkpoint_activity_ref`, and opaque `state`. An own-conversation execution
carries this source under `requesting_context`. A legacy boundary may include
`history_gap` rather than claim earlier private work was restored.

Before returning a successful conversational result, a supporting adapter emits
an acknowledged `provider_event` activity with method `cwr.working_state`. Its
data names the provider, working ref, native thread/session ID, invocation ref,
confirmed `context_snapshot`, and `idle: true`. Provider-specific compatibility
and compaction data may accompany it. The runtime accepts at most 16 KiB of this
control state for reuse and requires the prior invocation to be completed with
no active attempt. This differs from recovery of an interrupted live invocation,
which remains unsupported without exact call recovery.

Claude emits version 1. Codex emits version 3 with `provider: "codex"`,
`working_ref`, `thread_id`, `initialization_digest`, `invocation_ref`,
`context_snapshot`, `idle: true`, and the latest scoped native occupancy when
available. It may also retain a digest of the model-visible coworker directory
so unchanged discovery is omitted from the next request. Version 3 schedules
native compaction between token-estimated context delivery stages and records
whether retained usage became stale. It omits the former `working_bytes` and
`compacted_items` fields. Hosts accept Codex versions 2 and 3 only when both the
event and state identify Codex; other providers retain version-1 semantics.
Codex can reuse settled version-1 and version-2 history when its initialization
digest matches. Version-1 compatibility still includes the deprecated
`context_window_tokens` value; later digests omit it. Incompatible history fails
explicitly.

Codex preparation checkpoints use version 4 for the concise source-boundary
projection. Versions 1 through 3 rebuild preparation from pinned sources and a
compatible settled provider thread instead of reusing their transport-unit
index. No checkpoint version permits automatic replay after live execution
begins.

The runtime retains source ranges instead of repeated history bodies. Version-1
context snapshots cover a prefix. Version 2 adds `message_start`;
`message_count` is the exclusive end position, and the digest covers precisely
that range. `context_start` in delivered context matches the snapshot's start.
Prior private coverage plus the supplied range must establish the complete
shared context.

Codex and Claude fork the previous settled provider history into the new
invocation's execution. Retrying preparation leaves that previous state intact.
Codex forks inherit retained tool definitions. Preserve used tools and their
exact historical mappings, and validate provider-state and instruction
compatibility separately from the current discovery catalog and CWR action
authorization. Claude applies the current MCP configuration when resuming with
`--fork-session`. Incompatible or unavailable history is reported explicitly; it
must not become an unannounced fresh conversation. Sharing history grants no
execution authority.

Codex may alternate exact attributed-history injection with
`thread/compact/start` before its single live turn. Schedule these boundaries
with the shared advisory token estimator, native occupancy when trustworthy, and
a conservative absolute token threshold when the model window is unknown. Byte
counts may split transport payloads but do not schedule compaction or reject
source records. A preparation compaction completes only after the scoped
`contextCompaction` item and matching successful turn complete.

## Live calls and results

The adapter emits ordered `progress` and one terminal `result`. A terminal
result carries the declared action result or an error, and may carry supported
opaque continuation. The host validates the terminal response and exact output
schema. A `progress` frame may set `published: true` for authored progress to
the requesting conversation; the default is false for provider phase
diagnostics. Completed commentary is published through its acknowledged
activity. Publication retains source identity without creating an extra
response. Activity acknowledgments, provisional model output, and content-free
timing checkpoints retain their separate event semantics.

Progress frames may carry the optional `progress` measurement defined by the
[action response contract](../../../cwp/schemas/2026-09/action_response/README.md).
Adapters transport the action's measurement and explicit publication choice;
they do not infer task progress from raw output or elapsed time. A Shell binding
must allowlist each action that consumes `CWR_PROGRESS` stderr records. Valid
records require a one-line summary and may supply `published` and `progress`;
malformed or non-allowlisted records remain diagnostics, never conversation
content. Existing summary-only records default to unpublished.

Fallback updates belong to opted-in action helpers. After 60 seconds without an
authored update, a helper may say the action has not finished and report elapsed
time. It must not claim worker health or predict completion. Semantic updates
reset that quiet interval. Command exit stops the timer; host cancellation stops
the action and children in their inherited process group. Helpers never emit
`499`. Actions with an existing heartbeat must not gain another timer.

An adapter requests an available host operation with `call`, including its
invocation ref, unique call ID, operation name, and exact input. The host
replies with `call_result` carrying that ID and either a result or an error.

Synchronous child actions and executable message exchanges settle into the same
live call. The calling adapter remains alive while the child works. Asynchronous
requests return the admitted child invocation reference and record the eventual
result in the requesting conversation. Parent completion leaves asynchronous
children running; explicit parent stop cancels its unfinished descendants.

The protocol does not support `cwr.await_invocation`, persisted suspension,
`resumed_host_call`, or automatic fresh-turn resumption after a child completes.
An interrupted synchronous call must not replay completed child work. Recovery
can continue only when the adapter can account for the original live call and
its settled result; otherwise the host records an interrupted execution.

## Transport limits and failures

An individual action input or successful result is limited to 16 MiB after JSON
serialization. Each JSONL control frame is limited to 32 MiB, including its
newline, so an exact 16 MiB value still fits with its invocation or result
envelope. Readers enforce the frame limit while receiving bytes, including when
no newline arrives. Writers validate the complete capped frame before sending
any part of it.

The process event reader queues at most four ordinary frames and at most 32 MiB
of their encoded bytes. A terminal frame has a separate slot so progress cannot
hide completion. When a complete frame cannot enter the ordinary queue, the
reader holds that frame and stops reading. The ordinary queue plus either its
reader-held frame or the terminal slot can retain up to 64 MiB of encoded
frames. Cancellation releases that backpressure. Parsed JSON values and other
host or adapter copies are additional memory, so these transport limits are not
a whole-runtime memory guarantee.

Source conversation size is independent of those limits. Histories and
individual historical bodies larger than one frame travel through the context
stream. Adapters remain responsible for provider input limits, source turn
mapping, compaction, and continuation coverage.

Live typed action inputs, host-call replies, and terminal results retain their
declared contracts. This protocol does not implicitly replace those values with
references or truncate them. An oversized input fails before launch. An
oversized adapter result is rejected explicitly at the receiving protocol
boundary. If a completed host operation's reply cannot fit, the adapter receives
`error.code = "result_delivery_failed"`; completed child action outcomes remain
recorded, and the host does not execute the operation again.

The `context_delivered` timing checkpoint reports source bytes, chunk count,
largest chunk frame, invoke frame size, queue byte limit, and delivery duration.
These measurements contain no context text. Source data size and ingestion cost
are distinct from transport buffers and persisted snapshot storage.

## Changing the contract

Changes to framing, call lifecycle, or initialization must update the contract
and affected hosts and adapters together. A coordinated replacement of an
unpublished runtime bundle may break its previous protocol. When versions are
published or deployed independently, define compatibility or explicit version
selection so an unsupported pairing is rejected before execution. Increasing a
frame limit alone does not satisfy complete context delivery or replace checks
for growing histories, large individual bodies, and slow consumers.

## Activated sources and exact tool targets

Supply the activated role contexts in order, their established instruction
contents, one consistent shared execution configuration, CVO tool projections,
the shared directory, and confirmed shared-message deltas. If activated source
contents cannot be established, reject before provider execution rather than
reading the newest files. Retries retain the original source boundary and
complete initialization; later requests may continue a compatible settled
private history.

Logical model tools use the `cwp` namespace and coworker/verb/object names.
Retain each used native name, schema, exact runtime/coworker/role/action/binding
mapping, and action contract identity. Decode historical helper calls with their
historical schemas; missing decoders or mappings reject without execution or
history reset. Compare current action contracts and apply live authorization
before dispatch. Never resolve an old activation through current aliases or
defaults, and never prune used tools when discovery changes.
