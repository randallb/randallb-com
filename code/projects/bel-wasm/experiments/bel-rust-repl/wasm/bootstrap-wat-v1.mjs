// Self-hosting transport (larger explicit bounds, character tags).
// Transport only: JSON data -> values in Wasm memory -> character tree text.
// The module owns source validation and every compilation decision.
import { readFileSync } from "node:fs";

function main() {
  const [binary, input] = process.argv.slice(2);
  if (!binary || !input) {
    throw new Error("usage: self-wat.mjs EMITTER.wasm AST.json");
  }
  const module = new WebAssembly.Module(readFileSync(binary));
  if (WebAssembly.Module.imports(module).length) {
    throw new Error("emitter must have no imports");
  }
  const { memory, join, intern, choose } =
    new WebAssembly.Instance(module).exports;
  const bytes = readFileSync(input);
  if (bytes.length > 8388608) throw new Error("input exceeds 8 MiB");
  let nodes = 0;
  function pair(a, d) {
    if (++nodes > 524288) throw new Error("input node limit reached");
    return join(a, d);
  }
  function name(text) {
    let chars = 0;
    for (const c of Array.from(text).reverse()) {
      chars = pair((c.codePointAt(0) << 3) | 2, chars);
    }
    return intern(chars);
  }
  function encode(value, depth = 0) {
    if (depth > 128) throw new Error("input nesting limit reached");
    if (value === null) return 0;
    if (typeof value === "string") return name(value);
    if (Array.isArray(value)) {
      const values = value.map((v) => encode(v, depth + 1));
      return values.reduceRight((d, a) => pair(a, d), 0);
    }
    if (value && typeof value === "object" && Object.keys(value).length === 1 &&
        typeof value.char === "string" && Array.from(value.char).length === 1) {
      return (value.char.codePointAt(0) << 3) | 2;
    }
    if (
      value && typeof value === "object" && Object.keys(value).length === 1 &&
      Array.isArray(value.pair) && value.pair.length === 2
    ) {
      return pair(
        encode(value.pair[0], depth + 1),
        encode(value.pair[1], depth + 1),
      );
    }
    throw new Error("unsupported JSON value in data transport");
  }
  let result;
  try {
    result = choose(encode(JSON.parse(bytes.toString("utf8"))));
  } catch (error) {
    throw new Error(`Wasm emitter rejected input: ${error.message}`);
  }
  const pending = [result];
  const chunks = [];
  let steps = 0;
  let size = 0;
  while (pending.length) {
    if (++steps > 33554432) throw new Error("output traversal limit reached");
    const value = pending.pop();
    if (value === 0) continue;
    if ((value & 7) === 2) {
      const scalar = value >>> 3;
      if (scalar > 0x10ffff || (scalar >= 0xd800 && scalar <= 0xdfff)) {
        throw new Error("invalid output character");
      }
      const c = String.fromCodePoint(scalar);
      size += Buffer.byteLength(c);
      if (size > 33554432) throw new Error("output exceeds 32 MiB");
      chunks.push(c);
    } else if (
      value >= 4096 && (value & 7) === 0 &&
      value + 8 <= memory.buffer.byteLength
    ) {
      const view = new DataView(memory.buffer);
      pending.push(view.getInt32(value + 4, true), view.getInt32(value, true));
    } else {
      throw new Error("emitter output must be a character tree");
    }
  }
  process.stdout.write(chunks.join(""));
}

try {
  main();
} catch (error) {
  console.error(`emission failed: ${error.message}`);
  process.exitCode = 1;
}
