# Gateway response

Schema ref: `cwp://org.coworkerprotocol/2026-09/schemas/gateway_response`

[Schema](schema.json) · [Example](example.json) ·
[Request](../gateway_request/README.md)

The local runtime implements a synchronous terminal-only subset through
`cwr--send--request`: sequence 1, status 200 with a contract-validated result,
400 for an invalid request, 403 for unavailable or unauthorized execution, 404
for an unsupported contract, 499 for a stopped gateway child, or 502 for handler
failure or invalid output. Raw handler errors are not forwarded in the gateway
response. The underlying child action retains its normal execution records;
gateway implementations must keep secrets out of those records too. Streaming
gateway progress remains a contract for future implementation.

A response names the gateway request through `request_ref`. Like an
[action response](../action_response/README.md), it has no independent `type` or
`ref`. The host assigns strictly increasing positive sequence numbers, with zero
or more progress responses followed by exactly one terminal response. The first
accepted terminal response wins; no response follows it. These cross-record
rules require host validation beyond the JSON Schema.

Reuse the action-response status meanings: `1xx` progress, `2xx` completed with
`result`, `400–498` blocked with `error`, `499` stopped with optional `cause`,
and `5xx` failed with `error`. Only the host reports these gateway responses.
Result content belongs to the selected gateway contract. Credentials never
appear in responses, summaries, or errors.

Protocol status describes gateway execution. For the illustrative HTTP contract,
`result.status` is the upstream HTTP status and `result.body` is its response
body. A completed HTTP exchange can have outer status `200` and an upstream
error status such as `403`; callers must inspect the upstream status. Failures
before an HTTP response use the outer protocol error. This avoids confusing a
completed transport operation with provider-level success.

The example pairs with the request example and contains fictional Gmail IDs. No
mailbox was accessed. HTTP payload details remain part of the service contract,
not the generic gateway envelope.
