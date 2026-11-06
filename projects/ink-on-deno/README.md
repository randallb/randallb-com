# Ink on Deno

A terminal chat-interface experiment using React and Ink on Deno. It uses synthetic conversations and has no Workloop connection or customer data.

Requires Deno 2.9 or newer. From this archive's root:

```sh
cd experiments/ink-deno
deno task demo
```

Enter inserts a newline, Ctrl-E submits, PageUp/PageDown browse history, and Ctrl-C exits.

## Verify and package

```sh
deno task check
deno task compile
```

Full terminal and packaging tests require macOS, tmux, and `/bin/stty`; the compiled-binary test requires macOS arm64. The compile task targets macOS arm64. Other platforms have not been established by those tests. Unicode cell widths, complex emoji, long drafts, and tiny terminals have known limitations.

`ORIGIN.json` identifies the upstream revision. Original source and documentation remain unchanged in `experiments/ink-deno`. This README and attribution files were added for standalone distribution. Run upstream commands without `direnv exec .` when Deno is already installed. See the original README for exact test coverage and limitations.
