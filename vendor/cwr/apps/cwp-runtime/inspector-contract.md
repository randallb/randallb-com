# Invocation inspector reads

The inspector reads detailed tool work and execution relationships from CWR.
Desktop owns selection, collapse state, navigation, and rendering. Publication
nesting, message replies, calls, and waits remain separate relationships.

## Available fields

Every `ActionInvocation` lifecycle object exposes `activities`, `calls`, and
`waits`. Each connection accepts `first`, `after`, `last`, and `before`,
defaults to 50 entries, and permits 1–100 entries with either `first` or `last`.

```graphql
query Inspector($ref: ID!, $after: Cursor) {
  runtime {
    invocation(ref: $ref) {
      activities(first: 20, after: $after) {
        coverage
        pageInfo { hasNextPage endCursor }
        edges {
          cursor
          node {
            id kind label labelTruncated state active startedAt finishedAt
            detailsAvailable
            childInvocation { id state }
            command { text truncated }
            output { text truncated }
            error { text truncated }
            exitCode
            files(first: 10) {
              pageInfo { hasNextPage endCursor }
              edges { node { path pathTruncated changeKind diff { text truncated } } }
            }
          }
        }
      }
      calls(first: 20) {
        coverage
        edges { node { callId caller { id } callee { id state } } }
      }
      waits(first: 20) {
        coverage
        edges { node { callId state waiter { id } awaited { id state } } }
      }
    }
  }
}
```

The example lists the available selections; select smaller subsets as needed.
Published progress remains `Message.progress` on publication items. Activities
are tools and delegated work, not another copy of published progress text.
Filtered publication windows do not establish full execution history.

## Sources, identity, and coverage

Activity reads follow invocation -> `tool_operation` edges to canonical
operation nodes. File changes have their own nodes reached through `tool_file`
edges. Provider events are normalized when accepted during an active execution
attempt. Identity is scoped to attempt, provider, and provider item ID;
subsequent updates mutate that operation rather than append display snapshots.
The operation owns its lifecycle state. Raw events remain supporting execution
history.

Canonical timeline activation and tool capture are always enabled for local
runtimes. Startup migrates quiescent legacy conversations, retaining progress
under its original invocation. Active legacy conversations are retried after
settlement. An invocation must have a canonical publication before its attempt
begins; capture decisions remain recorded per attempt. Existing tool history is
not fabricated or rebuilt.

Operations are ordered by first source sequence, then canonical ref; updates
keep the same cursor position. Separate attempts cannot share operation identity
even when the provider reuses an item ID.

Calls come from invocation origin records. The pair (caller invocation ref,
`callId`) identifies the admitted call. Ordering uses admission sequence and
call ID; legacy missing sequences sort as zero with the call-ID tie-breaker.
Call cursors never contain a hidden callee ref. This connection covers retained
admitted calls, not failed pre-admission call attempts. It is independent of
publication nesting.

Waits come from their own ledger records. Identity is (waiter invocation ref,
`callId`); order is the call ID. The enum preserves PENDING, READY, RESUMING,
CONSUMED, and ABANDONED. The current runtime executes child calls synchronously
and does not create new suspension waits. Older waits remain readable after
startup recovery; this API does not reintroduce suspension or fabricate waits
for synchronous calls. Internal wait response payloads are excluded.

Activity coverage is PARTIAL: captured supported tools do not claim exhaustive
historical provider coverage, and earlier attempts may predate capture. Calls
and waits report COMPLETE for their retained ledger collections. File changes
report COMPLETE for the selected canonical operation's file list. Coverage
describes those sources, not all work the provider might have performed.

A null activity connection means no authorized captured attempt is available. A
nonnull connection with no edges means a captured attempt contains no supported
tool operations. Disabled and pre-capture attempts are unavailable, not empty.
Calls/waits on a readable invocation can be empty. Child invocation
relationships are null when the linked invocation is unavailable under the
session's visibility rules.

## Authorization and limits

Basic activity identity, kind, state, and timestamps require visibility of the
owning invocation in a conversation the session actor participates in. Detailed
payloads require that actor to be the direct requester, direct target, or
requester of a retained ancestor invocation. This is read authority only.

Other participants receive the label `Tool activity`, `detailsAvailable: false`,
and null command/output/error/exit-code/file selections. These checks live in
shared CWR reads and use the same frozen snapshot as source visibility. Knowing
a ref or cursor grants no extra access. Raw provider events, routing,
credentials, arbitrary host arguments/results, and private diagnostics are
excluded. A path is display data, never a browser fetch URL.

Labels are limited to 1,024 Unicode characters and paths to 4,096, with explicit
truncation flags. Kind/state metadata is limited to 256 characters. Command,
output, error, and diff previews contain at most 16 KiB of UTF-8, truncated at a
character boundary, and expose `truncated`. Null means absent or unauthorized;
previews are not full-output download endpoints. Exit codes are GraphQL Ints.

Shared inspector reads enforce a 512 KiB aggregate data budget per snapshot.
Selections requesting inspector fields also have a 512 KiB serialized GraphQL
data limit over HTTP and each WebSocket response, including scalar aliases.
Request a smaller page or fewer previews if the budget is exceeded. Existing
selection deadlines, cancellation, query complexity, and depth limits still
apply. Reads retain at most a page plus one result; exact counts are omitted.

Cursors bind actor, session, invocation, connection kind, and entry position.
Foreign scopes, changed file anchors, missing entries, and inverted ranges are
errors. Activity state/output updates preserve activity anchors. File cursors
are scoped to the latest projected activity update and file index; fetch a fresh
file page after that update changes. File changes do not receive invented
canonical resource identities.

## Live state and time

Queries and `subscription.runtime` use the same CWR reads, with one SQLite
snapshot per selection. Subscription events replace the selected window;
intermediate updates may be coalesced. Reconnect obtains current retained state
without replaying host events. Cursors are positions, not replay tokens.

`active` is true only for a running activity in the current actively claimed
attempt. An old unfinished activity can retain state `running` while `active` is
false; clients should not animate it as current work.

A completed activity exposes fixed `finishedAt` from its retained start and
duration. An unfinished or interrupted activity without retained finish evidence
has null `finishedAt`; this API does not start a timer or invent a terminal
state. Completed durations stay fixed across retries. Invocation total elapsed
time remains separate until CWR defines queued/suspended/retry duration policy.

Message `occurredAt` behavior is unchanged: streaming time is currently null;
publication supplies the retained message-recorded timestamp. Stable message and
placement IDs do not promise stable time across finalization.

## Remaining work

Attached/detached execution needs domain-owned retained evidence; no such field
is fabricated here. Full output access, recorded action input/output, user
controls, pending person requests, and general image/file/citation attachments
remain separate contracts. Desktop must refresh its native SDL and generated
Isograph artifacts before adopting these fields.
