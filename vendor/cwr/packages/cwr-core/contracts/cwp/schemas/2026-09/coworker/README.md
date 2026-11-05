# Coworker

Schema ref: `cwp://org.coworkerprotocol/2026-09/schemas/coworker`

[Schema](schema.json) · [Resource types](../../README.md)

A coworker is one concrete identity that can be imported, discovered, and
addressed. It extends the
[common declaration base](../../../../../../configuration/docs/cwr-toml.md). The resolver assigns its
ref from its resolved source file; aliases and inputs do not create identities.

```toml
type = "cwp://org.coworkerprotocol/2026-09/schemas/coworker"
name = "BFF"
slug = "bff"
description = "Helps carry out repository work."
authority = ["workspace.read", "workspace.write", "process.execute"]

[[roles]]
path = "./ROLE.toml"
as = "helper"

[[imports]]
path = "../deveng_ops/COWORKER.toml"
as = "deveng_ops"

[[integrations]]
path = "../../integrations/local_codex.toml"
as = "local_codex"
default = true
```

`name` and `description` explain who the coworker is and when to ask it. `slug`
supplies its CVO handle. Neither substitutes for the canonical identity.
`authority` grants workspace effects; role requirements do not grant them.
People can have no roles or executable bindings and remain discoverable through
host-native authorship and delivery.

`roles` loads and applies reusable [roles](../role/README.md) in list order.
Reject overlapping VO action names and conflicting shared execution parameters.
Each executable integration must support every selected role. Keep owning roles
and their artifact paths with actions and instructions throughout dispatch.

Each `integrations` entry has exactly one path or ref and a stable `as` binding
name. It loads its [integration](../integration/README.md) and can supply
`inputs`, `params`, `message_mode`, and `working_path`. One execution binding is
an implicit default. Multiple bindings require exactly one `default = true`. An
unavailable default never falls back. Explicit discovered targets select another
exact binding, and each binding retains independent private history.

Set `message_mode = "actions_only"` for software that accepts structured
operations rather than conversational requests. Ordinary messages then explain
its available actions without starting the adapter. The workspace selects the
execution directory independently of the declaration directory.

Imports name expected collaborators. Every loaded coworker is discoverable by
canonical ref within its runtime, even when not directly imported or
re-exported. An accepted request establishes participation; loading does not
start work.

An import may set `default_conversation_target = true` to preselect a coworker
for new conversations started by this coworker:

```toml
[[imports]]
path = "../workloop/COWORKER.toml"
as = "workloop"
default_conversation_target = true
```

The import uses exactly one `path` or `ref`, like any other import. At most one
import may enable this flag, and it must resolve to another coworker. Ordinary
imports, including those with the flag set to false, can load any resource type.
`cwr check` and graph loading reject invalid defaults. CWR exposes the resolved
canonical coworker ref in the selected coworker's host catalog.

Hosts use this preference only when starting a conversation. Existing targets
stay in place, and an explicit target or mention takes precedence. Without a
declared default, the person chooses a target. Handles such as `@workloop` and
`@bff` carry no special preference. Setting a default does not establish
participation or start work.
