# CWR resident GraphQL

The resident runtime exposes an authenticated GraphQL endpoint for local
consumers, independently of Workloop Desktop. The API supports queries and
action admission. Subscriptions, active-work objects, and Desktop migration
follow. Existing host commands remain available during migration.

## Query through the CLI

Build `cwp-runtime` and `cwr` together, then select an ordinary CWR workspace:

```sh
cargo build --workspace --locked
./target/debug/cwr --workspace /path/to/workspace graphql \
  '{ runtime { id actorRef conversations(first: 20) { edges { cursor node { id title archived } } pageInfo { hasNextPage endCursor } totalCount revision } } }'
```

The CLI starts or connects to the existing resident using the local bootstrap,
then sends the operation directly over HTTP to CWR. It prints the GraphQL JSON
response and exits unsuccessfully for HTTP or GraphQL errors. Queries may come
from stdin using `-`; an optional second argument supplies a JSON variables
object. Requests never follow HTTP redirects or use ambient HTTP proxies.

```sh
./target/debug/cwr --workspace /path/to/workspace graphql \
  'query($id: ID!) { runtime { conversation(id: $id) { id legacyTimeline(first: 50) { edges { cursor node { __typename ... on Message { id authorActorRef addressedActorRefs replyToMessageRef text } ... on ActionInvocation { id state } } } pageInfo { hasNextPage endCursor } revision } } } }' \
  '{"id":"cwp://example.test/conversations/example"}'
```

Timeline edges preserve authoritative anchor order. Message ancestry returned
internally by the indexed read does not create additional visible edges.
Invocation anchors resolve the same lifecycle object as direct invocation reads.
Cursors are opaque to clients; pass the returned cursor back without editing it.
Timeline cursors include the runtime's timeline revision.

## Discover and invoke actions

`runtime.actions` returns the authenticated session's available activations and
their input/output schemas. `invokeAction` admits an action through the existing
CWR validator and scheduler. Use the runtime ID as `url` to create an action
conversation, or an accessible root conversation ID to invoke within it.

```graphql
mutation RunAction($url: ID!, $activation: String!, $input: JSON!, $requestId: ID!) {
  invokeAction(url: $url, activation: $activation, input: $input, requestId: $requestId) {
    __typename
    ... on ActionInvocation {
      id
      state
      conversationRef
      sourceMessageRef
    }
    ... on ActionError {
      code
      message
      retryable
    }
  }
}
```

The response identifies the actual CWR invocation, not a second Desktop wrapper.
Admission does not mean execution has finished. Query `runtime.invocation(id:)`
for current state, public outcome status/summary, and parent/child invocation
refs. Invocation reads use the same conversation participation checks as
queries. Existing non-GraphQL CLI action commands retain their current
transport.

Choose a request ID once for each intended action. CWR's persisted ingress
fingerprint returns the original invocation on retries, including after restart;
changing the target, activation, or input with the same ID fails. Request IDs
are scoped to the authenticated actor and runtime. JSON object key ordering does
not change the input identity.

A definite rejection returns `ActionError`. After admission, publication or
response failures produce a GraphQL error with `ACTION_OUTCOME_UNKNOWN`; an HTTP
timeout or lost response may also have admitted the action. Retry with the same
request ID and original input. Never allocate a new ID to recover an ambiguous
response. Retry handling does not execute completed work again.

## Subscriptions

Use the same resource selections under `Subscription.runtime`:

```sh
cwr graphql --watch 'subscription { runtime { conversations(first:20) { totalCount edges { node { id title } } } } }'
cwr graphql --watch 'subscription($id:ID!) { runtime { invocation(id:$id) { id state outcome { status summary } } } }' '{"id":"cwp://…"}'
```

`--watch` prints each GraphQL result as one JSON line and flushes it
immediately. Heartbeat deadlines follow elapsed time even during continuous
updates. The server selects `graphql-transport-ws` from protocol offers in
either preference order. HTTP CLI responses have an 8 MiB limit; oversized
responses return an explicit error suggesting fewer fields or a smaller page. A
mutation may already be admitted, so keep its original request ID when
recovering its outcome. It sends protocol heartbeats while idle. A disconnect
exits with an error; rerun it to discover the current endpoint and receive a
fresh snapshot.

Native clients connect to `ws://<discovered-address>/graphql` with the bearer
Authorization header and the `graphql-transport-ws` subprotocol. Send
`connection_init`, wait for `connection_ack`, then send standard `subscribe`
messages. Multiple operation IDs share a socket; `complete` cancels one
operation. The HTTP Host and Origin rules apply to upgrades too. Credentials
stay in headers, never URLs. Browser credential integration remains future work.

Each subscription emits an initial snapshot, then reruns its selected resource
reads after resident publications, including action admission and execution
updates. Reads recheck actor and conversation access. Consumers have independent
selections and streams. Updates can be coalesced: this is current state, not an
event log or token-delta stream. No separate refetch is necessary after a
snapshot. Live-output fields and TUI/Desktop integration are not included in
this slice.

There are at most 32 WebSocket connections and 64 active subscription
operations. Notifications use a coalescing watch channel rather than growing
per-client queues. Idle connections must send protocol pings within 30 seconds.
Shutdown closes active connections; credentials rotate when the resident
restarts.

## Local authentication and discovery

The runtime serves GraphQL only through its private `graphql.sock` Unix-domain
socket; it does not bind a TCP port. The owner-only `graphql.json` discovery
file uses version 2 and contains `socket_path`, runtime ref, default actor ref,
PID, and bootstrap token. Treat the whole file as a credential. The shared
`LocalGraphqlEndpoint::read` helper rejects public files, symlinks, wrong
owners, and socket paths that do not name a private Unix socket owned by this
user. Credentials rotate at startup. Shutdown removes this process's discovery
and socket files; startup recovers a stale socket but refuses a live listener or
a non-socket file at that path. An exclusive `graphql.sock.lock` is held through
listener shutdown and discovery/socket cleanup, so a competing process fails
before replacing either file. The lock file stays on disk; closing its
descriptor releases ownership, including after a crash. Version 1 TCP discovery
is no longer supported.

The discovery token is a **local operator bootstrap capability**, not a
data-session token. POST `/graphql/session` with that bearer token and
`{"actorRef":"cwp://…"}` to issue a private `{id, actorRef, token}` credential.
Omitting `actorRef` selects the startup actor. The trusted local OS operator
already controls this workspace and can select configured actors; an actor ref
alone grants no access. This is not remote identity verification. The operator
may also supply `ingressIntegrationRef`, an absolute ref without a fragment,
when issuing the session. It remains fixed for all message and action admissions
in that session. Omitting it selects the GraphQL integration; the TUI supplies
its TUI integration so changing transports preserves its admission namespace.
Session credentials cannot issue replacement credentials or change this
identity.

Use the returned token for HTTP operations and WebSocket upgrades. Each session
fixes its actor and gets a separate terminal owner, including when two terminals
use the same actor. Listing and timeline access require conversation
participation. `cwr --actor <actor-ref> graphql …` opens its own session on the
existing resident, then closes it on exit. DELETE `/graphql/session` with the
session token revokes it. Idle HTTP-only sessions expire after 60 seconds; there
are at most 128 sessions. Session credentials cannot issue other credentials.
All credentials rotate on restart.

### Terminal providers and messages

A terminal opens one WebSocket per session and subscribes to `terminalEvents`.
It also subscribes to `runtimeRestarting`, which emits the replacement
generation only after a development restart commits. The client follows the
verified replacement, renews its session and providers, and reloads history
without resending mutations. An unexpected socket closure does not authorize
generation replacement. Wait for `SessionReady` before calling
`registerTerminalProvider`, then enable its coworkers with
`enableTerminalCoworker`. Private providers appear only in that terminal's
catalog; admission and AI descendants use the same owner scope. `TerminalWork`,
`TerminalStop`, and `TerminalResultValidation` events are delivered only to
their originating terminal. Complete work with `completeTerminalWork`; its
`terminal` JSON uses the existing `AdapterTerminal` shape (`invocation_ref`,
`sequence`, `status`, `summary`, optional `result`/`error`/`continuation`).
Result protection, validation, revocation, and stop acknowledgement use the
corresponding terminal mutations. Completion and validation use separate HTTP
requests so validation callbacks can be answered while completion is pending.

Closing the WebSocket or cancelling `terminalEvents` withdraws its providers and
revokes that session. A new connection needs a new session; it cannot take over
old providers or admitted work. Events have a 64-item queue; unavailable
delivery fails through the runtime's existing provider path. Ordinary `runtime`
subscriptions remain authoritative snapshots and may repeat selected data.

`createConversation(text:, requestId:, ownerContextRef:)` creates a
conversation; `sendMessage(text:, requestId:, conversationRef:)` requires an
existing conversation and preserves its owner. Both use session-derived
authorship and terminal scope. Reusing an actor's request ID with the same input
recovers its admission across sessions; changed input fails. Recovery keeps the
original work owner and never transfers it.

HTTP and WebSocket remain the framing over the Unix socket. POST `/graphql`,
POST/DELETE `/graphql/session`, and WebSocket upgrades use `Host: localhost` and
bearer credentials; browser Origin headers are rejected. The URL
`http://localhost/graphql` supplies framing only: clients must connect through
the discovered `socket_path`. The CLI uses that path for both operations and
watch subscriptions. `host.sock` remains the separate native host protocol.

Requests have a 64 KiB body limit, schema depth/complexity limits, and a
15-second HTTP execution timeout. Pages contain at most 100 entries. Each query
or subscription snapshot gets one ten-second read budget. Queue waits,
catalog/metadata checks, page reads, and invocation hydration share its absolute
deadline. Reads wait behind an eight-worker semaphore instead of rejecting a
ninth field. Cancellation interrupts registered database reads; a worker keeps
its permit until it actually exits. Cancelling reads does not cancel admitted
actions. Each new subscription snapshot gets a fresh budget, and errors report
`READ_BUDGET_EXCEEDED` or `READ_CANCELLED`. Conversation lists, direct resource
reads, publication pages, nested children, and source records share one lazily
opened SQLite snapshot per query or subscription selection. Catalog and action
admission keep their existing runtime authority checks. Mutation results read a
fresh post-admission snapshot.

## Schema and checks

`schema.graphql` is generated from the Rust schema:

```sh
./target/debug/cwp-runtime --graphql-schema > apps/cwp-runtime/schema.graphql
cargo test -p cwp-runtime -p cwr-cli --locked
python3 e2e/cwr-graphql/run.py
cargo clippy -p cwp-runtime -p cwr-cli --all-targets --locked -- -D warnings
cargo deny check
```

The HTTP tests cover authentication, origin/Host checks, pagination, canonical
message identities, actor isolation, credential rotation, and discovery cleanup.
The schema test detects stale checked-in SDL. Discovery tests reject public and
symlinked credentials and invalid Unix socket paths. The first GraphQL command
is additive: other CLI commands still use their existing transports until
migrated.

`ActionInvocation` is an interface with eight concrete lifecycle types:
`AdmittedInvocation`, `AwaitingPersonInvocation`, `RunningInvocation`,
`CompletedInvocation`, `BlockedInvocation`, `FailedInvocation`, and
`StoppingInvocation`, and `StoppedInvocation`. `ActionResult` contains those
eight object types and `ActionError`. Existing `... on ActionInvocation`
fragments select common fields. `state` returns the retained lowercase CWR state
as a String. Consumers checking `__typename` must handle lifecycle changes on
the same canonical ID.

`Conversation.timeline(filter:, first:, after:, last:, before:)` exposes
retained publication placements through `TimelineItemConnection`. Each
`TimelineItem` has its own canonical `id`, `publicationRef`,
`presentation { text }`, typed `source`, and optional paged `children`. A
resolved source contains a `Node` whose ID is the canonical message or
invocation ref; an unavailable source has an explicit presentation instead.
`Runtime.node(ref:)` and `Runtime.timelineItem(ref:)` resolve these identities
with participant access. `conversation` and `invocation` accept `ref:` as well
as the existing `id:`.

Publication pages read only the requested connection and source records. They
support forward and backward paging, return opaque connection- and
session-scoped `Cursor` values, and compute exact sibling `totalCount` only when
selected. Connection revisions include the persisted commit so source changes
invalidate the returned view. A retained empty root stays empty. Legacy
conversations without a retained root report that the publication timeline is
unavailable; no placements or identities are invented at read time. Canonical
writers and migration follow the configured timeline rollout stage, which
defaults to disabled.

The previous flat `TimelineNode` union is available through
`Conversation.legacyTimeline`. The TUI preview reads publication placements and
uses that compatibility field for conversations without a canonical timeline.
Workloop Desktop also consumes publication placements. Terminal outcomes retain
nullable historical data and public stop causes; nonterminal outcomes are null.
Legacy suspended work is settled by CWR recovery, with no public
WaitingInvocation type.

`Message` is now an interface implemented by `StreamingMessage` and
`PublishedMessage`. Both use the same canonical ref and common authored fields.
`Runtime.message(ref:)`, generic node lookup and publication sources agree on
the concrete type and text within the selection. Existing `... on Message`
fragments continue to work; clients inspecting `__typename` handle the two
concrete types.

For disclosed invocations in activated canonical timelines, the resident's model
output publisher writes provisional messages through shared CWR transactions.
Provider item identity and the claimed model run determine the message ref;
updates preserve it. Completed commentary and a successful terminal answer reuse
their provisional message and placement IDs when published. A replaced
unpublished provider item, interrupted/yielded attempt or restart withdraws its
provisional placement and source; retry uses a new model run and new IDs. Late
updates from old or closed model runs cannot restore them. Empty provisional
text is a valid visible message, and connection revisions change with text.

Provisional records share the read snapshot with retained publications but do
not enter compatibility history or provider context. A model-output close fences
further updates; terminal publication or interruption resolves its final
placement atomically. Legacy host live-output events remain available until
those consumers migrate.

Publication `timeline` and `children` accept
`TimelineFilter { search,
sourceTypes }`. Search is a case-insensitive substring
of CWR's public presentation text, trimmed at its boundaries and limited to
1,024 UTF-8 bytes. Kinds are `MESSAGE` and `ACTION_INVOCATION`, combined with
OR; kind and search criteria combine with AND. Omitted, null, and empty criteria
do not restrict the connection. Duplicate kinds, kind order, selecting both
kinds, and blank search normalize to equivalent scopes. Search visits only the
selected siblings, not descendants. A filtered-out parent does not promote its
children into the page.

Filtering occurs in shared CWR snapshot reads before slicing. The reader scans
indexed batches of at most 100 siblings, retaining at most one page plus one
matching item. Exact filtered counts scan the whole sibling connection only when
selected; cancellation and the selection deadline cover that scan. Cursor scope
includes the normalized filter and session. Changing criteria requires a new
window; a mismatched cursor is an error. A retained sibling stays a valid
position anchor when its text stops matching; withdrawal invalidates its cursor.
Growing/finalizing streaming text can change membership without changing the
source or placement identity. Subscriptions replace the selected window,
including removed entries.

Messages now expose `author`, filtered/paged `recipients`, `occurredAt`,
`replyTo`, `progress`, `action`, and filtered/paged `attachments` alongside the
existing ref fields. Streaming messages expose their producing `invocation`.
Known authors remain `Coworker` with retained name/handle metadata after
configuration removal; unknown classifications use `UnresolvedActor`. Generic
node lookup resolves the same actor identity under session visibility.

Progress retains its exact invocation and source-activity association. Public
informational responses distinguish `ProgressOutcome` (102) from other 1xx
codes. Missing summaries, timestamps, or duration evidence remain null. Activity
timing distinguishes running and completed values; completed durations are
fixed. Subscriptions selecting `elapsedSeconds` reevaluate roughly every half
second with a fresh selection budget. Invocation sources expose retained
lifecycle dates and the current execution generation, phase and unpublished
progress, without provider previews or private response payloads. Only active
claims appear as `currentExecution`. Unfinished activity timing becomes null
after interruption and cannot resume with a later attempt.

`ContentPresentation.attachments` and source-message attachments reference the
same canonical `HtmlVisualization` resources. Title/media-type filters apply
before paging, and exact counts exclude unavailable artifacts. Artifact metadata
and `html` access require a visible associated message; an orphan artifact ref
is insufficient. Metadata selection does not load HTML. Public action metadata
exposes the recorded selector or exact linked result status/summary, never
arbitrary action input or result JSON.

The broader presentation proposal still includes catalog discovery, additional
conversation/current-work relationships and filters, shared connection
interfaces, terminal outcome families, and client adoption of the added
metadata. Those are not claimed by this renderer-metadata slice.

Resident socket handshakes begin only after GraphQL discovery has been
published. HTTP request extraction and execution have a 15-second outer
deadline. Shutdown cancels subscriptions and drains HTTP for at most two
seconds, then retires the GraphQL executor. Transport shutdown does not itself
cancel admitted actions.

### Metadata completion and inspector follow-up

`Message.addressingMode` is nullable `EXPLICIT` or `IMPLICIT`, resolved from the
retained message rather than guessed from its recipients. Both streaming and
published message types implement it. Unknown historical addressing remains
null. Invocation `requester` and `target` resolve the existing actor refs under
the same authorized snapshot, including `UnresolvedActor` for identities without
retained local presentation metadata.

Message `occurredAt` is the earliest retained message-recorded timestamp, null
when unknown. Streaming occurrence time is currently null; finalization supplies
the published record's timestamp. Message and placement identity stay stable,
but that does not promise a stable timestamp across finalization.

The [invocation inspector reads](inspector-contract.md) expose paged detailed
activities and file changes plus explicit call/wait relationships on every
invocation lifecycle type. They reuse the existing public work projection and
ledger under the selection snapshot, with separate payload visibility and
preview/response limits. Published progress remains on message publications;
attached/detached execution still needs retained domain evidence.

A retained stop request changes an active invocation to `StoppingInvocation`.
`finishedAt` and `outcome` remain absent until its executor and stopped
descendants settle. Queued work settles immediately. `stopSettlement` is
`PENDING` while stopping, `CONFIRMED` after explicit settlement, and
`UNAVAILABLE` for older stopped records. A disconnected upstream provider does
not confirm exit; unconfirmed stops remain retained and cannot run again after
restart. `stopControl` links a stop action to its authorized target and exposes
its request disposition. Completion of that control means acceptance, not
executor exit.

## Coworker configuration

`runtime.messageTargets` and other `Coworker` objects expose `category`,
`bindings`, and `defaultBinding`. These describe current session-visible
configuration, including unavailable bindings, rather than the settings used by
a historical invocation. A missing declared default stays null; another binding
is never substituted. Historical-only coworkers have no current bindings or
category.

```graphql
{ runtime { messageTargets {
  id name handle category
  defaultBinding {
    id adapter { id name }
    model { provider model reasoningEffort }
  }
} } }
```

Binding IDs and adapter IDs are canonical CWR references. Model configuration is
null for non-model adapters. Codex, Claude, and Open Responses expose their
configured model; Codex also exposes configured reasoning effort. Unspecified
settings remain null. Provider remains null when CWR cannot determine it from
the adapter configuration; adapter brand and model names do not establish the
backend provider. These fields never include raw parameters, endpoints,
credentials, or local executable paths.

### Conversation creation and messaging

Use `createConversation(text:, requestId:, ownerContextRef:, fileRefs:)` to
start an independent conversation. `ownerContextRef` selects a pack or Workspace
context; omitting it uses the runtime's default context. Initial message
addressing selects a coworker within that context. Only the supplied text and
attachments enter the new conversation.

`sendMessage(text:, requestId:, conversationRef:, fileRefs:)` requires an
existing conversation and preserves its owner. It cannot create a conversation
or change its context. Reuse the original request ID and exact input to recover
uncertain admissions for either mutation.

Coworkers first call `cwp.cwr--list--conversation_contexts` with `{}` to
discover enabled destinations that include them. Each result includes
`owner_context_ref`, `is_default`, and `actor_refs`. Only configured memberships
are returned; missing or suspended context policy returns no destinations.
Discovery creates no work. Pass a returned owner reference to creation;
admission rechecks availability.

Coworker invocations receive `cwp.cwr--create--conversation` with
`owner_context_ref`, `text`, and optional `file_refs`. The owner is required for
this tool. Authorship comes from the active coworker invocation, not a tool
argument. Context membership, coworker permissions, file custody, and active
invocation fences are checked before admission. The result contains
`conversation_ref`, `message_ref`, and optional `invocation_ref`; it does not
wait for the new conversation's reply. The new work is independently scheduled.
The existing `cwp.cwr--send--message` tool continues a coworker request in the
active conversation.

## Measure runtime publication delay

Run the isolated, provider-free replay benchmark from the repository root:

```sh
direnv exec . python3 scripts/runtime_publication_replay.py --output /tmp/publication-baseline
```

The command repeats progress, tool, streaming and terminal activity with one and
four concurrent workers, varying relevant and unrelated retained history
independently. It writes source/build identity, machine load, ordered timing
samples and latency distributions. See the
[replay workload and timing contract](tests/publication_replay/README.md) for
configuration and interpretation.
