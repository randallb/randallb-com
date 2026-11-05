+++
ref = "~@/policies/cwr_installed_schedule_collections"
+++

# Installed schedule collection v1

Contract ref: `cwp://org.coworkerprotocol/2026-10/contracts/installed-schedule-collection-v1`.

This contract defines one host-built schedule collection for each installed pack
owner context. The native installer binds the owner context when it activates
the pack's verified schedule declarations. The collection is available to the
selected host actor while that owner remains in the host's current context
policy. Disabled or removed declarations remain inspectable; removing a
collection from navigation does not delete schedules or runs. A pack cannot
declare another schedule collection or choose its owner through this contract.

## Membership and identity

The owning schedules service supplies membership through its `schedule_owner`
and `installed_schedule` relationships. Each row has a canonical `schedule_ref`
derived from the installed owner context and stable release declaration ref.
Changing a label, revision, timing or availability keeps that identity. A
different installation of the same release has a different owner and schedule
ref. The collection is an owner projection, not a second writable membership
record. Its display name is **Schedules**.

## Reads

`Runtime.schedules(ownerContextRef: ID!): [InstalledScheduleV1!]!` lists the
installed owner's rows. `Runtime.scheduleRuns(ownerContextRef: ID!,
scheduleRef: ID!, first: Int! = 20): [ScheduleOccurrenceV1!]!` returns 1 to 100
recent occurrences for one row, newest due time first. Both reads require the
authenticated native host actor and a nondefault owner context in the current
host policy. The schedules service verifies each schedule's owning field even after
relationship traversal. An unavailable owner or foreign schedule is rejected;
the caller cannot widen access by supplying another schedule ref.

`InstalledScheduleV1` exposes canonical `id` and `scheduleRef`, owner and author
refs, declaration ref and revision, anchor and interval in Unix milliseconds,
availability, pause state, next due time, active occurrence ref and last
admission error. It does not expose saved action input or exact target bindings.
`ScheduleOccurrenceV1` exposes canonical `id` and `occurrenceRef`, schedule ref,
due time, outcome, invocation ref, conversation ref and archive state. The
conversation ref is an ordinary CWR conversation link, subject to its own read
authority. These rows are embedded selections; the global `Runtime.node(ref:)`
lookup does not resolve schedule records without an owner.

## Controls and recovery

Pause and resume use existing CWR actions `@cwr--pause--schedule` and
`@cwr--resume--schedule`. The native client invokes `invokeAction` with the
installed owner context as `url`, a stable `requestId`, and only `schedule_ref`
in input. CWR creates an ordinary conversation in that owner context, validates
the exact intrinsic route and owner scope, and retains its normal admission
receipt. Repeating the same request and destination recovers the committed
outcome; reusing a request ID with a different owner or input conflicts. The
schedule service performs the pause update in its owning transaction. Resume
sets the next eligible future tick and does not replay missed ticks.
Schedule control action results and retained summaries use the public v1
schedule and occurrence fields above; they do not serialize the saved action
input, exact target bindings, or occurrence definition.
Recovery discloses the retained outcome only while the selected host actor still
has the installed owner in the current host policy; removing that owner does
not create another admission.

An existing conversation can still call the schedule actions using its
retained owner; that path remains conversation-scoped. Scheduled work itself
continues through CWR's occurrence reservation and ordinary action admission.
The collection grants no execution authority and cannot install, edit, delete,
or run a schedule on demand.
