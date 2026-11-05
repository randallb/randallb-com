# Runtime definition

A reusable resource graph and input contract for a CWR runtime instance.
Workloop loop membership, management, and licensing belong to the host. See
[CWR configuration](../../../../../../configuration/docs/cwr-toml.md).

## Standalone gateways

Import a [gateway definition](../gateway/README.md) as a resource in the runtime
graph. It binds a role action to a configured integration without adding a
coworker to the conversational actor catalog. `cwr--send--request` resolves only
these standalone gateway routes.
