# Conversation caller boundary

External domain callers bind `LocalConversationModule` with the authenticated
actor and ingress. Importing the module grants no authority. Product hosts may
bind the bundled copy for packs that predate explicit imports. The owning
admission, read, transfer and archive implementations retain authorization,
transaction, retry and recovery behavior.

| Caller | Service operation |
| --- | --- |
| GraphQL message creation/continuation | `publish_message_in_context` |
| GraphQL publication fields and subscriptions | `publication_snapshot_in_context` |
| GraphQL action destination visibility | The same actor-bound publication snapshot |
| Host text command and receipt recovery | `publish_text_in_context`, `replay_text_in_context` |
| Provider ingress | `admit_provider_ingress` |
| Conversation list/read/browse intrinsics | `list`, `read_command_timeline`, `browse` |
| Rename/export/import/fork intrinsics | `rename`, `export`, `import`, `fork` |
| Host selected/conversation stop and receipt replay | `stop` |
| Archive queue and execution | `reserve_archive`, `set_archived_with_stops` |
| Local host exchange publication | Trusted host `publish_exchange` |
| Attended TUI rename | `rename` |

The archive reservation is obtained in command-reader order, before the worker
is queued. The owning archive operation rechecks authority, stops running work
and commits through its existing orchestration. The service does not replace
this with an ordinary property write. Upstream connection ownership and a pack's
owner context remain separate inputs.

`LocalConversationHost`, returned by `conversation_host`, is the trusted native
embedding adapter within this boundary. Host exchanges, bootstrap, receipt/broadcast
projections, turn statuses and compatibility host-record reads retain their
existing runtime authority, cancellation and projection rules. This adapter is
not a participant API and is never returned by an imported pack binding. It
does not manufacture an actor to route server-internal transport work through
participant checks.

Owning execution internals remain behind the service: action invocation
admission, provider continuation, scheduling, terminal/progress publication,
recovery and shutdown operate under their existing invocation/runtime
contracts. Their direct internal calls are intentional. Tests and isolated
migration fixtures can also call lower-level machinery. These are different
from an external caller bypassing conversation admission or reads.

`apps/cwp-runtime/tests/conversation_boundary.rs` checks production application
sources for direct calls to lower-level conversation entry points. Add a real
service operation when a new external caller needs one; do not weaken this
check or wrap an unscoped runtime handle in a product component.

Core conversation SQL remains within the owning CWR persistence/read
implementation. Desktop sidebar pins, ranking, preferences and completion
attention retain their separate product-state owner; this migration does not
claim those databases have moved into core conversation history.

Desktop `apps/workloop-desktop/pending_conversation_send_store.ts` owns recovery
intent SQL scoped by the authenticated workspace, runtime and actor context;
these product records remain separate from CWR history and admission receipts.
