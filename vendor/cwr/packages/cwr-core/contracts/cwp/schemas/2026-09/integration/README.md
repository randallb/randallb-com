# Integration

Schema ref: `cwp://org.coworkerprotocol/2026-09/schemas/integration`

[Schema](schema.json) · [Resource types](../../README.md)

An integration configures one adapter installation, account, or execution
environment. Coworker bindings and standalone gateway definitions can refer to
the same integration resource.

```toml
type = "cwp://org.coworkerprotocol/2026-09/schemas/integration"

[adapter]
ref = "cwp://org.coworkerprotocol/2026-09/adapters/codex"
```

`adapter` uses exactly one path or ref and loads the adapter contract directly.
The integration may provide typed `inputs` or a legacy `input_schema`, explicit
dependency inputs, and adapter-defined `params`. Validate effective inputs and
parameters before activation. Identical effective inputs can share the
integration inside one runtime; conflicting inputs reject independently of load
order.

A coworker owns its named `[[integrations]]` bindings, each selecting this
resource and supplying that coworker's route parameters. A standalone gateway
selects an integration directly in its [gateway definition](../gateway/README.md).
Every executable binding must support every selected role. See the [coworker
contract](../coworker/README.md) for default selection and exact binding
identity.

Keep credentials behind host-managed secret handling. Declare secret inputs with
`type = "cwp://org.coworkerprotocol/2026-09/schemas/secret"` and forward them
only to typed secret receivers. A legacy `input_schema` is opaque during
inspection; its annotations do not authorize typed secrets to cross that
boundary. Omit secrets from automatic model context, public records, and
diagnostics. Input validation is not itself secret handling.
