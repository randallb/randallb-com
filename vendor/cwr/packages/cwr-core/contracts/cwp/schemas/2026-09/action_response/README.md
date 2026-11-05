# Action response

Schema ref: `cwp://org.coworkerprotocol/2026-09/schemas/action_response`

[Schema](schema.json) · [Example](example.toml) ·
[All CWR resource types](../../README.md)

An action invocation emits zero or more responses. The actor can report
repeatable progress before exactly one terminal response:

```toml
invocation_ref = "cwp://com.example.runtime/invocations/01J..."
sequence = 1
status = 102
summary = "Drafting the recommendations"
```

The [`action_response` schema](schema.json) preserves the exact numeric status
and derives its meaning from the range:

| Status    | Terminal | Meaning                                                          |
| --------- | -------- | ---------------------------------------------------------------- |
| `100-199` | No       | Progress; may repeat and contains no result, error, or cause.    |
| `200-299` | Yes      | Completed; requires `result`, validated by the output schema.    |
| `400-498` | Yes      | Blocked; requires `error.code` because the request cannot apply. |
| `499`     | Yes      | Stopped by the host; may contain a structured `cause`.           |
| `500-599` | Yes      | Failed; requires `error.code` because execution failed.          |

Statuses below `100`, the entire `300-399` range, and statuses above `599` are
invalid. An actor reports any valid status except `499` through its
invocation-bound response surface. The host adds `invocation_ref` and the next
positive `sequence` when it retains the response. The host alone creates numeric
status `499`.

All responses in one invocation name that invocation and have strictly
increasing sequence numbers. An admitted invocation has no responses. A running
invocation has only progress responses. A terminal invocation ends with exactly
one response whose status matches its state. The first accepted terminal
response wins.

An action response has no independent `type` or `ref`. If an action produces a
document, artifact, or other lasting resource, its completed `2xx` result can
contain that resource's typed ref. Status `202` is terminal for the current
invocation. A synchronous caller remains running until its child settles.

A `1xx` response explicitly published to a conversation also appears as a linked
[progress message](../conversation_message/README.md#published-progress). The
message represents that existing response; it does not allocate another response
sequence number. Completed commentary can be published without a numeric
response. Provider phase status alone does not establish authored publication.

## Authored progress measurements

Only `1xx` responses may include `progress`. It requires a nonblank, single-line
`key`, stable within the action invocation. Optional `phase` and `unit` are
nonblank, single-line action-defined labels. Optional `completed` and `total`
must appear together and be non-negative integers no larger than
9,007,199,254,740,991. Omit unknown counts; derive a percentage only for a
positive total. Optional fields must be omitted rather than set to null.

```json
{
  "invocation_ref": "cwp://com.example.runtime/invocations/tests",
  "sequence": 2,
  "status": 102,
  "summary": "Rust tests: 8/24 test binaries complete.",
  "progress": {
    "key": "rust-tests",
    "phase": "testing",
    "completed": 8,
    "total": 24,
    "unit": "test-binaries"
  }
}
```

The required `summary` remains sufficient for presentation. Existing responses
without measurements remain valid. Updated runtimes and adapters must be
installed together before opting a producer into the richer record; older strict
decoders may reject the new field. Counts should not move backward for one key
unless the phase changes or the summary explains a reset. Keys are scoped to the
invocation, never shared across coworkers or separate calls.

CWR retains every accepted response and its matching activity. Hosts may show
only the latest summary for one key without modifying retained history.
Producers throttle phase and material count updates before requesting
publication. A terminal response alone establishes success, failure, or
cancellation.

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
