// Benchmark transport only: all measured computation executes in source-compiled rbel.
import {readFileSync} from 'node:fs';
import {performance} from 'node:perf_hooks';
import {cpus, release} from 'node:os';
import assert from 'node:assert/strict';
import {WasmValues} from '../literate/wasm-values.mjs';
const request = JSON.parse(readFileSync(process.argv[2], 'utf8'));
const binary = readFileSync(request.wasm);
const started = performance.now();
const module = new WebAssembly.Module(binary);
const moduleMs = performance.now() - started;
const names = ['live_cells','collections','reclaimed_cells','root_top','max_roots',
  'host_roots','live_symbols','max_calls','tail_transfers','total_bytes'];
const counters = e => Object.fromEntries(names.map(n => [n, e['stat_' + n]() >>> 0]));
const input = name => name === 'list-reverse' ? Array.from({length:1024}, (_,i) => i%2 ? 'right' : 'left')
  : ['tail-direct','tail-apply','allocation-churn'].includes(name) ? Array(10000).fill(null) : null;
const expected = name => name === 'list-reverse' ? input(name).reverse()
  : name === 'exact-arithmetic' ? [...'1000000000000000000000000000000'].map(char => ({char}))
  : name === 'utf8-roundtrip' ? [...'hé😉'].map(char => ({char})) : 'done';
const results = [];
for (const name of request.cases) {
  const samples = [];
  for (let sample = -request.warmup; sample < request.samples; sample++) {
    let t = performance.now();
    const values = new WasmValues(module, request.configuration);
    const initializeMs = performance.now() - t;
    let held;
    try {
      t = performance.now();
      const args = values.encode([name,input(name)]);
      held = values.retain(args);
      const encodeMs = performance.now() - t;
      assert.equal(values.exports.collect(),0);
      values.checkCompletion();
      const before = counters(values.exports);
      let elapsedNs = 0, validationMs = 0;
      for (let i = 0; i < request.iterations; i++) {
        t = performance.now();
        const result = values.exports.choose_args(args);
        elapsedNs += (performance.now() - t) * 1e6;
        // Status/decoding/assertions deliberately outside the execution timer.
        values.checkCompletion();
        t = performance.now();
        assert.deepEqual(values.decodeRooted(result,0,{nodes:0},true), expected(name), name);
        validationMs += performance.now() - t;
      }
      const after = counters(values.exports);
      assert.equal(after.root_top,0);
      assert.equal(after.host_roots,before.host_roots);
      if (sample >= 0) samples.push({elapsed_ns:elapsedNs,iterations:request.iterations,
        initialize_ms:initializeMs,encode_ms:encodeMs,validation_ms:validationMs,
        counters_before:before,counters_after:after});
    } finally {
      if (held) assert.equal(values.release(held),true);
      assert.equal(values.exports.stat_host_roots(),0);
      values.destroy();
    }
  }
  results.push({name,validation:true,samples});
}
console.log(JSON.stringify({backend:'wasm',engine:process.version,platform:process.platform,
  arch:process.arch,os_release:release(),cpu:cpus()[0]?.model,logical_cpus:cpus().length,
  module_compile_ms:moduleMs,results}));
