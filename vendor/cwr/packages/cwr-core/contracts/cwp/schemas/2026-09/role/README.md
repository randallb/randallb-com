# Role

Schema ref: `cwp://org.coworkerprotocol/2026-09/schemas/role`

[Schema](schema.json) · [Resource types](../../README.md)

A role supplies reusable instructions, action interfaces, supported adapters,
and dependencies. It extends the
[common declaration base](../../../../../../configuration/docs/cwr-toml.md) and has no authored
identity field. Loading a role does not create a coworker; a concrete coworker's
`roles` list selects it.

```toml
type = "cwp://org.coworkerprotocol/2026-09/schemas/role"
name = "Repository helper"
required_authority = ["workspace.read"]

[[adapters]]
ref = "cwp://org.coworkerprotocol/2026-09/adapters/codex"
as = "codex"

[adapters.params]
entrypoint = "./PROMPT.md"
```

Each adapter entry uses exactly one path or ref, optional `as`, input bindings,
and portable `params`. Selecting an adapter loads its contract and parameter
schemas directly, including a known bundled adapter. Unknown refs fail without
installation. Loading a contract does not supply an executable implementation.

`actions` follows the [action schema](../action/README.md). A role owns each
`verb--object` action. A coworker composes one catalog from its selected roles;
any duplicate VO name rejects graph loading, even with identical schemas.

Compose instructions in role order and retain role-local paths. Every executable
binding must support all roles. Equal shared settings combine, omitted settings
impose no value, and different explicit values reject. Union
`required_authority` and check actual grants and adapter capabilities.

Actions carry an explicit contract revision when behavior changes incompatibly.
The action contract identity also fingerprints activated resolved schemas,
`execution_scope`, and `response_mode`. A changed contract rejects retained
calls before execution without replacing their definitions or history.
