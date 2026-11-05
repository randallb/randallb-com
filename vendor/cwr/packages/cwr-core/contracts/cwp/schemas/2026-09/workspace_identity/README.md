# Local workspace identity

`.cwr/workspace.toml` stores a machine-local workspace identity using this schema.
It is separate from the authored `WORKSPACE.toml` composition.

```toml
type = "cwp://org.coworkerprotocol/2026-09/schemas/workspace_identity"
workspace_id = "03bbe874-d68b-4f85-bf4f-b4ea22188ea9"
runtime_root_ref = "cwp://local.my-mac/runtimes/03bbe874-d68b-4f85-bf4f-b4ea22188ea9"
```

Legacy JSON versions 1 and 2 remain readable and migrate without changing runtime
identity or state locations. Omit `runtime_root_ref` for a migrated version 1 identity.
