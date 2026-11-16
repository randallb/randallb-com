# Conversation

Schema ref: `cwp://org.coworkerprotocol/2026-09/schemas/conversation`

[Schema](schema.json) · [Example](example.toml) ·
[All CWR resource types](../../README.md)

A conversation is a shared collaboration context. Its participants are concrete
coworkers. The retained `participant_actor_refs` field uses a historical name
for their refs; it does not introduce another identity layer.
`timeline_connection_ref` identifies its authoritative root publication
connection:

```toml
type = "cwp://org.coworkerprotocol/2026-09/schemas/conversation"
ref = "cwp://com.example.runtime/conversations/product_review_01"
participant_actor_refs = [
  "~@/actors/randallb",
  "~@/actors/looper",
]
timeline_connection_ref = "cwp://com.example.runtime/conversations/product_review_01/timeline"
```

A conversation may have one mutable `title` of at most 120 Unicode characters.
CWR trims surrounding whitespace and rejects blank titles, line breaks, and
control characters. The title is presentation metadata: changing it does not
change the conversation ref, membership, messages, activity, or invocation
history. Hosts fall back to their existing label when the title is absent.

Conversation membership is contextual state, not another identity layer.
Messages identify their author and addressees with concrete coworker refs,
retained under historical actor field names. Addressing records who the author
selected; it does not create a participant-specific publication history. Content
published into the conversation is available to current and future participants,
subject to authorization when they read it. A placement records historical
disclosure but does not itself grant current access.

A conversation selects the published
[`conversation_message`](../conversation_message/README.md) value schema for
each item in its optional `messages` array. The message's `addressing_mode`
preserves whether the author selected its recipients explicitly or CWR inherited
them from context.

A conversation does not contain provider continuation. The host may retain
private recovery state for an invocation and continuing working history scoped
by configured runtime, selected volume, concrete coworker, named integration
binding, and requesting conversation, as defined by the
[invocation contract](../action_invocation/README.md). Execution records link
the history across requests; the shared conversation alone is insufficient to
identify it. Continuation is not a participant or another CWR resource.

The messages array includes published commentary and progress as well as
requests and final results. When `timeline_connection_ref` is absent, the record
is legacy and message order remains its compatibility publication order. When
the ref is present, the explicit timeline is authoritative: messages retain
canonical content while timeline items own publication placement and order. A
present but empty root connection means the conversation has no publications; it
must not fall back to projecting the messages array.

The timeline is a tree of bidirectionally pageable ordered
[`conversation_timeline_connection`](../conversation_timeline_connection/README.md)
values containing
[`conversation_timeline_item`](../conversation_timeline_item/README.md)
placements. Each placement links to canonical message or invocation content.
Connections order only their direct children. A child append does not reorder or
revise the root. Items, publications, and content retain distinct identities,
and the same content can have more than one placement.

A message's optional `progress` value distinguishes an interim contribution from
a settled reply and links to its invocation and source activity.

An action invocation names `conversation_ref` when the conversation's visible
history supplies its thread context and `source_message_ref` names the message
that directly caused it. Neither causality nor presentation parentage is
inferred from adjacency. Invocations outside a conversation omit the field.
Parent and child invocations do not inherit a conversation implicitly.

Every participant can address every other participant, regardless of who added
them. Admission and shared membership follow
[CWR Conversation Participation and Discovery](../../../../../../../memos/policy/cwr/cwr-conversation-participation.md).
Membership does not grant authority to perform an operation.

An accepted request establishes participation in its selected conversation.
Loading declarations, imports, discovery, and tool preferences add nobody and
start no work. There are no automatic participant defaults.

Legacy host presentation can combine canonical messages with retained work
events. `message_recorded` associates a message ref with its publication time
without changing the message's author, recipients, or reply link. Public
`work_phase` events carry metadata-only execution phases and optional
context-part progress; they never contain private reasoning or provider
payloads. `work_lifecycle` projects admission, actual execution start, stop
request, and finish from the invocation ledger, including the parent and any
synchronous caller. Its stable identity represents a lifecycle snapshot, not a
new event on every refresh.

Once a conversation has an explicit timeline, that retained topology is the only
publication authority. A compatibility read may flatten it for an older client,
but a host must not independently project a competing order or nesting. Private
execution conversations and provider continuation do not have a shared timeline.

Runtime-managed shared conversations receive canonical timelines and tool
capture without a rollout setting. On startup, the managed writer automatically
migrates compatible quiescent legacy conversations, preserving retained message
attribution and order, including progress and nested work. Active conversations
are retried after settlement. Ambiguous or incompatible legacy histories remain
unchanged rather than receiving an invented publication order. Existing
canonical timelines remain readable and maintained by the atomic writer.

Activation does not enable legacy export, import, or fork: those paths continue
to reject timeline-bearing conversations until transfer preserves visibility,
repeated placements, sibling order, and nesting. Legacy ordering logic can be
removed only after migration and transfer support are complete.

Hosts distinguish admission from execution and elapsed invocation time from
intervals of attributed work. A synchronous caller may retain an active adapter
attempt while blocked in a child call. Its retained call, child's response mode,
and host result establish that wait and resumption; `running` alone does not
mean the caller is currently generating output. Public work facts retain their
original timestamps and identities across reconnects and duplicate snapshots.
Each active worker also carries its delegation-path actors and the start of its
current execution turn. Hosts can therefore show the focused call chain while
keeping the root request as the stop scope, and reset the leaf's elapsed clock
when execution resumes after a child or tool returns.
