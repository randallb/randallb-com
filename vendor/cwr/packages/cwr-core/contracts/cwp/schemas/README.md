+++
ref = "~@/policies/cwr_resource_types"
+++

# CWR Resource Types

These are versioned CWP schema contracts used by CWR. Shared meanings belong to
[CWP references](../../../../../memos/policy/cwp/cwp-references.md) and the protocol model; declaration
composition, activation, and local state handling belong to CWR. A schema
contract does not make every surrounding runtime choice universal protocol
policy.

Every first-class CWR entity declares the schema that defines it. The base
schema for every `CWR.toml` uses the release ref itself. Concrete resource and
value schemas live below it:

```text
cwp://org.coworkerprotocol/<release>/schemas
cwp://org.coworkerprotocol/<release>/schemas/<schema_name>
```

The `type` field contains that schema ref. It is not a short enum and a runtime
must not infer it from a URI path, filename, TOML table name, or implementation
class.

Coworker Protocol names a schema release by its publication month in `YYYY-MM`
form. A release is one coherent set of schemas, such as:

```text
cwp://org.coworkerprotocol/2026-09/schemas
cwp://org.coworkerprotocol/2026-09/schemas/coworker
```

The release ref validates `CWR.toml`, which declares an entry point. Named refs
validate other resource declarations or contained values.

A release can change while it is a draft. Once published, its schema identities
and validation contracts are immutable. Coworker Protocol publishes another
calendar release when a contract changes; releases do not need to occur every
month.

A ref can still address a contained file or a named member without turning that
target into a separately declared entity. For example, a file below the current
mount can be opened through `~@/README.md`, and a policy rule can be selected
with a fragment. The declaration requirement applies when a target has its own
independent CWR identity, authority, or lifecycle outside the containing
resource—not merely because a contained value has state or participates in a
relationship.

This policy defines the typed-resource contract for the `2026-09` CWR format.
Every accepted declaration uses the explicit schema type defined by this
release.

## Declaration envelope

Every declaration supplies `type`. Ordinary CWR, runtime, coworker, role, and
integration files receive identity from their resolved source; they do not
author a `ref`. Schema and adapter contracts retain their protocol-owned
identities. Runtime records retain their assigned refs and historical contracts.
Values nested in a resource do not acquire independent identity merely because
they use a schema.

## Current declaration contracts

| Schema suffix    | Resource                                                                                                                           |
| ---------------- | ---------------------------------------------------------------------------------------------------------------------------------- |
| Base release ref | [CWR.toml](2026-09/cwr/README.md): a generic resource graph and configuration inputs.                                            |
| `/runtime`       | [Runtime definition](2026-09/runtime/README.md): a resource graph with a configuration contract.                                 |
| `/workspace`     | [Workspace composition](2026-09/workspace/README.md): selected runtime definitions, inputs, paths, and work volumes.             |
| `/coworker`      | [Coworker](2026-09/coworker/README.md): one concrete, addressable identity.                                                      |
| `/role`          | [Role](2026-09/role/README.md): reusable instructions, VO actions, adapters, and dependencies.                                   |
| `/integration`   | [Integration](2026-09/integration/README.md): an adapter installation, account, or environment.                                  |
| `/gateway`       | [Gateway](2026-09/gateway/README.md): a standalone request route bound to a role action and integration.                         |
| `/adapter`       | [Adapter](2026-09/adapter/README.md): an execution contract; adapter-named transport contracts retain compatibility identifiers. |

Coworkers load selected roles, expected collaborators, and named integration
bindings themselves. Standalone gateways load a role and integration without
becoming conversational actors. Standalone actor and actor-integration authoring
contracts are retired. People can participate through host-native identity and
delivery without an executable binding. Requirements grant no authority.

Imports, exports, roles, adapters, and integration bindings share path/ref/as
locators and explicit input bindings. The resolver loads selected adapter
contracts, including known bundled contracts, rather than injecting every
adapter into every resource namespace. Unknown refs do not trigger downloads.

## Configuration value types

| Schema suffix | Contract                                                                         |
| ------------- | -------------------------------------------------------------------------------- |
| `/plaintext`  | [Plaintext](2026-09/plaintext/README.md): ordinary JSON configuration.         |
| `/secret`     | [Secret](2026-09/secret/README.md): sensitive text with value-free inspection. |

Volume requirements are separate composition resources with logical names and
requested access, not ordinary input values.

## Records and value contracts

| Schema suffix                       | Contract                                                                                                                      |
| ----------------------------------- | ----------------------------------------------------------------------------------------------------------------------------- |
| `/action`                           | [Action](2026-09/action/README.md): one role-owned VO action interface.                                                     |
| `/action_invocation`                | [Invocation](2026-09/action_invocation/README.md): an admitted request and its exact route.                                 |
| `/action_response`                  | [Response](2026-09/action_response/README.md): ordered progress and one terminal result.                                    |
| `/conversation`                     | [Conversation](2026-09/conversation/README.md): ordered shared collaboration.                                               |
| `/conversation_message`             | [Message](2026-09/conversation_message/README.md): attributed shared content and addressing.                                |
| `/conversation_live_status`         | [Live status](2026-09/conversation_live_status/README.md): leased current activity.                                         |
| `/conversation_timeline_connection` | [Timeline connection](2026-09/conversation_timeline_connection/README.md): one ordered, pageable set of sibling placements. |
| `/conversation_timeline_item`       | [Timeline item](2026-09/conversation_timeline_item/README.md): one durable publication placement.                           |
| `/conversation_timeline_content`    | [Timeline content link](2026-09/conversation_timeline_content/README.md): a typed link from placement to canonical content. |
| `/conversation_timeline_cursor`     | [Timeline cursor](2026-09/conversation_timeline_cursor/README.md): stable connection-local pagination identity.             |

Declarations use the shared [base specification](../../../../configuration/docs/cwr-toml.md). Exact
targets and historical tools follow
[tool activation](../../../../../memos/policy/cwr/cwr-subjects-actors-and-tool-activation.md). The
[migration guide](../../../../../memos/runbooks/migrate-cwr-coworkers-and-roles.md)
explains current fields and preserving existing records. Retired manifests are
not automatically converted or treated as aliases for the new contracts.

The historical `/loop` schema remains accepted as a compatibility spelling for a
reusable runtime definition. Workloop owns product loop semantics.

Local workspace identity is persisted in `.cwr/workspace.toml` using the
[workspace identity schema](2026-09/workspace_identity/README.md).
Host-private composition state persists each selected runtime root using the
[configured runtime identity schema](2026-09/configured_runtime_identity/README.md).
