# Building the native host through WABT

The native app uses the existing rbel native architecture:

```text
literate rbel definitions → core.wasm → WABT wasm2c → C → libcwr executable
```

The compiled module implements protocol transitions. Freshly authored C code
supplies bounded JSON/value transport, local storage, deterministic actions,
receipts, and the line-oriented command. WABT's C runtime is linked into the
executable. Running the app needs only platform system libraries; Node, V8,
Python, WABT tools, compiler tools and separate Wasm/JavaScript files are not
runtime dependencies. The standalone Wasm package remains available for embedding.

From `projects/bel-wasm`, generate the core with the existing compiler, prepare
the pinned native WABT tools/runtime, then compile the app:

```sh
python3 libcwr/embedding/build.py --rbel /path/to/rbel-bundle/bin/rbel
python3 experiments/bel-rust-repl/wasm/command/setup-tools.py
python3 experiments/bel-rust-repl/wasm/native/setup-runtime.py
python3 libcwr/embedding/build-native.py
python3 libcwr/embedding/verify.py --rbel /path/to/rbel-bundle/bin/rbel
```

Native compilation requires a C11 compiler and WABT 1.0.39. The setup uses the
existing checksum-pinned official WABT archive and pinned PicoSHA2 build input.
Node may be used by developer tests or the portable Wasm compiler bootstrap;
the native app and its C compilation do not use Node. With the existing native
macOS rbel bundle, core generation also uses its native tools.

`--tools DIR`, `--runtime DIR`, `--runtime-license FILE`, and `--cc COMPILER`
select trusted tool/runtime inputs. For an existing native rbel bundle, use
`libexec/rbel`, `share/rbel/runtime`, and `licenses/WABT-LICENSE` respectively.
`--wasm FILE` selects a recognized standalone package's core. `--output FILE`
selects the native executable. Build on the intended operating system and
architecture; no cross-compilation is claimed.

The default outputs are `target/libcwr-embedding/libcwr`, `libcwr.json`, and
`libcwr.runtime-license.txt`. The manifest records exact core, host C sources,
generated C, compiler, WABT tool/runtime sources and executable hashes. The WABT
license is embedded, accessible through `libcwr --runtime-license`, and supplied
as a notice alongside the executable. Project attribution remains in the package.
Release signing/notarization and publication are separate actions.

Outputs and aliases are checked against maintained source, package files,
compiler/tool/runtime inputs and each other before old native success products
are removed. Compilation uses captured source bytes in private staging and
rechecks source/package hashes before publishing success. A failed validated
attempt leaves no success executable/manifest/license.

The host retains the existing explicit trusted-context boundary and dedicated
local directory. Its process recovery and atomic replacement are not power-loss
or distributed-storage guarantees. It does not implement provider/network or
general-purpose filesystem actions, and fixture controls require `--test-host`.


The executable includes the user-facing commands described in the [CLI guide](cli-guide.md).
`--version` reports the packaged source revision and protocol-core SHA256 so
successive builds can be identified. Uncommitted builds carry a `-dirty` suffix;
source archives without Git metadata are labeled `unversioned-source`. Native
and standalone manifests retain the revision alongside exact source hashes.


Source-backed chat and `/reload` use `native-reload.c/.h` to compile a private
native byte-list worker and replace it after successful checks. The compiler
bundle and system C compiler are development dependencies of this mode. See
[the chat guide](chat-guide.md#edit-and-reload-a-source-coworker) for invocation,
bounds, last-working-version behavior, and retained conversation semantics.
