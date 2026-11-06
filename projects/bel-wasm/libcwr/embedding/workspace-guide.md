# Workspace conversations and coworker actions

Launch the native command at workspace home:

```sh
libcwr workspace --directory ./workspace
```

With no arguments in a terminal, `libcwr` opens workspace home using `.libcwr`
in the current directory. The home lists/navigates shared conversations and
available coworkers; it is not a conversation and does not activate anyone.
The explicit `chat` command keeps the earlier single-coworker demonstration.

## Start a conversation

```text
workspace> /new notes
```

Use `/conversations` to list retained conversations, `/open notes` (or `/use
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

## Inspect and control

| Command | Purpose |
| --- | --- |
| `/actions` | Discover the REPL/language actions and their exact protocol targets. |
| `/coworkers` | Inspect the workspace's available coworkers. |
| `/new NAME` | Create a named conversation and open it. |
| `/open NAME`, `/use NAME` | Open an existing conversation. |
| `/conversations` | List visible conversations. |
| `/home` | Return to workspace home. |
| `/invite NAME` | Join an available coworker without requesting work. |
| `/remove NAME` | Remove a participant and request cancellation of its work. |
| `/history` | Read the current conversation's attributed history. |
| `/status` | Inspect queued/running/completed/cancelled work. |
| `/reload` | Invoke the language coworker's source rebuild/reload action. |
| `/help`, `/exit` | Display controls or leave the terminal session. |

Workspace domain slash commands are real invocations of discoverable REPL or
language coworker actions. The terminal parses input and displays results; it
does not privately implement membership or protocol transitions. Control commands
do not add those control coworkers as conversation participants. Requests retain
caller identity, exact target, inputs and results, and current authority applies
through the same entry point used by other authorized callers. Discovery or
membership alone grants no operation authority. Root workspace navigation
confers no administrative privilege.

## Source reload

To use the reloadable worker instead of the builtin uppercase transformation:

```sh
libcwr workspace --directory ./workspace \
  --source libcwr/coworkers/uppercase.rbel.md \
  --rbel target/rbel-command/bin/rbel
```

Source changes take effect only after a successful `/reload`. Definition/import/
example errors retain the last working build. The compiler bundle and system C
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
