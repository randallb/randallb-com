# Configured runtime identity

`runtime-identity.toml` stores the immutable CWP root selected for one
configured runtime in host-private platform state. It is separate from the
authored `WORKSPACE.toml` composition.

```toml
type = "cwp://org.coworkerprotocol/2026-09/schemas/configured_runtime_identity"
runtime_id = "c234503497e5b9bc9b1e6a6279bdba95ea9dbaf12c0b3eb28f2b0f84b55af010"
runtime_root_ref = "cwp://local.my-mac/runtimes/c234503497e5b9bc9b1e6a6279bdba95ea9dbaf12c0b3eb28f2b0f84b55af010"
```

Fresh configured runtimes use a machine-scoped `/runtimes/` root. A runtime
database created before this record existed keeps its exact
`cwp://org.coworkerprotocol.local/loops/...` root. That compatibility value is
never minted for fresh state.
