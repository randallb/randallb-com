// Self-hosting transport (larger explicit bounds, character tags).
// Transport only: JSON data -> values in Wasm memory -> character tree text.
// The module owns source validation and every compilation decision.
import { readFileSync } from "node:fs";
import {WasmValues} from "./wasm-values.mjs";

function main() {
  const [binary, input] = process.argv.slice(2);
  if (!binary || !input) {
    throw new Error("usage: self-wat.mjs EMITTER.wasm AST.json");
  }
  const module = new WebAssembly.Module(readFileSync(binary));
  if (WebAssembly.Module.imports(module).length) {
    throw new Error("emitter must have no imports");
  }
  const driver = new WasmValues(module,{pairCells:4194304});
  const {memory} = driver.exports;
  const bytes = readFileSync(input);
  if (bytes.length > 8388608) throw new Error("input exceeds 8 MiB");
  let result;
  try {
    result = driver.choose(driver.encode(JSON.parse(new TextDecoder('utf-8',{fatal:true}).decode(bytes))));
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
      driver.pairp(value)
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
