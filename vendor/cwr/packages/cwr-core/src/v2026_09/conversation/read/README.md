# Conversation history and current work reads

`@cwr--browse--conversation` gives a participant small recent or around-message
pages, literal search, reply links, and UTF-8-safe continuations of large text
and structured action input or result. Each entry retains the author, recipient,
message and reply refs, publication position, and known invocation origin.
Browse output has a serialized response limit as well as per-message excerpt
limits; a single large result cannot make a page unbounded. An empty or partial
page does not mean the coworker has read the rest of the archive. Browsing is
read-only and cannot replay historical actions.

`ConversationReadStore::begin_operation` pins a SQLite snapshot. In that same
operation, call `conversation_history(ref, window)` and
`conversation_current(ref)` to read published history and current work without
a mixed persisted revision. Access must be authorized by the calling runtime
service before exposing these read methods, as with the existing indexed reads.

History uses legacy message publication positions until an explicit timeline
exists, then derives each message's compatibility position from its first
placement in the flattened authoritative timeline. It uses the existing
first/after/last/before page window and includes published commentary and action
results exactly once as canonical messages. It returns `ConversationHistoryPage`
with `ConversationHistoryEntry` rows. Each row exposes `origin_invocation_ref`
from its explicit commentary, root admission, or child publication link, including
after completion and reopen. The lookup uses publication edges for only the loaded
page, in the same snapshot. User messages and legacy replies without an explicit
link have no origin; source, author, and text never establish that relationship.
Public commentary and explicitly published progress from nested execution are
stored in the root shared history, while the underlying execution activity stays
in its private conversation.
Existing `ConversationContentPage` and `ConversationContent` shapes are unchanged.
The message's `progress` identity
links commentary to its invocation and source activity. Activity records and
provisional previews are not extra history rows. Existing `conversation_content`
and history reads retain their prior rendering semantics for legacy conversations.
For an explicit timeline they expose only messages placed in the reachable
canonical topology. A message placed more than once remains one history record;
the timeline carries its distinct placements, order, and nesting. Exact-message,
search, reply-link, excerpt, and canonical-resource reads apply the same
visibility rule, while current-work reads remain operational and can still show
an active invocation before publication. Sidebar previews and completion history
likewise derive only from placed message and invocation content once the explicit
root exists; an authoritative empty timeline exposes neither.

The explicit-timeline foundation adds direct connection and item contracts.
It uses the existing CWPDB nodes, edges, and ref bindings: the conversation
points to one root connection, and timeline items point to canonical message or
invocation records. There is no embedded conversation copy or additional table.
Once a conversation has `timeline_connection_ref`, a participant reads its root
or a child connection with `first`/`after` or `last`/`before`, then resolves the
linked message or invocation through an authorized canonical-content read. A
connection cursor remains valid after later appends; its revision fences the
returned snapshot but is not part of cursor identity. During rollout, the
existing `conversation_timeline` method is a flat compatibility read. For an
explicit timeline it now derives a depth-first, parent-before-children view from
the retained topology at query time; a present empty root stays empty. That
adapter adds read-time traversal and temporary dual API semantics, but stores no
second order or visibility state. Legacy conversations continue to use the old
projection until migration. No host command exposes the new connection reads
before retained writes and legacy migration are enabled.

`migrate_legacy_conversation_timeline` is the dormant per-conversation cutover.
It first derives deterministic root, publication, item, and child identities,
then builds a canonical shadow timeline and compares its flattened anchors with
the legacy projection. Only a quiescent, unambiguous conversation is committed.
The timeline, every invocation disclosure decision, and the conversation pointer
share one CWPDB transaction, so interruption leaves the conversation legacy and
a completed retry reports the existing root. Active work or an ambiguous causal
source is skipped without guessing; identity collisions fail and roll back the
whole conversion. Legacy progress messages or invocations without standalone
anchors are also left legacy because synthesizing placements for them would
change visibility or flat order.

Current work includes every nonterminal invocation in the conversation, even
when its source message is outside the loaded page or its parent is terminal.
Invocation IDs distinguish simultaneous calls by the same actor. Parent/child
links and explicit waits remain separate. `conversation_invocation(ref, id)`
loads active or terminal details by identity. It does not expose private inputs,
provider history, or wait response payloads. Child terminal publication atomically retains the exact reply message ref on
the invocation; both detail reads and publication replay use that link. Older
records without it use source/author matching only if one invocation and one
message qualify; ambiguous legacy replies remain unassigned. Root terminal
replies use the admission's explicit reply ref. Published commentary uses its
explicit invocation link. No text matching is used.

Terminal invocation details include `outcome` even when no reply was published:
the exact response status, summary, and optional error/cause code and message.
The SQL view selects only these public fields, excluding result payloads
and arbitrary error/cause `details`. Active invocations have no outcome; progress
responses cannot become terminal summaries.

The invocation start is the first claim. Each subsequent claim increments the
execution generation and retains its own start and preceding activity sequence
atomically. Phase updates cannot reset that timer. Activity time is the latest
retained event in that execution, or its start. Older resumed records without
these fields report an unknown current start and no old progress until their
next claim. A terminal invocation has no current execution. An inactive claim
must not be presented as actively generating output.

`apply_live_output` accepts only a matching invocation, actor, and actively
claimed execution generation. An empty text clears the preview, not the
invocation. SQLite does not persist that preview. The eventual subscription
layer must attach its authoritative transient state, order updates, and replace
current snapshots on completion/recovery, including an empty active collection.

`ConversationSnapshotPosition` identifies runtime, conversation, and database
commit revision. It is **not** a history cursor or a usable stream resume token.
The subscription layer must add runtime epoch/transient event position and
provide an atomic snapshot-to-stream handoff or explicit reset. This read
foundation does not implement subscription delivery or replay.

## Publication selections

`LocalRuntime::publication_snapshot_until` authorizes the actor and upstream
session scope, then opens a read-only SQLite view for one consumer selection.
Conversation metadata, participant authorization, lists, publication pages,
nested pages and source records use that same captured commit. Each subsequent
subscription selection opens a fresh view. Drop releases the transaction; the
selection deadline and cancellation interrupt all nested SQL reads.

Publication pages use `conversation_timeline_connection` to fetch only the
requested siblings. `conversation_timeline_item` resolves a reachable placement
without loading the whole topology. An exact direct-sibling count is available
through `conversation_timeline_connection_count`; callers request it only when
selected. Source records retain the canonical visibility rules above. A missing
or inaccessible source yields an unavailable presentation while retaining its
publication item. An absent legacy root is not synthesized by the read path.

Streaming message records are separate from retained conversation messages and
have no history membership edge until CWR publishes them. Canonical publication
connections include them, while flat compatibility history omits them. The
resource reader reports whether the message is streaming; publication text,
source lookup, sibling counts and nested fields share the same SQLite snapshot.
Finalization removes the provisional marker in the transaction that records the
retained message. Withdrawal deletes only provisional sources and placements,
leaving monotonically allocated sibling positions and durable identity claims.
A cursor pointing at a withdrawn item fails and requires a fresh page.


Publication filtering uses `ConversationPublicationFilter` on the same frozen
view as source resolution. Normalization trims/lowercases search and sorts and
deduplicates source kinds; empty and all-kind selections mean no kind
restriction. Search matches shared public presentation text, including current
streaming text. It examines only the selected sibling connection. The reader
uses 100-item indexed batches, retains at most the requested page plus one
match, and scans all siblings for an exact count only when requested. All
scanning and hydration share the deadline and cancellation of the selection.

Message metadata follows exact retained source-activity and invocation links;
ambiguous historical action associations remain unavailable. Author labels use
retained local identity evidence and preserve known coworker classification.
Related-actor reads cannot expose another conversation's identities. Attachment
reads authorize a visible associated message before returning canonical
visualization metadata or separately requested HTML. Unassociated artifacts
remain unavailable. Recipient and attachment filters are shared CWR rules, and
all nested values remain at the selection's captured commit.
