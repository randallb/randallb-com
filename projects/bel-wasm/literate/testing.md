# Adjacent tests executed on Wasm

`test.py` adds named `rbel test=NAME` fences to the existing definition and example
format. It extracts source; the existing reader/compiler tooling owns parsing,
closures and evaluation. The default native bundle uses the reader written in
rbel. The optional `bootstrap-wasm.py` bundle uses the existing Rust syntax reader
and Wasm rbel compiler for CI; Rust performs no protocol evaluation. The Python and JavaScript code do not evaluate rbel.
The native `rbel check` command continues to check the older definition/example
format; use this test command for chapters containing test fences.

```sh
python3 literate/test.py path/to/chapter.rbel.md
python3 literate/test.py libcwr/workflow.rbel.md --entry cw-engine --host libcwr/host.mjs
```

A complete chapter can contain:

````markdown
```rbel definition=same
(def same (x) x)
```

```rbel example=same-symbol
(same 'hello)
=>
hello
```

```rbel test=two-steps
(rt-assert 'first (same 'hello) 'hello
  (fn (ignored)
    (rt-assert 'second (same nil) nil
      (fn (ignored) (rt-done)))))
```
````

Each test is one rbel expression. Its value is `(done)` or a suspension generated
by `(rt-request operation arguments continuation)`. The continuation is a compiled
function receiving the operation's reply. It can compute, branch, assert, or
request the next operation. `rt-assert` compares actual and expected values
structurally inside Wasm, reports a name and its values, then resumes the next
continuation. Each case must make at least one assertion. Example expectations
are quoted data and never evaluated. All identifiers are unique across input
files; `rt-` definition names are reserved for the runner.

`--include FILE` adds definitions/examples/tests. `--entry NAME` selects a
one-argument compiled entry function for the host's `engine.call(data)` operation;
the default is identity. `--host FILE.mjs` explicitly loads a trusted mechanism
module exporting `createHost({module, WasmValues, directory})`. It returns
`request(operation, arguments)`, optional `evidence()`, and optional `close()`.
The host may construct an independent Wasm instance from `module` and call the
selected entry with data. Its policy remains in compiled source. The generic
runner knows nothing about conversations, invocations, permissions or retries.

A test's continuation lives in its own driver instance. A host can destroy and
recreate the protocol instance without serializing that continuation. The libcwr
host demonstrates disk-backed value recovery. It never restores an old Wasm
pointer into a new instance. Host controls are test fixtures, not public APIs.

`--report FILE.json` writes assertion outcomes and source/compiler/Wasm/host hashes.
`--docs FILE.md` generates an exact copy of authored sources with source hashes,
after successful acceptance. `--verbose` prints assertion names. Prior output
files are removed before a valid run starts; input/output aliases, including
hard links, are rejected before removal. The complete compiler bundle is protected
from output writes. Source is captured once for extraction, compilation, hashes
and documentation; changed or missing input at the end of the run fails and
removes success evidence. Generated code and Wasm are private
temporary files and never reused after failure. Reports and docs should live in
ignored `target/`. The tool does not publish them.

Errors name the source, fence line and test when execution fails. Malformed
headers, empty/unclosed fences, duplicate names and unsupported syntax fail.
There are at most 32 inputs of 128 KiB each; the combined compiler input retains
the native command's 128 KiB limit. The existing 64 MiB per-instance arena and
compiler limits apply. Data transport has 128-depth/262144-node bounds; symbol
names are limited to 4096 characters; tests have at most 2048 host steps. Wasm
traps and parent-enforced process timeouts fail the command. The default Node
execution timeout is 120 seconds, configurable with `--timeout`.

Use `python3 libcwr/verify.py` for positive and deliberate failure runs, including
changes to an actual definition and expected result, traps, malformed blocks,
and the existing foundation/reader examples. A passing runner is not evidence
for an application whose definitions were not exercised.

The portable bootstrap bundle can be selected with
`--rbel target/rbel-wasm-bootstrap/bin/rbel`. Build it with
`python3 literate/bootstrap-wasm.py` after the documented Rust-reader and Wasm
compiler bootstrap. Its manifest pins the packaged reader, compiler, transport
and assembler bytes. It is not a substitute for testing the native command.
