# CWR graph declaration

Schema ref: `cwp://org.coworkerprotocol/2026-09/schemas`

[Schema](schema.json) · [Resource types](../../README.md)

`CWR.toml` declares a generic resource graph and its input contract. A
[reusable runtime](../runtime/README.md) has a distinct schema; a
[workspace](../workspace/README.md) selects definitions to configure and run. It
shares `type`, `imports`, `exports`, and typed `inputs` (or legacy
`input_schema`) with specialized coworker and role declarations. It has no
authored `ref` or root actor list.

```toml
type = "cwp://org.coworkerprotocol/2026-09/schemas"

[[imports]]
path = "./coworkers/bff/COWORKER.toml"
as = "bff"
```

Importing a coworker loads its roles, collaborators, adapters, and integration
bindings transitively. Use exactly one path or ref per locator, optional `as`,
and explicit `inputs`. Exports present existing resources through local names;
they neither create identities nor restrict canonical discovery inside a
runtime.

The [base specification](../../../../../../configuration/docs/cwr-toml.md) defines source roots,
resource identity, input validation, secret forwarding, activation, and
workspace/volume separation.
[Workspace migration](../../../../../../../memos/runbooks/migrate-cwr-coworkers-and-roles.md)
provides a worked conversion and operational commands.

Typed input declarations use `type`, `as`, `purpose`, and `required`. Dependency
mappings can use `[[imports.inputs]]` with `ref = "~@/source_alias"` and
`as = "receiving_alias"`. The receiving declaration must use the same input
type. `inspect_requirements` follows these contracts through the existing graph
resolver without supplying values. It reports legacy schemas as opaque and never
infers schema compatibility.

## Standalone gateways

Import a [gateway definition](../gateway/README.md) as a resource. It selects a
role action and configured integration without adding the gateway handler to the
conversational actor catalog.
