# Action

Schema ref: `cwp://org.coworkerprotocol/2026-09/schemas/action`

[Schema](schema.json) · [Example](example.toml) ·
[All CWR resource types](../../README.md)

An action is an operation owned by one role:

```toml
[[actions]]
verb = "draft"
object = "brief"
title = "Draft brief"
description = "Draft a concise brief."

[actions.input_schema]
path = "./schemas/draft_brief_input.schema.json"

[actions.output_schema]
ref = "cwp://com.example/schemas/2026-09/draft_brief_output"
```

The [`action` schema](schema.json) defines the common value shape. An action
does not have its own `type` or `ref`. Its canonical action name is
`<verb>--<object>`, so this declaration's name is `draft--brief`. Its identity
is the owning role ref plus that canonical action name. The `(verb, object)`
pair must be unique across all roles selected by one coworker.

`verb` and `object` are lowercase snake-case routing fields. `title` is a short
human-facing label. `description` explains what the action does and supplies the
semantic text used when the action is presented to a model. Neither `title` nor
`description` affects identity or routing.

`input_schema` and `output_schema` each contain exactly one locator:

- `path` loads a bundled JSON Schema file relative to the role declaration that
  owns the action.
- `ref` resolves a JSON Schema document already available through CWR.

A schema path must start with `./`, remain below the role declaration's
directory, and resolve to one regular non-symlink file. Absolute paths, `..`,
directories, globs, and escaping symlinks are invalid. Moving or mounting the
coworker keeps its bundled schema paths relative to the role's source directory,
not the process working directory or importing `CWR.toml`.

Both forms resolve to a JSON Schema that validates invocation input or completed
output. A referenced schema document is not required to be a typed CWR resource;
the locator field establishes the expected target kind. Hosts retain the exact
resolved schema or its checkpoint when validation must remain reproducible.

## Execution and response

Actions declare two independent behaviors:

| Field             | Values                                        | Default                   |
| ----------------- | --------------------------------------------- | ------------------------- |
| `execution_scope` | `requesting_conversation`, `own_conversation` | `requesting_conversation` |
| `response_mode`   | `sync`, `async`                               | `sync`                    |

`requesting_conversation` performs collaboration in the conversation where the
request was made. `own_conversation` performs it in the invocation's execution
conversation, with its own participants. Discovery, messages, accepted requests,
and nested action calls use that selected conversation. The request and final
response remain in the requesting conversation.

`sync` returns the completed action response. `async` returns an admitted
invocation reference without waiting for completion. The eventual result is
recorded in the requesting conversation for later work; it does not start or
resume the caller. The output schema always describes the eventual successful
result, including for asynchronous actions; it does not describe the admission
receipt. CWR's invocation tool follows the action declaration, rather than
asking the caller to select timing on each call.

For example, Sourceng can submit work using:

```toml
execution_scope = "own_conversation"
response_mode = "async"
```

The runtime retains both choices with the invocation's admitted execution
snapshot. Reloading the catalog does not change an existing invocation's scope
or response behavior. Neither setting grants authority or confidentiality.
Stopping a parent always stops its nonterminal descendants. Normal completion
does not stop asynchronous children.

Conversational requests through supported native adapters continue private
working history by default. Named actions start independent history. This is
separate from `execution_scope` and `response_mode`: history reuse changes
neither collaboration membership nor result routing or response timing. See the
[process adapter contract](../../../../cwr/adapters/2026-09/process-adapter.md#context-delivery)
for supported adapters and restoration requirements.

## Action contract identity

The activated contract fingerprints the resolved input and output schemas,
`execution_scope`, `response_mode`, and explicit `contract_revision`. A schema
edit behind the same locator changes the fingerprint after activation. Breaking
behavior changes without another declared difference require a revision change.
Compare this identity before executing discovered or retained calls. A changed
contract rejects without running or replacing old definitions, mappings, calls,
results, or provider history. Fresh discovery returns the current exact target.
