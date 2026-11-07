// Filesystem-free source-bundle transport. Every semantic decision executes in rbel.
import {readFileSync} from 'node:fs';
import {WasmValues} from '../../../../literate/wasm-values.mjs';
const [binary, requestPath, mode = 'value'] = process.argv.slice(2);
try {
  if (!['value', 'wat', 'selectors'].includes(mode)) throw Error('invalid module transport mode');
  const request = JSON.parse(new TextDecoder('utf-8', {fatal:true}).decode(readFileSync(requestPath)));
  const driver = new WasmValues(new WebAssembly.Module(readFileSync(binary)),{pairCells:4194304});
  try {
    if (mode === 'selectors') {
      for (const selector of request) {
        const answer = driver.choose(driver.encode(selector));
        if (driver.decode(answer) !== 't') throw Error(`example ${selector} failed`);
      }
      process.stdout.write(JSON.stringify({passed:request.length}) + '\n');
    } else {
      const answer = driver.choose(driver.encode(request));
      if (mode === 'value') process.stdout.write(JSON.stringify(driver.decode(answer)) + '\n');
      else driver.withRoots([answer], () => {
        const pending = [answer], chunks = [];
        let steps = 0, bytes = 0;
        while (pending.length) {
          if (++steps > 33554432) throw Error('compiler output traversal limit');
          const value = pending.pop();
          if (value === 0) continue;
          const kind = driver.exports.value_kind(value);
          if (kind === 3) {
            const scalar = String.fromCodePoint(value >>> 3);
            bytes += Buffer.byteLength(scalar);
            if (bytes > 33554432) throw Error('compiler output exceeds 32 MiB');
            chunks.push(scalar);
          } else if (kind === 4) {
            const view = new DataView(driver.exports.memory.buffer);
            pending.push(view.getInt32(value + 4, true), view.getInt32(value, true));
          } else throw Error('compiler output must be a character tree');
        }
        process.stdout.write(chunks.join(''));
      });
    }
  } finally { driver.destroy(); }
} catch (error) {
  console.error(`rbel module frontend: ${error.message}`);
  process.exitCode = 1;
}
