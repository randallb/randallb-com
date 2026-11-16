# CWP Open Responses subset (2026-04-24)

This directory defines the Open Responses shapes that CWP adapters may exchange
with a model provider. It is a dated subset, not a claim that CWP implements an
Open Responses server, client, transport, or the complete 2026-04-24 version.
Anything not listed in [`profile.json`](profile.json) is outside the
compatibility claim.

The upstream OpenAPI document is checked in as [`openapi.json`](openapi.json).
It is the exact 138,849-byte document published at
`https://www.openresponses.org/openapi/2026-04-24/openapi.json` from upstream
commit `4849f32442bc380852bb4068f0b6cfff347c2baa`. Its SHA-256 digest is
`d598753c3a86fd8a2434828fe39dcc8786a7a694bbe46080a0d24c9fa40e72df`.

## Directions

`adapter_to_provider` means CWP adapter input sent to the model provider.
`provider_to_adapter` means model output received by the adapter. `replay` means
an earlier item resent as input to continue a provider conversation. Direction
describes model traffic only; none of these shapes creates or completes a CWP
action, wait, result, progress record, or conversation message by itself.

## Request

The request validates first against upstream `CreateResponseBody`, then against
this subset. The subset accepts only these fields; all other request fields are
rejected even though the full upstream schema knows about them.

| Field                  | Direction             | Upstream required | Supported value                                                     |
| ---------------------- | --------------------- | ----------------- | ------------------------------------------------------------------- |
| `model`                | `adapter_to_provider` | no                | string or `null`                                                    |
| `input`                | `adapter_to_provider` | no                | string, supported item array, or `null`; a string is a user message |
| `previous_response_id` | `adapter_to_provider` | no                | opaque provider response ID string or `null`                        |

`CreateResponseBody` has no required fields in the pinned schema. The profile
does not invent defaults or make an optional upstream field required.

### Text input and replay items

Only the following `ItemParam` components are supported. Content arrays are
text-only even where the full upstream component also permits media or refusal
content.

| Component                     | Direction                       | Required fields                                           | Profile constraint                                                                                                           |
| ----------------------------- | ------------------------------- | --------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------- |
| `UserMessageItemParam`        | `adapter_to_provider`, `replay` | `type`, `role`, `content`                                 | `type=message`, `role=user`; content is a string or `InputTextContentParam[]`                                                |
| `SystemMessageItemParam`      | `adapter_to_provider`, `replay` | `type`, `role`, `content`                                 | `type=message`, `role=system`; content is a string or `InputTextContentParam[]`                                              |
| `DeveloperMessageItemParam`   | `adapter_to_provider`, `replay` | `type`, `role`, `content`                                 | `type=message`, `role=developer`; content is a string or `InputTextContentParam[]`                                           |
| `AssistantMessageItemParam`   | `replay`                        | upstream: `type`, `role`, `content`; profile: `phase` too | `type=message`, `role=assistant`; content is a string or `OutputTextContentParam[]`; phase is `commentary` or `final_answer` |
| `FunctionCallItemParam`       | `replay`                        | `call_id`, `type`, `name`, `arguments`                    | `type=function_call`                                                                                                         |
| `FunctionCallOutputItemParam` | `adapter_to_provider`, `replay` | `call_id`, `type`, `output`                               | `type=function_call_output`; output is a string or `InputTextContentParam[]`                                                 |

`InputTextContentParam` requires `type` and `text`, with `type=input_text`.
`OutputTextContentParam` requires `type` and `text`, with `type=output_text`.
Assistant history must retain the original item ID and phase when they are
available; a profile-conforming assistant replay item includes a phase so
commentary cannot silently become a final answer or vice versa.

## Output items

Normal model output contains only these `ItemField` components:

| Component      | Direction                       | Required fields                                                           | Profile constraint                                                                                                   |
| -------------- | ------------------------------- | ------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------- |
| `Message`      | `provider_to_adapter`, `replay` | upstream: `type`, `id`, `status`, `role`, `content`; profile: `phase` too | `type=message`, `role=assistant`; content contains only `OutputTextContent`; phase is `commentary` or `final_answer` |
| `FunctionCall` | `provider_to_adapter`, `replay` | `type`, `id`, `call_id`, `name`, `arguments`, `status`                    | `type=function_call`                                                                                                 |

`OutputTextContent` requires `type`, `text`, and `annotations`, with
`type=output_text`. Only the exact `phase=commentary` value authorizes an
adapter to classify text as commentary. A missing phase can be retained as
sanitized diagnostic data, but it is not a profile-conforming output message and
must never become commentary.

`FunctionCallOutputItemParam` is input/replay data, not model-generated output.
The first profile does not include the corresponding upstream
`FunctionCallOutput` response component. Even an explicit provider echo remains
outside this profile's provider-to-adapter output union and may only be retained
as bounded, privacy-filtered diagnostic data.

## Events

Exactly seven provider-to-adapter streaming events are supported:

| Event type                    | Upstream component                       | Required fields                                                                | Profile payload                                                               |
| ----------------------------- | ---------------------------------------- | ------------------------------------------------------------------------------ | ----------------------------------------------------------------------------- |
| `response.output_item.added`  | `ResponseOutputItemAddedStreamingEvent`  | `type`, `sequence_number`, `output_index`, `item`                              | non-null supported output item (`Message` or `FunctionCall`)                  |
| `response.output_item.done`   | `ResponseOutputItemDoneStreamingEvent`   | `type`, `sequence_number`, `output_index`, `item`                              | the completed item opened at the same output index                            |
| `response.content_part.added` | `ResponseContentPartAddedStreamingEvent` | `type`, `sequence_number`, `item_id`, `output_index`, `content_index`, `part`  | `OutputTextContent` only                                                      |
| `response.content_part.done`  | `ResponseContentPartDoneStreamingEvent`  | `type`, `sequence_number`, `item_id`, `output_index`, `content_index`, `part`  | the completed `OutputTextContent` opened at the same content index            |
| `response.output_text.delta`  | `ResponseOutputTextDeltaStreamingEvent`  | `type`, `sequence_number`, `item_id`, `output_index`, `content_index`, `delta` | text appended to an open output-text part                                     |
| `response.output_text.done`   | `ResponseOutputTextDoneStreamingEvent`   | `type`, `sequence_number`, `item_id`, `output_index`, `content_index`, `text`  | complete text for the open output-text part                                   |
| `error`                       | `ErrorStreamingEvent`                    | `type`, `sequence_number`, `error`                                             | `ErrorPayload`; nested required fields are `type`, `code`, `message`, `param` |

Optional fields remain governed by their upstream schemas. For example,
`response.output_text.delta` may include `logprobs` and `obfuscation`, and
`response.output_text.done` may include `logprobs`; this profile does not make
those optional values user-facing.

## Ordering and correlation

Events for one provider response obey these rules:

1. `sequence_number` strictly increases.
2. `response.output_item.added` opens one `output_index` and non-null `item.id`.
   The index and ID cannot be reused for another item.
3. A message's `response.content_part.added` follows its output-item start and
   opens one `(item_id, output_index, content_index)` tuple. Function calls do
   not use content-part or output-text events in this profile.
4. Zero or more `response.output_text.delta` events follow the matching
   content-part start. They correlate through `item_id` and indexes, not through
   a phase copied onto each delta.
5. One `response.output_text.done` closes the text stream. Its `text` is the
   authoritative complete value and may repair or replace text accumulated from
   the added part and deltas.
6. `response.content_part.done` follows text done and closes that content index.
   Its text agrees with the authoritative output-text done value.
7. `response.output_item.done` follows all content completion for a message and
   closes the output index. Its item keeps the same ID, type, and assistant
   phase as the added item, and its completed content agrees with output-text
   done. A function call has no argument-delta lifecycle in this profile; its
   complete `arguments` string is carried by its item events.
8. `error` may arrive while an item or part is open and terminates the supported
   event stream. No supported event follows it.

An adapter tracks assistant phase on the `Message.id` established by the item
event. Interleaved item streams therefore remain distinguishable. Missing or
unknown phase never defaults to commentary.

## Validation

Validation has two mandatory layers:

1. **Pinned-schema validation.** Validate the whole value against the referenced
   component in `openapi.json`. This enforces standard field types, required
   arrays, enum values, lengths, and nested schemas.
2. **Profile validation.** Reject fields, union members, content kinds, event
   types, directions, and sequences not listed in `profile.json`. Enforce the
   text-only constraints, assistant phase and identity correlation, and the
   ordering above.

Passing the upstream schema alone does not establish CWP compatibility. A
profile rejection cannot be relaxed by treating an unknown shape as model text.
Provider-prefixed or otherwise unknown values may only enter bounded,
privacy-filtered diagnostic handling.

## Continuation identities

`previous_response_id` is an opaque provider response identity. It is not a CWP
conversation ref, action ref, invocation ref, model-run ref, item ID, or
function `call_id`, and none of those values may be substituted for it. The host
may retain a provider continuation privately and send it only back to the same
provider integration for the intended conversation. The profile does not expose
the private store or manufacture a response ID from a CWP action result.

Within one provider response, `Message.id` and `FunctionCall.id` identify output
items; `item_id` plus `output_index` and `content_index` correlate streaming
events; and `call_id` pairs a function call with its later result. These
identities remain distinct. Replaying an item preserves its provider item ID and
assistant phase when present, while a new CWP action or model run keeps its own
CWR identity.

## Explicit exclusions

The subset excludes:

- `response.completed` and every response-resource lifecycle event, including
  created, queued, in-progress, incomplete, failed, and completed events;
- `ResponseResource` construction or inference from provider or CWP lifecycle;
- function-call argument delta and done events;
- `FunctionCallOutput` as provider-to-adapter output, including explicit echoes;
- reasoning, reasoning-summary, refusal, annotation-stream, and compaction items
  or events;
- images, audio, video, files, or other media content;
- item references, tool definitions and tool-choice configuration, sampling and
  text-format controls, metadata, usage, and other unlisted request fields;
- WebSocket control events, HTTP/SSE server behavior, reconnection, or recovery;
  and
- provider-prefixed extensions and all other unlisted standard or provider
  shapes.

Codex `turn/completed` or another provider terminal signal may close an
adapter's private model run. It must not be synthesized as `response.completed`,
and CWP action acceptance or completion must not synthesize any Open Responses
event.
