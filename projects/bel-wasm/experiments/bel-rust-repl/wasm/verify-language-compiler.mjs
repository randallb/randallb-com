// Black-box behavior and resource evidence for source-compiled ABI2 fixtures.
import {readFileSync, writeFileSync} from 'node:fs';
import {resolve, join} from 'node:path';
import assert from 'node:assert/strict';
import {WasmValues} from '../../../literate/wasm-values.mjs';
const directory = resolve(process.argv[2]);
const create = (name, configuration = {}) => new WasmValues(
  new WebAssembly.Module(readFileSync(join(directory, name + '.wasm'))),
  {pairCells:4096, rootSlots:4096, hostSlots:256, symbolSlots:256, callLimit:32, ...configuration});
const results = [];
const cases = [
  ['identity', ['t'], 't'], ['join-roots', [null], [['left'], 'right']],
  ['closure', [null], ['left', 'right']], ['all-rest', ['a', 'b'], ['a', 'b']],
  ['dotted-rest', ['a', 'b', 'c'], ['b', 'c']], ['apply', [null], {pair:['a', 'b']}],
  ['primitive-shadow', [null], {pair:['a', 'b']}],
  ['primitive-shadow-car', [null], {pair:['a', 'b']}], ['let-shadow', [null], 'outer'],
  ['do-empty', [null], null], ['multi-body', [null], 'value'],
  ['catch-payload', [null], ['oops']], ['catch-type', [null], 'type-error'],
  ['catch-recovery-error', [null], 'outer'], ['catch-arity', [null], 'arity-error'],
  ['catch-apply-tail', [null], 'invalid-argument-list'], ['catch-noncallable', [null], 'not-callable'],
];
for (const [name, argumentsValue, expected] of cases) {
  const values = create(name);
  try {
    assert.deepEqual(values.decode(values.allocate('choose_args', values.encode(argumentsValue))), expected);
    assert.equal(values.exports.stat_root_top(), 0);
    results.push({name, passed:true});
  } finally { values.destroy(); }
}
{
  const values = create('error-reuse');
  try {
    for (let i = 0; i < 1000; i++) {
      assert.throws(() => values.allocate('choose_args', values.encode(['t'])), error => {
        assert.equal(error.rbel?.payload, 'failed');
        const id = error.rbel.function, e = values.exports;
        const text = (address, length) => new TextDecoder('utf-8', {fatal:true})
          .decode(new Uint8Array(e.memory.buffer, address, length));
        assert.equal(text(e.diag_source_addr(id), e.diag_source_len(id)), '<ast>');
        assert.equal(text(e.diag_function_addr(id), e.diag_function_len(id)), 'test');
        assert.equal(e.diag_line(id), 0);
        return true;
      });
      assert.equal(values.decode(values.allocate('choose_args', values.encode([null]))), 'recovered');
      assert.equal(values.exports.stat_root_top(), 0);
    }
    results.push({name:'repeated-error-reuse-and-source-diagnostics', passed:true});
  } finally { values.destroy(); }
}
{
  const values = create('non-tail-bound');
  try {
    assert.equal(values.decode(values.allocate('choose_args', values.encode(['t']))), 'call-depth-exhausted');
    assert.equal(values.decode(values.allocate('choose_args', values.encode([null]))), 'recovered');
    assert.equal(values.exports.stat_root_top(), 0);
    assert.equal(values.exports.stat_max_calls(), 32);
    results.push({name:'non-tail-bound-is-recoverable', passed:true});
  } finally { values.destroy(); }
}
{
  const values = create('non-tail-bound', {rootSlots:32768, callLimit:512});
  try {
    assert.equal(values.decode(values.allocate('choose_args', values.encode(['t']))), 'call-depth-exhausted');
    assert.equal(values.decode(values.allocate('choose_args', values.encode([null]))), 'recovered');
    assert.equal(values.exports.stat_root_top(), 0);
    assert.equal(values.exports.stat_max_calls(), 512);
    results.push({name:'maximum-accepted-call-limit-is-recoverable', passed:true});
  } finally { values.destroy(); }
}
{
  const values = create('intermediate-gc');
  try {
    let list = 0;
    for (let i = 0; i < 600; i++) list = values.allocate('join', 0, list);
    assert.deepEqual(values.decode(values.allocate('choose', list)), [['left'], 'right']);
    assert(values.exports.stat_collections() > 0);
    assert(values.exports.stat_reclaimed_cells() > 0);
    assert.equal(values.exports.stat_root_top(), 0);
    results.push({name:'earlier-sibling-survives-later-allocation-and-collection', passed:true,
      collections:values.exports.stat_collections()});
  } finally { values.destroy(); }
}
{
  const values = create('escaping-capture');
  try {
    const closure = values.allocate('choose_args', values.encode(['captured']));
    const root = values.retain(closure);
    try {
      for (let i = 0; i < 20000; i++) values.allocate('join', 0, 0);
      assert.deepEqual(values.decode(values.allocate('invoke_value', closure, values.encode([['tail']]))), ['captured', 'tail']);
      assert(values.exports.stat_collections() > 0);
      assert.equal(values.exports.stat_root_top(), 0);
      results.push({name:'escaping-capture-survives-host-retention-and-collections', passed:true});
    } finally { values.release(root); }
  } finally { values.destroy(); }
}
{
  const values = create('do-discard-handoff', {pairCells:1024});
  try {
    const args = [Array(300).fill(null), ...Array(63).fill(null)];
    const closure = values.allocate('choose_args', values.encode(args));
    const retained = values.retain(closure);
    try {
      assert.equal(values.exports.collect(), 0);
      assert.equal(values.allocate('invoke_value', closure, 0), 0);
      assert.equal(values.exports.stat_root_top(), 0);
      results.push({name:'do-discard-releases-output-handoff-before-capturing-environment', passed:true});
    } finally { values.release(retained); }
  } finally { values.destroy(); }
}
{
  for (const rootSlots of [16, 32, 64, 65]) {
    const values = create('wide-entry', {rootSlots});
    try {
      assert.equal(values.exports.choose(...Array(64).fill(0)), 0);
      assert.equal(values.exports.completion_status(), 2);
      assert.equal(values.decode(values.exports.error_value()), 'root-exhausted');
      assert.equal(values.exports.stat_root_top(), 0);
      const args = values.encode(Array(64).fill(null));
      assert.equal(values.exports.choose_args(args), 0);
      assert.equal(values.exports.completion_status(), 2);
      assert.equal(values.decode(values.exports.error_value()), 'root-exhausted');
      assert.equal(values.exports.stat_root_top(), 0);
      assert.deepEqual(values.decode(values.allocate('join', 0, 0)), [null]);
    } finally { values.destroy(); }
  }
  const values = create('wide-entry', {rootSlots:256});
  try {
    assert.equal(values.allocate('choose', ...Array(64).fill(0)), 0);
    assert.equal(values.exports.stat_root_top(), 0);
  } finally { values.destroy(); }
  results.push({name:'public-positional-and-generic-root-exhaustion-publish-THROW-and-nil', passed:true});
}
for (const name of ['direct-tail', 'mutual-tail', 'closure-tail', 'apply-tail']) {
  const values = create(name, {pairCells:1200000, callLimit:8});
  try {
    let list = 0;
    for (let i = 0; i < 1000000; i++) list = values.allocate('join', 0, list);
    assert.equal(values.allocate('choose', list), 1);
    const e = values.exports;
    assert.equal(e.stat_root_top(), 0);
    assert(e.stat_max_calls() <= 3);
    assert(e.stat_tail_transfers() >= 1000000);
    assert(e.stat_collections() > 0);
    results.push({name:'million-step-' + name, passed:true, maximum_calls:e.stat_max_calls(),
      maximum_roots:e.stat_max_roots(), tail_transfers:e.stat_tail_transfers(), collections:e.stat_collections()});
  } finally { values.destroy(); }
}
for (const result of results) console.log('PASS ' + result.name);
writeFileSync(join(directory, 'wasm-results.json'), JSON.stringify(results, null, 2) + '\n');
