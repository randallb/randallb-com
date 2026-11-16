# Built-in execution adapters and gateway contracts

See [Gateways and execution adapters](../../../../../../memos/policy/cwr/gateways-and-adapters.md) for the
responsibility model. Slack's resource ref and schemas are retained for
compatibility with existing workspace declarations. CWR does not currently ship
an implementation for that contract.

Direct adapter network access, including model-provider access, is a temporary
pilot/alpha exception to the
[CWP-only execution communication boundary](../../../../../../memos/policy/cwr/gateways-and-adapters.md#execution-communication-boundary-and-pilot-exception).
The existing provider setup below describes current behavior, not the target
gateway-mediated architecture.

The `2026-09` Coworker Protocol release makes these execution and gateway
contracts available to the host resolver. A role adapter selection or
integration adapter locator loads the chosen contract directly; unused contracts
are not added to every resource namespace:

| Contract       | Responsibility    | Resource                                     | Parameter schemas                                                                                                                                                                                      |
| -------------- | ----------------- | -------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Codex          | Execution adapter | [codex.toml](codex.toml)                   | [coworker](codex.coworker-params.schema.json) · [integration](codex.integration-params.schema.json) · [actor integration](codex.actor-integration-params.schema.json)                            |
| Claude         | Execution adapter | [claude.toml](claude.toml)                 | [coworker](claude.coworker-params.schema.json) · [integration](claude.integration-params.schema.json) · [actor integration](claude.actor-integration-params.schema.json)                         |
| Open Responses | Execution adapter | [open-responses.toml](open-responses.toml) | [coworker](open-responses.coworker-params.schema.json) · [integration](open-responses.integration-params.schema.json) · [actor integration](open-responses.actor-integration-params.schema.json) |
| Loopback       | Execution adapter | [loopback.toml](loopback.toml)             | [coworker](loopback.coworker-params.schema.json) · [integration](loopback.integration-params.schema.json) · [actor integration](loopback.actor-integration-params.schema.json)                   |
| RNG            | Execution adapter | [rng.toml](rng.toml)                       | [coworker](rng.coworker-params.schema.json) · [integration](rng.integration-params.schema.json) · [actor integration](rng.actor-integration-params.schema.json)                                  |
| Shell          | Execution adapter | [sh.toml](sh.toml)                         | [coworker](sh.coworker-params.schema.json) · [integration](sh.integration-params.schema.json) · [actor integration](sh.actor-integration-params.schema.json)                                     |
| Slack          | Gateway           | [slack/adapter.toml](slack/adapter.toml)   | [coworker](slack/coworker-params.schema.json) · [integration](slack/integration-params.schema.json) · [actor integration](slack/actor-integration-params.schema.json)                            |

The TOML files are ordinary resources validated by the core
[adapter schema](../../../cwp/schemas/2026-09/adapter/schema.json). Their
parameter schemas are bundled JSON Schema documents located by path; they have
no separate CWR resource identity.

Each declaration also lists the workspace authority its contract can realize.
Codex, Claude, and Shell provide all three `2026-09` workspace effects. Open
Responses, Loopback, and RNG currently provide none. Slack remains loadable only
for legacy workspace compatibility and has no installed host. These contracts
cannot satisfy a coworker that requires repository access until they supply and
enforce those effects.

These declarations do not contain or launch executables. A trusted host maps an
approved built-in adapter ref to its installed implementation and private launch
configuration.

The local host supplies stable private adapter storage independently of
workspace persistence. The Codex implementation initializes its provider home
through an out-of-band configuration entry point; setup and new invocations
consume that same home. See
[adapter-owned storage](../../../../../../memos/runbooks/adapter-storage.md) for
configuration, import behavior, and retained-session compatibility.

Process implementations use the [process adapter protocol](process-adapter.md)
for invocation control, streamed conversation context, live host calls, and
results.

The local Shell adapter defaults to `/bin/bash -c` noninteractively, without
login startup, in the working directory and environment supplied by the host. An
integration can set `params.runner` to an executable and fixed arguments; the
adapter appends each command as one argument. Coworkers using that integration
share its runner. See
[Run a Shell coworker](../../../../../../memos/runbooks/sh-adapter.md) for shared
workspace setup and individual command examples.

A Codex coworker may set `isolated = true` when it only transforms supplied
input into a validated result. The adapter then starts Codex with read-only
filesystem access, disables network access, and withholds CWR host tools. The
provider's `cwp.cwr--return--result` tool remains available.

Codex actors run with `danger-full-access` by default. The local runtime grants
only its canonical bound workspace; other trusted hosts must name that path in
`CWR_CODEX_DANGER_FULL_ACCESS_WORKSPACE`. Workspace resources cannot grant
unrestricted access to themselves or another checkout. An actor integration may
set `sandbox = "workspace-write"` to opt down. Approval prompts remain disabled.
An isolated coworker remains read-only regardless of this actor setting.

See [Run a Claude coworker](../../../../../../memos/runbooks/claude-adapter.md)
for the native CLI setup, credentials, and current tool limits. The local
runtime grants Claude native access only for its canonical bound workspace
through `CWR_CLAUDE_DANGER_FULL_ACCESS_WORKSPACE`; other trusted hosts must
explicitly name that path. This selects a trusted host workspace and is not
filesystem or process containment.

See
[Run an Open Responses coworker](../../../../../../memos/runbooks/open-responses-adapter.md)
for generic HTTP endpoint setup and the OpenRouter example.
