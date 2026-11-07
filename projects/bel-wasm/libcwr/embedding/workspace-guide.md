# Workspace conversations and coworker actions

Launch the native command at workspace home:

```sh
libcwr workspace --directory ./workspace
```

With no arguments in a terminal, `libcwr` opens workspace home using `.libcwr`
in the current directory. The home lists/navigates shared conversations and
available coworkers; it is not a conversation and does not activate anyone.
The explicit `chat` command keeps the earlier single-coworker demonstration.

## Local identity

A new workspace saves a hostname-based runtime identity in `context.json`:
`cwp://local.<hostname>/runtimes/<random UUID>`. On macOS, the host uses the
computer's Bonjour `LocalHostName` when available, with the system hostname as
a fallback. The short hostname is normalized to lowercase letters, digits and
hyphens. It identifies a local namespace; opening a reference does not perform
a DNS lookup. Separate workspaces receive separate UUIDs.

The saved identity is authoritative. Restarting or moving the workspace, or
renaming the computer, preserves its exact identities, authorship, permissions
and history. Older saved workspaces retain their existing `org.example.libcwr`
references; upgrading the executable does not rename or migrate those records.
New claims in a saved workspace belong to its existing runtime namespace.

## Start a conversation

```text
workspace> /new notes
```

Use `/inspect` (or `/list`, `/conversations`) to list retained conversations, `/open notes` (or `/use
notes`) to reopen one, and `/home` to return to workspace home. Opening changes
where you are looking; it does not change conversation ownership or erase history.

Inside a conversation, ordinary text publishes one shared message and activates
nobody. Participants can read shared history under current authorization. A
mention such as `@uppercase hello` requests work from that coworker, joining it
if it is available in this workspace and the request is accepted. The prompt
remains available while a coworker works. Replies retain their actual author,
source message and invocation links. Publishing replies/progress does not by
itself create another request.

The directory includes available people and software even when they have no
conversational execution route. Such coworkers may expose actions through `/actions`;
the initial conversational software route is `uppercase`.

`/invite uppercase` joins the available coworker without activating it.
`/remove uppercase` ends participation and future delivery, cancels queued work
and requests cancellation of running work in that conversation. Earlier messages
and completed results remain. Cancellation does not undo an effect already
performed; uncertain outcomes remain visible rather than being retried silently.
Re-invitation never replays old requests or resumes cancelled work automatically.

## A named source coworker

To run Greeter alongside the builtin uppercase coworker:

```sh
libcwr workspace --directory ./workspace \
  --coworker greeter --source libcwr/coworkers/greeter.rbel.md \
  --rbel target/rbel-command/bin/rbel
```

Use `/coworkers` and `/actions` to discover it, then `/new demo` and
`@greeter Randall`. Greeter replies `Hello, Randall`; `@uppercase Randall`
replies `RANDALL` using its separate identity and builtin implementation.
Invitation alone starts no work. Both replies retain their actual coworker
author, source message and invocation links.

The launcher selects one named source coworker. Its source and compiler paths
are trusted launch configuration; conversation text and action inputs cannot
select arbitrary files. `/check` and `/reload` apply to that configured source
coworker, leaving builtin uppercase unchanged. Start again with the same name,
source and saved directory to retain its identity and conversation history.
The earlier `--source` form without `--coworker` still supplies the uppercase
demonstration's implementation. A general registry of multiple source files is
not part of this first named-source interface.

## Inspect and control

| Command | Purpose |
| --- | --- |
| `/actions` | Discover the REPL/language actions and their exact protocol targets. |
| `/coworkers` | Inspect the workspace's available coworkers. |
| `/new NAME` | Create a named conversation and open it. |
| `/open NAME`, `/use NAME` | Open an existing conversation. |
| `/inspect`, `/list`, `/conversations` | List visible conversations using the same REPL list action and output. |
| `/home` | Return to workspace home. |
| `/invite NAME` | Join an available coworker without requesting work. |
| `/remove NAME` | Remove a participant and request cancellation of its work. |
| `/history` | Read the current conversation's attributed history. |
| `/status` | Inspect queued/running/completed/cancelled work. |
| `/check` | Check configured rbel source and its imported examples without activating a new worker. |
| `/reload` | Check examples, rebuild, and activate configured source only on success. |
| `/help`, `/exit` | Display controls or leave the terminal session. |

Workspace domain slash commands are real invocations of discoverable REPL or
language coworker actions. The terminal parses input and displays results; it
does not privately implement membership or protocol transitions. Control commands
do not add those control coworkers as conversation participants. Requests retain
caller identity, exact target, inputs and results, and current authority applies
through the same entry point used by other authorized callers. Discovery or
membership alone grants no operation authority. Root workspace navigation
confers no administrative privilege.

## Check source and reload

To use the reloadable worker instead of the builtin uppercase transformation:

```sh
libcwr workspace --directory ./workspace \
  --source libcwr/coworkers/uppercase.rbel.md \
  --rbel target/rbel-command/bin/rbel
```

After editing the configured source or a relative import, use `/check`. It invokes
`language--do--check`, runs the source's executable examples (including imported
examples), and reports the compiler's diagnostics. Example failures identify the
source file, fence line and example name. The invocation retains the check result
and diagnostics so they remain inspectable after the terminal output scrolls away.
A successful check leaves the current worker and conversation history unchanged.

Then use `/reload` to check the examples again, build and activate the edited
worker. Source changes take effect only after a successful `/reload`.
Definition/import/example errors retain the last working build. A separate check
does not reserve a later reload result: reload validates the source as it exists
when reload runs. Both commands work from workspace home or an open conversation
through the same discoverable language coworker action boundary. The compiler bundle and system C
compiler are required to rebuild. Conversations, participants and completed work
are preserved. The existing native byte-list worker's independent 256-byte chunk
behavior and resource limits still apply; see [source chat](chat-guide.md).

Read-only control results point to an authorized live view rather than retaining
another copy of the whole history or prior status output. Retrying the same read
can therefore return the current view. Mutating requests retain their original
outcome; changing the input for a reused request identity is rejected.

Keep the same workspace directory to retain history. Reopening does not re-execute
completed actions. Interrupted attempts remain inspectable and must not silently
repeat external effects. The local owning-session lock prevents a second writer;
never remove a live owner's lock to open another process. This workspace interface
exposes `/status`; it does not yet add in-session `/run` or `/reconcile` controls.
Exit before using the existing one-shot/JSONL inspection and recovery interfaces.
Interrupted source-worker attempts remain uncertain and are not restarted silently.

This local milestone does not add models, a linter coworker, network collaboration,
installed-pack navigation, a general file browser, or migration into Workloop.

## Ordinary rbel resource programs

A complete example lives in `libcwr/coworkers/looper.rbel.md`. It imports the
shared ordinary rbel resource functions, constructs its identity, role, action,
binding and interfaces, and supplies an actual handler. No TOML resource is read.

```sh
libcwr workspace --directory ./looper-state \
  --resource ./libcwr/coworkers/looper.rbel.md --rbel /path/to/rbel
```

Use `/resource` to inspect the checked definition and source/build provenance.
Use `/actions` to discover its exact action target. In a conversation:

```text
/new demo
/invite looper
/call looper draft--brief {"topic":"Launch","notes":"Start with one useful step."}
```

Looper returns `{"brief":"Brief: Launch\nStart with one useful step."}`. This
example formats supplied information deterministically; it does not summarize
or use a model. The invocation retains structured input/output, the actual
request author, exact target and response links. Invitation alone starts no work.

`/check` compiles and checks examples and resource interfaces without activating
edits. `/reload` replaces the active program only after those checks pass.
Changing the handler or moving a source tree preserves its logical identity;
relative module imports must still resolve. This first slice keeps the existing
role/action interfaces fixed during reload. A new identity must be introduced
explicitly in a separate workspace, rather than inheriting old permissions.

These are ordinary rbel programs with a bounded JSON value transport. Interface
checking is ordinary rbel, including exact record fields and text requirements.
It is an initial runtime resource interface, not a general static type system.
