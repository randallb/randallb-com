# Plaintext input

Schema ref: `cwp://org.coworkerprotocol/2026-09/schemas/plaintext`

[Value schema](schema.json) · [Input declarations](../cwr/README.md)

Ordinary JSON configuration. Values may be strings, numbers, booleans, arrays,
objects, or null. A path string does not grant folder access.

Declare the full type with a local `as`, optional `purpose`, and `required`
(default `true`). The existing JSON Schema validator checks concrete values.

Workloop local setup accepts `storage = "local_json"` (the default) or
`storage = "workspace_variable"` on a plaintext input declaration. Local JSON
retains ordinary configuration semantics. A workspace variable is a text value
selected through Variables & secrets; it keeps typed provenance and approved
destinations, and its value is excluded from retained execution records.
`storage` is invalid on other input types. Secret inputs use workspace secrets
or an existing explicitly assigned legacy key.

See
[workspace environment](../../../../../../../apps/workloop-desktop/docs/workspace-environment.md)
for supported destinations and replacement/removal behavior.
