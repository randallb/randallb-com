# Adapter

Schema ref: `cwp://org.coworkerprotocol/2026-09/schemas/adapter`

[Schema](schema.json) · [Example](example.toml) ·
[All CWR resource types](../../README.md)

An adapter describes a reusable communication or execution contract whose
implementation is resolved by the host:

```toml
type = "cwp://org.coworkerprotocol/2026-09/schemas/adapter"
ref = "cwp://org.coworkerprotocol/2026-09/adapters/loopback"
name = "Loopback"
authority_capabilities = []
```

The adapter ref identifies the contract and acts as the key through which a host
can find an approved implementation. The resource does not contain a command,
executable path, download URL, or package installer. Merely importing or
validating it must never execute code or start a download.

`authority_capabilities` declares which effects from the
[actor-authority policy](../../../../../../../memos/policy/cwr/actor-authority.md) a conforming
adapter implementation can provide and enforce. It does not grant those effects.
The active route receives only the actor's authority, and a route is limited
when the adapter cannot realize all of the coworker's requirements.

## Parameter schemas

An adapter can locate a separate JSON Schema for each configuration layer:

| Field                             | Validates                                         |
| --------------------------------- | ------------------------------------------------- |
| `coworker_params_schema`          | One matching `role.adapters[].params` object.     |
| `integration_params_schema`       | The selected integration's `params` object.       |
| `actor_integration_params_schema` | The selected actor integration's `params` object. |

Each field contains exactly one of `path` or `ref`:

- `path` loads a bundled JSON Schema file relative to the adapter declaration.
- `ref` resolves a JSON Schema document already available through CWR.

A path must start with `./`, remain below the adapter declaration's directory,
and resolve to one regular non-symlink file. A referenced schema is not required
to be a typed CWR resource; the locator field establishes the expected target
kind. A ref with a fragment may select a subschema.

These locators do not identify the adapter implementation. A host resolves the
selected adapter and applies each declared schema to its corresponding parameter
object. Omitted parameter objects are interpreted as empty objects; JSON Schema
`default` annotations do not populate them. Hosts retain the resolved schema or
its checkpoint when reproducible validation matters.

The layers remain separate:

| Location                   | Contains                                                                                    |
| -------------------------- | ------------------------------------------------------------------------------------------- |
| `role.adapters[].params`   | Portable role configuration such as a prompt, flake, or software entrypoint ref.            |
| `integration.params`       | Installation, execution-environment, or provider-account configuration.                     |
| `actor_integration.params` | Coworker binding identity and defaults such as a Slack user ID, model, or reasoning effort. |

An adapter defines any inheritance or override behavior between layers. A host
must not rely on an implicit TOML merge.

## Runtime boundary

A coworker binding says the coworker knows how to use the adapter. An
integration configures the adapter for an installation or execution environment.
An actor integration selects that configured path for one actor. Adapter array
order does not select a default.

The host maps an approved adapter ref to a resolved implementation. That
mapping, executable resolution, process isolation, credentials, and enforcement
are host concerns. The portable adapter field describes the implementation's
authority capabilities; it does not contain one root's grants, OAuth tokens,
provider account, model choice, coworker configuration, or actor identity.

A host may ask its registered implementation whether an integration is ready
without starting an invocation. Readiness reports `ready`, `setup_required`, or
`temporarily_unavailable` together with a stable code and user-facing message.
Hosts should show that result during coworker discovery and check it again just
before dispatch. The check belongs to the trusted implementation; portable
adapter resources cannot supply an executable health-check command.

For a login need, a trusted implementation may also return a structured
`repair` offer alongside `setup_required` and `provider_login_required`. The
offer names a host setup operation such as `provider_login` and its display
label. The host runs that operation through a separate setup path that remains
available while ordinary adapter dispatch is blocked, then checks readiness
again before reporting success. Portable adapter resources cannot provide an
executable login command, credentials, or an arbitrary repair handler. The
2026-09 built-in Codex implementation owns
its login check and browser repair; other providers advertise a repair offer
only when their trusted host handler is available.

The 2026-09 adapter contract does not yet enumerate supported operations or
declare action-specific bindings.

The release's
[built-in adapter catalog](../../../../cwr/adapters/2026-09/README.md) contains
the canonical Codex and loopback adapter resources and their bundled parameter
schemas.

## Loading and role composition

A role's adapter locator loads the selected contract and parameter schemas
without a duplicate import. Known bundled refs follow that rule; unused
contracts are not injected into every scope. Loading a contract neither installs
an implementation nor starts work.

Every executable binding supports all selected roles. Compose role-local
instructions in list order and retain each artifact directory. Shared execution
settings combine only when equal; conflicting explicit model or other shared
settings reject with the contributing roles named. Union authority requirements
without granting effects.

Keep provider-state compatibility separate from live CWR authorization.
Revocation of an action target can preserve settled private history while the
next call is denied. Retry initialization remains exact, including activated
prompt and role contents, request, route, result contract, and observed context.
