# CWR Core

This crate owns the presentation-neutral Coworker Runtime: resource-graph
loading and checking, reference identity, conversations, actions, persistence,
adapter protocol and routing contracts, and runtime records. The CLI and host
applications depend on these APIs; core does not depend on an application or a
concrete adapter implementation.

Bundled adapter executables live in `../adapters`. Generic applications live in
`../../apps`.

## Execution and retained state

Local admissions retain a fingerprint of their execution route, including the
adapter, working directory, parameters, and authority. Execution checks that
binding against the current graph. A changed route fails the queued request with
a resubmit message before starting its adapter. Pending requests saved by older
builds without this binding also need resubmission. The fingerprint is host
metadata and is removed before process transport.

Hosts can call `LocalRuntime::try_schedule_execution` to reserve capacity before
creating a worker. Keep the returned task until the worker and its output
publisher finish. New admissions and recovered work use the same retained queue;
a separate single slot lets runtime control commands run while conversation
workers are occupied.

Routine persistence saves undo records for the conversations and invocations
being changed, validates those changes, and writes them in one SQLite
transaction. A failed commit restores those records in memory. Full validation
still runs when opening retained state. Host projections borrow unrelated
history; they copy only the conversations being presented.

The resident service disconnects clients whose output queues fill. Clients must
reconnect and request a snapshot to recover retained results. Publication keeps
one event order without waiting for a client to drain its socket.

## Reference and action contracts

The
[coworker, role, and tool-activation policy](../../memos/policy/cwr/cwr-subjects-actors-and-tool-activation.md)
owns action naming and exact target selection. Tools use CVO names:
`coworker--verb--object`, preserving underscores within each field. A local
handle is presentation; admission retains the exact runtime, concrete coworker,
owning role, named binding, and action contract.

The `refs` module implements the protocol's `cwp://...` grammar. A `CoworkerRef`
identifies a concrete coworker. Roles supply reusable behavior and own semantic
`<verb>--<object>` action interfaces. Actions do not acquire invented
`/actions/...` resource refs. Internal compatibility names do not change these
identity and routing rules.

## Shared conversation reads

The additive
[history and current-work API](src/v2026_09/conversation/read/README.md)
provides paginated published messages and complete active invocation reads in
one SQLite operation, independently of Desktop and TUI rendering.

## Ref storage API

The [Ref Storage API contract](./resources/reference/ref-storage-api.md) defines
the first resource CRUD, link/unlink, and traversal operations before
implementation.
