// Ordinary WebAssembly APIs; no Bel evaluator or project runtime is loaded.
import { readFileSync } from "node:fs";
import assert from "node:assert/strict";

const [file, mode = "copy"] = process.argv.slice(2);
const module = new WebAssembly.Module(readFileSync(file));
assert.deepEqual(WebAssembly.Module.imports(module), []);
const { memory, join, choose } = new WebAssembly.Instance(module).exports;
const view = () => new DataView(memory.buffer);
const isPair = (x) => x >= 4096 && (x & 7) === 0;
function decode(x) {
  return isPair(x)
    ? [decode(view().getInt32(x, true)), decode(view().getInt32(x + 4, true))]
    : x;
}
const char = (0x1f642 << 3) | 2;
const symbol = 33;
const shared = join(1, 0);
const dotted = join(symbol, char);
const nested = join(shared, join(dotted, join(shared, 0)));

if (mode === "copy" || mode === "mutated") {
  const transform = (x) => Array.isArray(x) ? x.map(transform) : 1;
  for (const x of [0, 1, char, symbol, shared, dotted, nested]) {
    const before = decode(x);
    const result = choose(x);
    const after = decode(result);
    assert.deepEqual(after, mode === "copy" ? before : transform(before));
    assert.deepEqual(decode(x), before, "source pair mutated");
    if (isPair(x)) {
      assert.notEqual(result, x, "output must construct new pairs");
    }
  }
  // Force linear-memory growth, then verify old and new handles still work.
  let tail = 0;
  for (let i = 0; i < 9000; i++) tail = join(symbol, tail);
  assert(memory.buffer.byteLength > 65536);
  assert.deepEqual(
    decode(choose(dotted)),
    mode === "copy" ? [symbol, char] : [1, 1],
  );
  assert.throws(() => choose(3), WebAssembly.RuntimeError);
  assert.throws(() => join(3, 0), WebAssembly.RuntimeError);
  // A finite, documented arena must trap cleanly at its maximum.
  assert.throws(() => {
    for (;;) join(0, 0);
  }, WebAssembly.RuntimeError);
  // Failed allocation does not destroy previously allocated pairs.
  assert.deepEqual(decode(dotted), [symbol, char]);
} else if (mode === "calls") {
  assert.equal(choose(nested), shared);
  assert.equal(choose(0), 0);
  assert.throws(() => choose(1), WebAssembly.RuntimeError);
} else if (mode === "identity") {
  assert.equal(choose(join(shared, shared)), 1);
  assert.equal(choose(join(shared, join(1, 0))), 0);
  assert.equal(choose(join(char, char)), 1);
} else if (mode === "type") {
  assert.deepEqual([0, 1, symbol, char, nested].map(choose), [
    17,
    17,
    17,
    25,
    9,
  ]);
} else if (mode === "truth") {
  assert.deepEqual([0, 1, symbol, char, nested].map(choose), [0, 1, 1, 1, 1]);
} else {
  throw new Error(`unknown mode ${mode}`);
}
console.log(JSON.stringify({ file, mode, imports: [], result: "PASS" }));
