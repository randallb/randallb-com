# Embed libcwr

This package exposes the first conversation/action slice from the maintained
literate rbel chapter. It includes the same import-free `core.wasm` for JavaScript
embedding and for the native executable's embedded Wasm engine. Only this slice
is implemented; the chapter lists unmigrated protocol standards.

## Build

From `projects/bel-wasm`, using the existing compiler bundle:

```sh
python3 libcwr/embedding/build.py
python3 libcwr/embedding/build-native.py
```

`--rbel PATH` selects a relocated native or portable compiler bundle. The output
is `target/libcwr-embedding`. Native packaging requires a Node release supporting
`--build-sea`; select an official standalone Node executable with `--node PATH`.
Build tools are not needed when running the resulting executable. See [native build details](native-build.md) for the pinned official runtime and license notices. Native packages
are specific to their build operating system and architecture.

## Embed the core

In a browser or another JavaScript host supporting WebAssembly, copy `core.mjs`,
`values.mjs`, and `core.wasm` together:

```js
import {Core} from './core.mjs';
const core = new Core(await (await fetch('./core.wasm')).arrayBuffer());
const [nextState, response, effect] = core.transition(state, event, trustedEnvironment);
// Persist nextState successfully before executing effect.
core.close();
```

In Node, use `const {Core} = require('./core.cjs')` and pass the Wasm file's bytes.
The adapter has no protocol decisions. Each call creates a new bounded Wasm
instance and crosses the boundary with values, so saved state holds no arena
pointers and repeated requests do not accumulate an instance's arena allocations.
The core returns a proposed state, response and effect. The embedding owns
serialization, atomic commit, current authority, effect execution and authenticated
completion/reconciliation evidence. It must serialize transitions for its store.

The value ABI uses JSON-compatible data: `null` for nil, strings for interned
symbols, arrays for proper lists, `{"char":"x"}` for one Unicode scalar, and
`{"pair":[head,tail]}` for improper lists. Records are lists of `[key,value]`
entries. The request is the chapter's existing internal tuple contract, not a
public ingress/wire API. The host must resolve canonical references and supply
checked context and authenticated identities; caller input cannot set authority,
claim tokens or receipts. Keep this boundary when connecting another application.

## Native host

The executable embeds both the Wasm engine and the exact `core.wasm` asset. It
runs the Wasm core internally; this is not direct native compilation of rbel.
Its local host persists state and receipts and performs the chapter's deterministic
`store--text` and `append--text` actions in a dedicated local directory.

Supply a trusted context file and feed one event per line:

```sh
./libcwr --directory ./session --context ./example-context.json < example-events.jsonl
```

The context file is trusted host configuration, not request-author data. The
first Wasm session recovery loads retained state. Events are processed serially.
The host commits state before effects, keeps durable bound receipts, and surfaces
uncertainty for reconciliation after a restart. It does not implement provider,
network, general-purpose filesystem actions or Workloop integration.

`--test-host` is an explicit local fixture interface used by acceptance to inject
failures and alter fictional context. Do not expose that interface as an ingress
endpoint. The normal interface accepts protocol events only.

## Acceptance and limits

Run `python3 libcwr/embedding/verify.py` after building both products. It runs the
adjacent workflow through the standalone Wasm package and through the native
executable, compares assertions and host traces, checks malformed transport and
packaged-file integrity, and exercises process restart and receipt recovery.
Native execution must work with no Node/compiler/build tools on PATH. Reports
record actual source/artifact hashes; building alone does not establish acceptance.

The host is single-writer local storage. It holds `.libcwr-lock` for the session.
After an ungraceful process termination, confirm that the recorded owner has
stopped, then remove that lock before reopening the directory. Automatic stale
lock takeover is avoided because competing recovery processes can race.
The state and receipts remain available for protocol reconciliation. Atomic replacement demonstrates process
interruption recovery; it is not distributed locking, power-loss durability or
exactly-once external effects. The core adapter enforces value depth/node bounds;
embedders also own request sizes, process timeouts and service-level resource
isolation. Browser compatibility is based on the standard WebAssembly and ESM
interfaces; an actual browser run must be recorded separately before claiming it.
