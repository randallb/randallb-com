# Installed schedules spike

Compose `CWR.toml` to import the exact supported service, entities and typed
relationships. `LocalRuntime::imported_schedules_module` validates those exports.
The native Desktop installer binds the bundled implementation through
`schedules_module_as` using its authenticated actor and installed owner context.
Importing declarations grants no authority and ordinary coworker actions cannot
install schedules or select another owner.

A release declares one fixed UTC interval of at least 1,000 milliseconds, anchor, saved input, and source
coworker/action selector in `pack.toml`. The native host verifies the installed
release and maps that source member to the installed actor. Activation resolves
one exact runtime/actor/role/binding/contract target and validates input. The
installed schedule identity hashes installation context plus release declaration
identity. Authored revision is separate from retained pause, availability, active
occurrence and next eligible deadline.

Desktop installation contexts currently use `urn:uuid`. Their exact value stays
in `owner_context_ref` and is passed unchanged to ordinary action admission. The
schedule-owner entity uses a deterministic CWP storage binding for that native
context record; it does not replace the context identity or grant authority.
Typed edges organize the retained records; owning fields and authenticated
bindings decide access even if an index is stale.

The runtime worker reserves an occurrence and the installed schedule's active
pointer in one graph transaction. Ordinary admission commits independently using
the occurrence ref as its ingress identity. Pending reservations reconcile the
existing receipt before checking whether new work is paused, unavailable or
stale. Owner, exact target and input conflicts reject; committed receipts remain
recoverable after target removal. Unknown admission outcomes remain pending.
One admitted/running occurrence blocks overlapping ticks. Offline ticks are
skipped and resume chooses a future tick. No sidecar or second database is used.

All schedule persistence uses the existing runtime cwpdb graph through
EntityCatalog and typed unordered relationships inside the schedules service.
There is no physical schema change. Conversation/invocation services continue to
own their records, receipts and history. Schedule graph transactions never span
ordinary action admission and never assume a cross-record admission transaction.

The native timer is independent of views. Controls are ordinary CWR actions:
`list--schedules`, `get--schedule`, `pause--schedule`, `resume--schedule`, and
`list--schedule_runs`. In a conversation their owner comes from its retained
context. The native installed-schedules-v1 collection may invoke the same
actions with an authenticated owner destination, which creates an ordinary
owned conversation and durable action receipt. The schedule ref cannot widen
either owner scope. Desktop invokes them through its GraphQL/Isograph action
boundary.
Each run gets a fresh visible `CreateForAction` conversation. Terminal success,
failure or cancellation makes it eligible for archival through the conversation
service. Failed archival retries independently and never repeats execution.

This remains a single-runtime spike. The retained run listing uses a bounded
20,000-edge scan that returns an explicit limit error, then sorts the requested
recent page; production needs an owning bounded time-ordered query. This service
provides no editing/deletion, Run now, calendar recurrence, workflow retries,
failover or migration of the closed scheduler spike. No merge/release/customer
activation is authorized by this work.

Activation reconciles the complete current declaration set: omitted schedules remain inspectable as removed and cannot admit new work. Re-activation by an authenticated viewer preserves the original author. Timer results report committed conversation archives for native publication independently of new admissions.
