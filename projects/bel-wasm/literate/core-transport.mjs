// Bootstrap transport only. Source parsing and compilation both execute in rbel.
import {readFileSync} from 'node:fs';
import {WasmValues} from './wasm-values.mjs';
const [binary, operation, path] = process.argv.slice(2);
try {
  if (!['read', 'compile'].includes(operation)) throw Error('unknown core operation');
  const bytes = readFileSync(path);
  if (bytes.length > 131072) throw Error('source exceeds 128 KiB');
  const source = new TextDecoder('utf-8', {fatal:true}).decode(bytes);
  const module = new WebAssembly.Module(readFileSync(binary));
  const driver = new WasmValues(module);
  const request = driver.encode([operation, Array.from(source, char => ({char}))]);
  const answer = driver.exports.choose(request);
  if (operation === 'read') {
    const pairs = value => {
      if (Array.isArray(value)) return value.map(pairs).reduceRight((tail, head) => ({pair:[head,tail]}), null);
      if (value && typeof value === 'object' && value.pair) return {pair:value.pair.map(pairs)};
      return value;
    };
    process.stdout.write(JSON.stringify(pairs(driver.decode(answer))) + '\n');
  } else {
    const pending = [answer], chunks = [];
    let steps = 0, bytes = 0;
    while (pending.length) {
      if (++steps > 33554432) throw Error('compiler output traversal limit');
      const value = pending.pop();
      if (value === 0) continue;
      if ((value & 7) === 2) {
        const cp = value >>> 3;
        if (cp > 0x10ffff || cp >= 0xd800 && cp <= 0xdfff) throw Error('invalid output character');
        const char = String.fromCodePoint(cp);
        bytes += Buffer.byteLength(char);
        if (bytes > 8388608) throw Error('compiler output exceeds 8 MiB');
        chunks.push(char);
      } else if (driver.pairp(value)) {
        const view = new DataView(driver.exports.memory.buffer);
        pending.push(view.getInt32(value + 4, true), view.getInt32(value, true));
      } else throw Error('compiler output must be a character tree');
    }
    process.stdout.write(chunks.join(''));
  }
} catch (error) {
  console.error(`rbel core: ${error.message}`);
  process.exitCode = 1;
}
