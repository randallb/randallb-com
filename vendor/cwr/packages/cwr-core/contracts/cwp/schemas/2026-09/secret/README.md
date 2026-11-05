# Secret input

Schema ref: `cwp://org.coworkerprotocol/2026-09/schemas/secret`

[Value schema](schema.json) · [Input declarations](../cwr/README.md)

Sensitive text. Inspection reports declared availability without resolving the
value. Forwarding must remain secret at each typed receiver; opaque legacy
schemas cannot receive a typed secret.

Declare the full type with a local `as`, optional `purpose`, and `required`
(default `true`). The existing JSON Schema validator checks concrete values.
