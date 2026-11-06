# Use libcwr from the terminal

The native `libcwr` executable now gives names and readable output to the
conversation/action engine. You can keep a local workspace, return in a new
terminal process, and inspect what the engine retained as the implementation
evolves. Running it needs no Node, Python, compiler, or separate Wasm file.

This first version runs one local action: **store text**. The text you submit
becomes an invocation-owned artifact when you run that invocation. It does not
call a model, contact coworkers, or connect to Workloop yet.

## Start an interactive session

From the folder containing the executable:

```sh
./libcwr repl --directory ./trial
```

You get a prompt. Enter these commands one at a time:

```text
init
conversation create journal
submit first --text 'Hello from libcwr'
status
run first
artifact first
history
```

`conversation create journal` selects that conversation, and the prompt becomes
`libcwr[journal]>`. `submit first` uses the selected conversation. `use NAME`
switches to an existing conversation after checking that you can read it.
`history` without a name shows the selected conversation. Selection lasts for
the session; use `use journal` when returning in a new session. You can also use
the explicit forms `submit journal first --text 'Hello from libcwr'` and
`history journal`.

`init` creates a fictional local identity, trusted context, and local workspace.
It refuses to replace existing workspace configuration or take over an unrelated
nonempty directory. `submit` admits the work and records its source message; it
does not execute the action. Before `run`, status shows `first` as `admitted`.
After `run`, it is `completed`, and `artifact` prints exactly `Hello from libcwr`.
The REPL adds a display newline after an artifact so the next prompt starts
on a new line. The separate `artifact` command writes the exact text without
adding a newline.

Use `help` to see commands and `exit` or `quit` to leave. End of input also exits.
Blank lines do nothing. An invalid command or rejected operation reports its
error and leaves the session available for the next command. Quotes group text
containing spaces; single and double quotes and backslash escaping are supported.
These commands do not invoke a shell: `$()`, backticks, and semicolons inside
text remain literal text. Enter each command on one line.

Running `./libcwr` with no arguments in a terminal also opens the REPL.
Without a terminal, no arguments prints help; scripts can explicitly select
`repl` and pipe commands into it. The workspace directory is fixed for the
session: choose it at launch, rather than putting `--directory` on each prompt
command. Nested `repl` commands are rejected.

If you omit `--directory`, libcwr uses `.libcwr` in the current directory. Use
an explicit directory when moving between shells or comparing builds. No command
uses a global cloud account or searches other folders for a workspace. Each
operation opens the workspace and releases it when finished, so an idle REPL
does not retain its owner lock. The next command reads retained state afresh.

All commands also remain available as separate terminal invocations:

```sh
./libcwr --version
./libcwr status --directory ./trial
./libcwr show first --directory ./trial
```

## See what happened

```sh
./libcwr list conversations --directory ./trial
./libcwr list invocations --directory ./trial
./libcwr show first --directory ./trial
./libcwr history journal --directory ./trial
./libcwr status --directory ./trial
```

`show` displays the retained invocation and its current phase. `history` shows
the conversation's admitted source messages. `status` gives an overview of
conversations and invocations. Reads pass through the same protocol authority
checks as the engine; changing the configured caller does not make another
participant's records visible.

Choose names containing 1–64 ASCII letters, digits, hyphens, or underscores.
Conversation and invocation names are scoped to the local workspace. The CLI
maps them to retained canonical references; you do not need to type those refs.

The invocation name is also its retry identity. Submitting `first` again with
the same conversation and text recovers the existing admission. Changing that
text while reusing `first` reports a conflict. Use a new name for new work:

```sh
./libcwr submit journal second --text 'Another item' --directory ./trial
./libcwr run second --directory ./trial
```

Running a completed invocation again returns its retained completion and does
not repeat the action. State survives process restarts, so these commands can
be run on different days with the same workspace directory.

## Inspect recovery

```sh
./libcwr recover --directory ./trial
./libcwr show first --directory ./trial
./libcwr reconcile first --directory ./trial
```

Opening the workspace recovers retained engine state. `recover` makes that
operation explicit and shows the resulting overview. If a process stopped after
starting an action, the invocation stays visibly uncertain. `reconcile` asks the
engine to check its bound local receipt; a matching completed receipt can finish
that invocation without repeating the action. Missing evidence does not prove
that an action never ran. Inspect the returned status before choosing another
step; a blocked or denied operation exits unsuccessfully.

Only one process may own a workspace at a time. An ungraceful termination can
leave `.libcwr-lock` behind. Confirm that its recorded process has stopped before
removing that lock and reopening the workspace. `recover` does not silently
remove an existing owner's lock.

## Follow changes between builds

`./libcwr --version` identifies the application build and compiled protocol core.
Keep the workspace directory while replacing the executable with a later build,
then use `status`, `show`, and `history` to see retained work. Keep a copy of the
workspace before trying an incompatible future data-format change; this initial
interface does not introduce a general migration/import/export facility.

For scripts, append `--json`:

```sh
./libcwr status --directory ./trial --json
./libcwr show first --directory ./trial --json
```

A successful JSON reply has `command`, `code`, and `value` fields. Protocol
rejections also supply their code and return a nonzero exit status. Invalid
command syntax or local file errors print a diagnostic to stderr and return
unsuccessfully. `--help` lists the commands and quick start. No arguments prints help when
standard input is not a terminal.

## What the directory contains

`context.json` is trusted local host configuration, not request input.
`state.json` holds retained protocol values. The host also stores invocation-keyed
artifacts, bound receipt files, and `executions.jsonl`, plus a lock while a
process owns the workspace. Keep the directory private and intact rather than
editing retained state by hand. Local atomic replacement supports process
recovery; this is not distributed storage or a power-loss durability guarantee.

The earlier JSON-lines event interface remains available for developers. The
user commands construct those events and use the same native engine and host
mechanisms. Failure injection and context-changing fixture operations remain
behind the explicit `--test-host` developer interface; normal commands do not
expose them. Failure/cancellation lifecycle expansion and provider integrations
are separate future work.
