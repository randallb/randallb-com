# Chat with the local rbel coworker

Launch the native executable from your terminal:

```sh
./libcwr chat --directory ./workspace
```

A new dedicated workspace initializes automatically. Existing workspaces keep
their configuration and stored work. The default conversation is `chat`; return
to the same directory to reopen it. Running `./libcwr` without arguments in a
terminal also opens chat, using `.libcwr` in the current directory.

Type ordinary text at the `you[chat]>` prompt:

```text
you[chat]> hello
uppercase: HELLO
you[chat]> another thought
uppercase: ANOTHER THOUGHT
```

The coworker runs the existing rbel uppercase transformation. It changes ASCII
`a`–`z` to `A`–`Z`; other characters, including accented letters and emoji, stay
unchanged. It runs locally without a model, network account, Node, Python,
compiler, or separate Wasm file. This first coworker demonstrates the protocol
conversation and action workflow; it does not interpret natural-language tasks.

Each nonempty message is submitted and executed automatically. You do not need
to invent invocation names or enter `submit` and `run`. Both your source message
and the coworker's response are retained, with their actual authors, reply
relationship, and invocation identity. Closing the terminal and returning does
not erase that history or repeat completed work.

Quotes, backticks, and `$()` in ordinary messages are literal text. They do not
run shell commands. Enter one message per line. A blank line does nothing.

## Conversation controls

Controls begin with `/`:

| Command | Result |
| --- | --- |
| `/new notes` | Create or select a named conversation. |
| `/new` | Create a conversation with a generated name. |
| `/use chat` | Reopen a saved conversation. |
| `/history` | Display the current conversation with author labels. |
| `/coworker` | Show the selected coworker. |
| `/coworker uppercase` | Select the local uppercase coworker. |
| `/status` | Inspect retained conversations and invocation progress. |
| `/recover` | Inspect interrupted work after recovery. |
| `/reconcile NAME` | Check a retained invocation's bound action receipt. |
| `/run NAME` | Run admitted work or a permitted safe retry. |
| `/help` | Show chat controls. |
| `/exit` or `/quit` | Leave chat. End of input also exits. |

Only the local uppercase coworker is provided in this version. Unknown commands,
unavailable conversations, and unsupported coworker names report errors and
leave the prompt available. To send text beginning with a slash, type two:
`//this is a message` sends `/this is a message`.

Conversation names contain 1–64 ASCII letters, digits, hyphens, or underscores.
Chat uses one workspace directory for the session. Choose that directory at
launch, rather than passing directory options to individual chat controls.

## Interrupted work

An action can execute before its completion is retained. Such work remains
visible as uncertain; a missing receipt does not prove the action never ran.
Use `/status` to find its invocation name and `/reconcile NAME` to check its
bound receipt. Successful reconciliation retains one response without executing
the action again. Recovering or reopening completed work must not add another
copy of its reply. If reconciliation has checked the missing receipt and the exact
action is declared safe to repeat, `/run NAME` can retry it. The current
reconciliation response remains `reconciliation-required`; its retained
`checked` marker records that the receipt was checked. With a missing receipt,
a permitted retry can execute the action again; the completed response is
retained once. Work that has not passed reconciliation cannot run again.

The local host allows one owner at a time. If an ungraceful termination leaves
`.libcwr-lock`, confirm the recorded owner has stopped before removing the lock.
Do not remove a live owner's lock. This is local process-recovery behavior,
not distributed storage or power-loss durability.

The old command interface remains available through `./libcwr repl` and the
one-shot commands in [the CLI guide](cli-guide.md). The raw JSONL developer
transport remains available with an explicit trusted context. Existing stored
text artifacts can still be inspected through those interfaces.
