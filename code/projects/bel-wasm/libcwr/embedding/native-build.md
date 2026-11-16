# Building the native host

The executable embeds the same `core.wasm` as the browser/JavaScript package,
its host adapter, and Node's V8/Wasm runtime. It executes Wasm internally;
protocol transitions remain in the module. The executable does not load scripts
or Wasm from the checkout and does not need `node`, Python, or build tools on PATH.

First generate the core package under `target/libcwr-embedding`. Then, from
`projects/bel-wasm`, use an official portable Node distribution with
[`--build-sea`](https://nodejs.org/api/single-executable-applications.html):

```sh
python3 libcwr/embedding/build-native.py \
  --node target/native-node-v26.12.0/node-v26.12.0-darwin-arm64/bin/node
```

The outputs are `target/libcwr-embedding/libcwr`, `libcwr.json` (input/runtime and
executable hashes), and `libcwr.node-license.txt` (Node's full license including
third-party notices). The license is also embedded and available through
`libcwr --runtime-license`. Keep the license notice with any distribution.

For local builds, the selected Node distribution must contain its `LICENSE` at
its root, beside `bin/`. `--node-license PATH` supports another location.
`--wasm`, `--main`, and `--output` select the module, first-party main entry and
executable destination. `core.cjs`, `values.cjs`, and `host.cjs` must be beside
the selected Wasm module. The builder statically bundles those explicit modules
and permits only explicit bundled modules and `node:` builtins at runtime.

On macOS the builder checks the selected runtime's dylibs and rejects non-system
dependencies. A Homebrew Node binary may depend on Homebrew's `libnode` and many
other dylibs; copying it into a SEA would not make it independently portable.
The tested official Node v26.12.0 `darwin-arm64` binary links only macOS system
libraries. Build the executable on the intended OS and architecture; this builder
does not claim cross compilation or verification on other platforms.

The verified local bootstrap archive is:

- URL: <https://nodejs.org/dist/v26.12.0/node-v26.12.0-darwin-arm64.tar.xz>
- SHA-256: `a2e255e35347f727f29aa158c552a60140b17b6ba68719d5d6fc46c7d058aa75`
- Checksum listing: <https://nodejs.org/dist/v26.12.0/SHASUMS256.txt>

Download the archive into ignored `target/native-node-v26.12.0`, verify its SHA-256
against that value and the official listing, and extract there. The original
archive and checksum listing are retained locally as build evidence. The archive
supplies the runtime license and third-party notices; its implementation is a
platform runtime dependency, not a CWR implementation dependency.

Build inputs and aliases are checked before old executable products are cleared.
A failed validated build leaves no success executable or manifest. Build inputs
are captured, packaged from those bytes, and checked again before publishing the
executable. The builder strips `NODE_OPTIONS`/`NODE_PATH` while building and sets
SEA `execArgvExtension` to `none`. macOS binaries are signed ad hoc for local use;
release signing/notarization and publishing are separate work.

The runtime makes the executable much larger than `core.wasm`. Inspect
`executable_bytes` in `libcwr.json`; this is an embedded runtime distribution,
not a small direct-machine-code compilation of the rbel core.
