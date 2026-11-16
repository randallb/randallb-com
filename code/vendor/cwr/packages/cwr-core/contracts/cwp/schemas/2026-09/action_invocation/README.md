# Action invocation

Schema ref: `cwp://org.coworkerprotocol/2026-09/schemas/action_invocation`

[Schema](schema.json) · [Example](example.toml) ·
[All CWR resource types](../../README.md)

Current CWR admissions request a concrete coworker to perform a role-owned
action. The invocation record retains historical actor field names:

```toml
type = "cwp://org.coworkerprotocol/2026-09/schemas/action_invocation"
ref = "cwp://com.example.runtime/invocations/01J..."
requester_actor_ref = "~@/actors/randallb"
target_actor_ref = "~@/actors/looper"
conversation_ref = "cwp://com.example.runtime/conversations/product_review_01"
source_message_ref = "cwp://com.example.runtime/messages/01JREQUEST"
timeline_publications = { "cwp://com.example.runtime/publications/01J..." = { kind = "root" } }
actor_integration_ref = "~@/actor_integrations/looper_slack"
action_name = "draft--brief"
input = {}
state = "running"
```

For current admissions, `requester_actor_ref` and `target_actor_ref` identify
the concrete requesting and target coworkers; they do not introduce separate
actor identities. Retained pre-migration records may instead identify standalone
actors under their original contracts. Preserve those exact refs; do not map
them to current coworkers or reinterpret them through current aliases. See the
[retired actor contract](../actor/README.md) and
[migration guide](../../../../../../../memos/runbooks/migrate-cwr-coworkers-and-roles.md).
`actor_integration_ref` retains the selected execution or delivery route under
its historical field name. Current admission selects the target coworker's exact
named integration binding and owning role; standalone actor-integration
declarations are retired. The selected binding's adapter must support every role
selected by the coworker. The selected action's input schema validates `input`.
The invocation retains an ordered `responses` sequence whose values follow the
core action-response schema. The selected action's output schema validates a
successful response's `result`. An adapter's internal provider call or
continuation does not create another action invocation.

`conversation_ref` names the requesting conversation, where the request and
final response are recorded. `source_message_ref` names the message that
directly caused the invocation. It is a causal link, independent of timeline
adjacency, reply links, and the placement beneath which the invocation is
presented. Legacy records may omit it until deterministic migration can
establish the relationship. `timeline_publications` maps each absolute,
unfragmented publication ref to its append-only disclosure decision: `root`,
`child` with an absolute, unfragmented `parent_item_ref`, or `undisclosed`. Map
keys make publication identity unique. Distinct publication refs may place the
same invocation more than once. The source and each new decision are recorded
atomically with its placement. A disclosed timeline item points back to this
invocation by ref; `undisclosed` deliberately creates no item.

The runtime links the invocation to a separate execution conversation whose
parent is the requesting conversation. The action's `execution_scope` selects
which conversation owns collaboration. Membership is local to that conversation;
the parent link does not inherit participants or expose their actions.
Requesting context can be supplied explicitly without changing membership.
Omission of `conversation_ref` means the invocation is not part of a
conversation; the host must not infer one from its actors or parent.

Recovery state belongs to one invocation and its selected integration binding.
Separately, supported conversational adapters can continue settled private
history across invocations for the same configured runtime, selected volume,
concrete coworker, named integration binding, and requesting conversation. The
execution records retain that association, the previous settled invocation, and
confirmed shared-source coverage. Each new request has its own invocation,
authority, calls, and terminal state. A provider identifier alone establishes
neither continuity nor safe replay; the adapter validates the history and its
compatibility with the current execution contract.

An action invocation has one of these states:

- `admitted` means CWR accepted the request but has not started the target.
- `running` means execution has started and the invocation is runnable or has an
  active adapter attempt. An active attempt may emit `1xx` progress responses.
- `completed` is terminal and ends with an exact `2xx` status.
- `blocked` is terminal and ends with an exact `400-498` status.
- `failed` is terminal and ends with an exact `5xx` status.
- `stopping` means CWR accepted a stop and retained a host-generated numeric
  `499` response, but cancellation has not settled. The invocation cannot resume
  or accept further provider responses as its outcome. Its executor or an
  affected descendant may still be running.
- `stopped` is terminal with the same host-generated numeric `499` response.
  When settlement is tracked, cancellation has settled for the invocation and
  its affected descendants.

An admitted invocation may omit `actor_integration_ref` while admission selects
an allowed route. The selected ref must be recorded before an adapter-backed
invocation becomes `running`; the structural schema enforces that transition.
Once selected, the route remains on every later record. An invocation stopped
before execution may never select a route and may omit `actor_integration_ref`.
A stopping or stopped invocation with a retained `started_at` or `1xx` progress
response has begun execution and must retain the selected ref.

Synchronous actions and executable message exchanges keep the original adapter
attempt running until the child has a canonical terminal response. That response
returns to the original tool call, preserving the caller's working state.
Asynchronous actions return an admitted invocation reference. Their eventual
result is recorded in the requesting conversation without starting or resuming
the parent.

`cwr.await_invocation`, persisted suspension, and automatic fresh-turn
resumption are not supported. During upgrade, previously suspended executions
settle with an explanatory failure, preserving child outcomes and never
replaying completed work. Status `202` remains a completed terminal response.

Crash recovery may restore an execution only when its adapter can preserve the
recorded input, working state, and original calls, including any settled child
result. Otherwise CWR reports an interrupted execution and retains completed
outcomes. Failure to deliver a completed result must not execute the action
again.

Recoverable provider preparation must record which source records were confirmed
or covered by compaction and whether live execution began. Validate that state
against the invocation, selected integration, recorded source, and configuration
that determines initialization. A provider session identifier alone does not
establish source coverage or safe recovery. Resolve an uncertain acknowledgment
before reusing preparation state; rebuilding preparation from the same recorded
source is permitted only when live effects have not begun. Unsupported recovery
must leave completed outcomes intact and report the interruption.

The intrinsic `stop--invocation` action requests a stop by invocation ref and
may include a human-facing reason. The request is recorded on the invocation; it
does not create a separate cancellation resource. Stop acceptance closes the
response outcome immediately, while `stopping` distinguishes that decision from
confirmed executor exit. If completion and stopping race, the first accepted
terminal response wins. Late provider output cannot revive a stopping or stopped
invocation.

CWR records `stop_requested_at`, the requesting actor, and the reason when it
accepts the stop. Replaying the same stop preserves that original decision.
`finished_at` remains absent while the invocation is `stopping`. The invocation
becomes `stopped` only after its executor confirms exit and every affected
stopping descendant settles. Unclaimed work with no unsettled descendants can
become `stopped` immediately. A disconnected provider, a timeout, or reopening
the store does not establish executor exit; unconfirmed cancellation remains
`stopping`. Older stopped records whose settlement was not tracked cannot be
used as evidence of executor exit or assigned an invented settlement time.

The requester actor, target actor, or another actor authorized by host policy
may request the stop. Adapter credentials and possession of an
`actor_integration_ref` provide transport, not authority.

A child invocation records `parent_invocation_ref`. There is no attachment
choice. Explicitly stopping a parent recursively stops every nonterminal
descendant, including descendants below completed intermediate invocations. A
normally completed parent leaves asynchronous children running; a later explicit
stop still stops its running descendants without rewriting the parent's
completed result. A parent whose own executor exits first remains `stopping`
until the affected descendants settle. Accepting a child stop returns the
child's numeric `499` response to a live synchronous caller; that response does
not by itself establish that cancellation has settled. A stopped caller cannot
be resumed by later child output. Cancellation follows invocation ancestry
regardless of execution scope or response mode.

The host assigns a lasting invocation ref below a stable runtime-owned resource
root. It may expose the invocation through a mount-relative `~@/invocations/...`
alias, but that alias is not the invocation's portable identity. The host may
store the record inside or outside the current root without changing its
absolute ref.

## Concrete coworker and configuration context

Current requests identify concrete coworkers. Preserve exact configured runtime,
selected volume, owning role, VO action, named integration binding, and action
contract selection at admission. CVO shorthand resolves once; queuing, retries,
and default changes cannot retarget it. Retained serialized actor fields in
historical records keep their original concrete identities and are never
resolved through today's aliases.

Shared requests, useful updates, and results retain their authors, recipients,
conversation boundaries, and invocation ancestry. Private history belongs to its
runtime, volume, coworker, binding, and requesting conversation. Contract
changes, unsupported historical decoding, and unavailable targets report failure
without starting recipient work or rewriting prior messages, calls, results, or
checkpoints. Retry initialization must remain exact and current permissions
still apply.
