# Gateway request

Schema ref: `cwp://org.coworkerprotocol/2026-09/schemas/gateway_request`

[Schema](schema.json) · [Example](example.json) ·
[Response](../gateway_response/README.md)

The local runtime exposes `cwr--send--request` through the adapter host-call
transport (`cwr.send_request`). Callers supply `type`, `resource_ref`, and an
object `request`. Ordinary conversation text never triggers a gateway request.

## Configure a standalone gateway

Import a gateway definition resource in the runtime graph. The definition selects its role, configured integration, and synchronous action; the handler does not need to be imported as an ordinary coworker. See [gateway definition](../gateway/README.md). The graph checker reports the resolved gateway routes. A gateway definition selects routing; it does not grant credentials or network access.

The selected action's input schema validates the entire admitted gateway envelope below, including its contract-specific `request` payload. Its output schema validates the gateway result. Execution uses the existing action and adapter machinery, including caller authority checks and child cancellation. The internal child invocation is retained as ordinary execution history; the caller supplies no action target and receives a gateway response.

## Identity and authority

CWR assigns `ref` and derives `requester_actor_ref` and `parent_invocation_ref`
from the authenticated caller. Caller-supplied identity fields are rejected. The
example shows the admitted envelope, not the tool's input. The schema also
allows `conversation_ref` for hosts that supply it from authenticated context.

`resource_ref` selects the configured contract. It is neither an account
selector nor a permission grant. Configuration selects the handler, and normal
coworker admission still determines whether the caller may execute it.

The request declaration grants no credentials, network isolation, or VM
isolation. Unsupported access fields are rejected rather than silently treated
as grants. Desktop resolves an approved Google token privately for its Google
gateway; the token stays out of operation input, CWP records, and results.
Provider destination and authentication must come from host-approved setup so a
caller cannot redirect credentials to an arbitrary URL. SMS identity does not
grant Google access; the combined SMS-to-Gmail path needs its own origin rule
and verification before live cutover.

## Completion and stopping

CWR wraps the validated action result in one terminal gateway response. Stopping
the parent stops its gateway child through the existing lifecycle. A completed
host-call retry reuses its retained result. This does not guarantee exactly-once
external effects across a crash or undo an external operation on cancellation.
Streaming gateway progress, discovery, and provider handlers remain future work.
