# Private working-history selection

The continuation-selection implementation is in [working.rs](./working.rs).

## Selected-volume policy drift

Private continuation is intended to be scoped by selected volume as well as
configured runtime, coworker, binding, and conversation. The owning policy is
`memos/policy/cwr/cwr-conversation-participation.md`, under “Shared and
private conversations.”

The `predecessors` lookup does not compare the selected volume, and
`AdapterRoute::provider_fingerprint` does not include it. Configured-runtime
identity is independent of work volumes, so changing the primary volume while
keeping the runtime, conversation, binding, and route otherwise compatible can
reuse earlier private history. Enforce volume isolation in continuation
selection and provider compatibility before claiming that guarantee is met.
