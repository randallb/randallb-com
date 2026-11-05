# CWR workspace composition

Schema ref: `cwp://org.coworkerprotocol/2026-09/schemas/workspace`

[Schema](schema.json) · [Example](example.toml) ·
[Resource types](../../README.md)

A workspace selects one or more runtime definitions by `runtimes.path`. Each
selection has a name, source roots, working directory, named work volume, and
optional inputs, acting coworker, additional volume access, and reconfiguration
permission. Runtime feature switches come from the shared configuration catalog,
not workspace-specific TOML fields. Canonical publication timelines and tool
capture are standard runtime behavior; supported legacy histories migrate
automatically at startup. Paths resolve relative to the workspace file. The
selected declaration uses the `/runtime` schema or a generic CWR graph. Legacy
`/loop` definitions and `[[loops]]` entries remain accepted; coworker
declarations are not runtime definitions. Invalid or unsupported workspace types
and fields fail before input resolution or execution. `version = 1` is retired.

Volumes contain work materials. The primary volume defaults to `read_write` when
no explicit attachment is supplied; `volume_access` can select `read` instead.
These grants describe host access and do not sandbox native subprocesses.

Runtime databases live under the platform state home in `CWR/loops/<key>` on
macOS and Windows, or `cwr/loops/<key>` elsewhere. The key is the existing hash
of canonical workspace-file path and configured-runtime name. `XDG_STATE_HOME`,
when absolute, overrides the platform state home; use a private local directory.
CWR creates owned private application, `loops`, and context directories on
opening. These legacy directory names preserve existing storage paths; they do
not determine new protocol identity. A fresh configured runtime uses
`cwp://local.<hostname>/runtimes/<key>` and atomically saves it in
`runtime-identity.toml` before creating its database. A database created before
that sidecar existed retains its exact
`cwp://org.coworkerprotocol.local/loops/<key>` root and is grandfathered after a
successful open. CWR leaves inspection free of state creation and locks.

Reopening the same workspace and runtime reuses its state. Other configured
runtimes have separate databases. Moves, copies, and renames may start fresh in
this pre-release format. Unused experimental volume-local databases are not
imported; in-use ordinary workspace and multiplayer history remains governed by
its existing preservation requirements.

From the repository root, after building the CLI and runtime, this example needs
no provider credentials or external service:

```sh
cwr_example=packages/cwr-core/contracts/cwp/schemas/2026-09/workspace/example.toml
target/debug/cwr workspace check "$cwr_example"
target/debug/cwr workspace coworkers "$cwr_example" example
target/debug/cwr workspace invoke "$cwr_example" example @cwr--list--coworkers '{}'
```

Open and resume that same runtime in a terminal:

```sh
target/debug/cwr-tui workspace "$cwr_example" example
target/debug/cwr-tui workspace "$cwr_example" example resume last
```

Execution uses one resident per canonical workspace path and runtime name. CLI
and TUI share its conversations and acting coworker; disconnecting either client
leaves the other's work intact. A different acting coworker or changed
executable configuration is rejected while the resident is active. The
comparison includes activated prompt and role contents. Configured residents
retain their selection until exit, without graph reload. Stop and drain controls
take the same runtime:

```sh
target/debug/cwp-runtime "$cwr_example" --runtime example --drain-resident begin
target/debug/cwp-runtime "$cwr_example" --runtime example --drain-resident status
target/debug/cwp-runtime "$cwr_example" --runtime example --stop-resident
```

Existing drain operations, including `cancel`, `pause-autostart`, and
`resume-autostart`, apply to that owner. Reopening an unchanged selection after
exit restores its existing records.

The first two commands inspect without opening state. The last executes an
intrinsic action and retains its records in host storage. See the
[conversion guide](../../../../../../../memos/runbooks/migrate-cwr-coworkers-and-roles.md#select-runtimes-inputs-and-volumes)
for converting numeric workspace manifests and graph-as-runtime entry points.

## Typed requirements and readiness

Workspace `[[inputs]]` declares plaintext or secret aliases. Optional
`[bindings.<alias>]` entries supply `{ value = ... }` or an explicit
`{ env = "VARIABLE" }` binding. `[[runtimes.inputs]]` maps
`ref = "~@/workspace_alias"` to `as = "runtime_alias"`. Existing object-form
`runtimes.inputs` keeps its literal values and explicit environment bindings.
Input defaults are never inferred.

A workspace volume keeps its `name`, optional `path`, `access` (default
`read_write`), `purpose`, and `required` (default `true`). A missing path or an
unavailable directory is reported as missing. `[[runtimes.volumes]]` maps a
workspace volume through `ref`, `as`, and `access` to a declared runtime volume.
Both the receiver's request and the workspace's grant must allow that access.
Existing primary `volume` and `volume_access` attachments still work.

`LocalWorkspaceComposition::requirements()` produces one serializable
`WorkspaceRequirements` report. Each requirement records its declaration scope,
alias, type, purpose, requiredness, sources, consuming runtime, and
availability. Volume rows also record requested and available access. Invalid
mappings, missing bindings, and unsupported/opaque contracts remain distinct.
Ready runtimes in a partially configured workspace can still run independently.

```sh
target/debug/cwr workspace check packages/cwr-core/contracts/cwp/schemas/2026-09/workspace/typed-example.toml --json
target/debug/cwr-tui requirements packages/cwr-core/contracts/cwp/schemas/2026-09/workspace/typed-example.toml
target/debug/cwr workspace invoke packages/cwr-core/contracts/cwp/schemas/2026-09/workspace/typed-example.toml ready @cwr--list--coworkers '{}'
```

`check` prints the report even when incomplete and exits nonzero unless the
whole workspace is ready. Omit `--json` for the shared text presentation. The
TUI `requirements` viewer scrolls the same report; use `workspace` to open or
resume a configured runtime. Without a terminal it prints the text report.

Inspection never resolves environment values. By default an environment-backed
input is missing until a host supplies its availability through
`requirements_with_environment`; that API accepts names only. Reports contain
neither those environment binding names, secret values, nor secret handles. CLI
execution resolves the selected runtime's inputs and validates the prepared
graph before acquiring state. Fully concrete legacy `input_schema` runtimes
still execute, while inspection labels their untyped boundary as unsupported. A
typed secret cannot cross a legacy boundary. Reloads retain the types of
activated inputs; changed volume requirements require reopening through the
workspace so its grants are checked again.

Saved bindings and native credential storage are separate deliveries.

Volume mappings in `WORKSPACE.toml` connect a workspace to its selected runtime.
An ordinary local workspace can also bind a volume requested by an imported
runtime through host-owned product state under `WLP/imported-volumes/`. Each
workspace entry point has a separate local file with `version: 1` and a
`bindings` array; each entry supplies the imported runtime's canonical `source`
file, volume `alias`, folder `path`, and exact `access` (`read` or
`read_write`). CWR rejects missing required volumes, unused bindings, access
mismatches, and unavailable folders. The bound folder is included in the
instructions of coworkers reached through that runtime's imports, together with
its read or read/write access. For loops registered in `WORKLOOP.toml`, Desktop
creates a separate private folder under `WLP/loop-volumes/` for each source and
volume alias, then grants only the access requested by that source.
Registry-created bindings carry `origin: "registry"`; Desktop removes them when
their loop is removed or disabled, even if that source remains imported by the
workspace. Separately approved custom-import bindings have no registry origin
and remain only while their source is reachable. The registry's volume path is
not used as a host grant or copied into the private folder. Desktop preserves
approved grants for imports outside the loop registry. After activation, changed
folder grants require reopening the workspace runtime. This is a local folder
location, not a filesystem sandbox for native processes. A workspace file cannot
approve its own folder access. Desktop requires explicit host approval before an
import outside the loop registry can use a host folder.
