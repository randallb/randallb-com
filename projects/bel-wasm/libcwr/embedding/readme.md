# Embed libcwr

This package exposes the first conversation/action slice from the maintained
literate rbel chapter. It includes the same import-free `core.wasm` for JavaScript
embedding and as the input to the native executable's WABT/C compilation. Only this slice
is implemented; the chapter lists unmigrated protocol standards.

## Build

From `projects/bel-wasm`, using the existing compiler bundle:

```sh
python3 libcwr/embedding/build.py
python3 experiments/bel-rust-repl/wasm/command/setup-tools.py
python3 experiments/bel-rust-repl/wasm/native/setup-runtime.py
python3 libcwr/embedding/build-native.py
```

`--rbel PATH` selects a relocated native or portable compiler bundle. The output
is `target/libcwr-embedding`. Native compilation uses WABT 1.0.39 `wasm2c` and a system C compiler.
The resulting executable needs only system libraries. See [native build details](native-build.md) for tools and license notices. Native packages
are specific to their build operating system and architecture.

## Use the native CLI

The native executable also provides readable commands over a persistent local
workspace. Start with the [CLI guide](cli-guide.md); no JSON authoring is required.

```sh
./libcwr init --directory ./trial
./libcwr conversation create notes --directory ./trial
./libcwr submit notes first --text "Hello from libcwr" --directory ./trial
./libcwr status --directory ./trial
./libcwr run first --directory ./trial
./libcwr artifact first --directory ./trial
./libcwr history notes --directory ./trial
./libcwr --version
```

Submitting admits work; `run` performs the current deterministic local text
storage action. Commands retain state between processes. The current CLI does
not call a model/provider. `--json` selects machine output, and the original
JSONL interface remains available to embedders. Default local data directory is
`.libcwr`; explicit directories make separate experiments easy to keep apart.

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

The executable compiles the exact `core.wasm` through WABT `wasm2c` and C,
using the existing rbel native build architecture. It has no Node/V8 or separately
installed Wasm-engine runtime dependency. Its C host persists state and receipts and performs the chapter's deterministic
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

The host is single-writer local storage. The JSONL transport holds `.libcwr-lock`
for the session; REPL commands acquire and release it for each operation.
After an ungraceful process termination, confirm that the recorded owner has
stopped, then remove that lock before reopening the directory. Automatic stale
lock takeover is avoided because competing recovery processes can race.
The state and receipts remain available for protocol reconciliation. Atomic replacement demonstrates process
interruption recovery; it is not distributed locking, power-loss durability or
exactly-once external effects. The core adapter enforces value depth/node bounds;
embedders also own request sizes, process timeouts and service-level resource
isolation. Browser compatibility is based on the standard WebAssembly and ESM
interfaces; an actual browser run must be recorded separately before claiming it.

## Chat with a software coworker

`libcwr chat --directory ./workspace` opens a conversation with the local rbel uppercase coworker. Ordinary text submits and runs an action, retaining both authors and the reply link. See the [chat guide](chat-guide.md). No arguments on a terminal starts chat; `repl` keeps the explicit command interface.


Source-backed chat supports an explicit `/reload` edit loop with
`chat --source SOURCE --rbel COMPILER`. The maintained sample is
`libcwr/coworkers/uppercase.rbel.md`; it uses relative imports. The chat guide
explains native worker replacement, retained history, and failed-build recovery.
Run `python3 libcwr/embedding/reload-verify.py` after building the native host
for isolated same-process source reload checks.


The workspace-first interface is documented in [workspace conversations](workspace-guide.md).
Its domain slash commands invoke discoverable REPL/language coworker actions;
plain messages publish without activation, and mentions request work from available
coworkers. The explicit `chat` command retains the earlier demonstration interface.
Run `python3 libcwr/embedding/workspace-verify.py` for the new native workflow.
