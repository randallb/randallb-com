# Ink on Deno compatibility demo

Synthetic conversation only. No Workloop host connection or customer state. Uses
Ink 6.3.1, React 19.1.0, React types 19.1.5 and the repository's Deno 2.9.7. The
independent config and lock leave Desktop dependencies untouched.

From the checkout root, with direnv loaded:

```sh
direnv exec . deno task --config experiments/ink-deno/deno.json demo
direnv exec . deno task --config experiments/ink-deno/deno.json check
direnv exec . deno task --config experiments/ink-deno/deno.json compile
./experiments/ink-deno/artifacts/ink-deno
```

Enter inserts a newline; Ctrl-E submits the local draft; PageUp/PageDown move
through synthetic history; Ctrl-C exits. The response streams automatically. The
composer only appends text and supports backspace; this is not a complete
editor. Wrapping counts Unicode code points, not graphemes or terminal cell
widths. Wide characters, complex emoji, long drafts, bracketed paste and tiny
terminals need further work before using this as product code.

## Deno tests

```sh
direnv exec . deno task --config experiments/ink-deno/deno.json test
```

`Deno.test` is the runner. It does not supply PTYs, terminal screen parsing or
termios. The tests use the repository's existing tmux executable for real PTYs,
key input, resize and rendered screen capture, plus macOS `/bin/stty` for raw
mode and terminal attribute checks. No Python, venv, extra npm dependency or
native addon is required. Each case creates its own tmux server on a short,
temporary socket under `/tmp` and kills that server in `finally`; existing
development sessions are untouched.

Three tests have different scopes:

- Non-TTY source execution checks rejection of piped input, exit 1 and the exact
  diagnostic. It does not exercise the interactive renderer.
- Source execution in a real tmux PTY checks rendering, multiline input and
  backspace, older history and draft retention while text streams, paging, 80×24
  → 50×18 → 100×32 resize, submission and Ctrl-C exit 0. It checks raw input
  flags, exact before/after terminal attributes and the cursor-show escape.
- macOS arm64 packaging compiles a disposable source/config/lock copy, removes
  that source and its dependencies, checks the binary's non-TTY diagnostic, and
  exercises the compiled conversation in real PTYs with Ctrl-C and SIGTERM. Each
  case gets a fresh empty DENO_DIR. SIGTERM checks shell exit 143 and exact
  terminal restoration. The checkout source is never moved or removed.

The source PTY test is skipped outside macOS; the compiled PTY test is skipped
outside macOS arm64. A run with these skips does not establish terminal or
packaging compatibility on that platform. Deno's default resource and operation
sanitizers remain enabled. The test command grants read/write/environment and
subprocess permissions for these disposable local fixtures.

The tests wait for complete expected screen content rather than only a status
line. They save final rendered screens and raw ANSI output in ignored
`artifacts/`; temporary sources, binary, caches and tmux servers are cleaned up.
If the test process is forcibly killed, any leftover fixtures remain isolated
temporary data; the checkout source stays intact.

## Manual terminal rehearsal

Run both the source demo and the compiled executable in your terminal. While the
reply streams, type two lines using Enter, correct a typo with backspace, page
to older history and back, resize narrow/wide and send using Ctrl-E. Verify
draft/history retention, then exit with Ctrl-C and check ordinary shell input,
echo and cursor behavior. For an exact comparison, run `stty -g` before and
after the demo. Repeat with SIGTERM from another shell using the demo's PID.

Automation checks a real tmux PTY, not visual behavior in Terminal/iTerm or
every terminal emulator. The original Python/PTY investigation and logs remain
historical local evidence under ignored artifacts, not prerequisites or files
published in this experiment.

The experiment passed on macOS 26.5.2 (25F84), arm64. The binary is
approximately 74 MiB. This establishes local runtime and packaging feasibility,
not an authenticated CLI, a production editor, performance at large history
sizes, release signing/notarization, or support on other operating
systems/terminals.
