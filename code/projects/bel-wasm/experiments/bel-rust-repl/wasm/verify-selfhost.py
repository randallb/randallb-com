#!/usr/bin/env python3
"""Bootstrap once, then verify self-compilation and program behavior without Rust."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
OUT = ROOT / 'target/bel-selfhost'
TOOLS = ROOT / 'target/bel-wasm-tools/node_modules/.bin'
SOURCE = HERE / 'self-compiler.bel'


def run(*args, cwd=None, success=True):
    result = subprocess.run(list(map(str, args)), cwd=cwd, capture_output=True, text=True)
    if (result.returncode == 0) != success:
        raise AssertionError(f'{args}\n{result.stdout[-2000:]}\n{result.stderr[-4000:]}')
    return result


def assemble(wat, binary):
    run(TOOLS / 'wat2wasm', wat, '-o', binary)
    run(TOOLS / 'wasm-validate', binary)


def isolated(compiler, ast, name):
    """The compiler, data and generic adapter are the only project files present."""
    with tempfile.TemporaryDirectory(prefix='bel-selfhost-isolated-') as directory:
        work = Path(directory)
        shutil.copyfile(compiler, work / 'compiler.wasm')
        shutil.copyfile(HERE / 'self-wat.mjs', work / 'transport.mjs')
        shutil.copyfile(ROOT / 'literate/wasm-values.mjs', work / 'wasm-values.mjs')
        (work / 'input.json').write_text(json.dumps(ast))
        result = run('node', 'transport.mjs', 'compiler.wasm', 'input.json', cwd=work)
        wat, binary = OUT / f'{name}.wat', OUT / f'{name}.wasm'
        wat.write_text(result.stdout)
        assemble(wat, binary)
        return binary


def inspect(binary, script):
    prefix = '''import {readFileSync} from 'node:fs';
import assert from 'node:assert/strict';
const m = new WebAssembly.Module(readFileSync(process.argv[1]));
assert.deepEqual(WebAssembly.Module.imports(m), []);
const e = new WebAssembly.Instance(m).exports;
assert.equal(e.rbel_abi_version(),2);
assert.equal(e.runtime_init(2097152,262144,65536,65536,512),0);
'''
    run('node', '--input-type=module', '-e', prefix + script, binary)


def q(x):
    return ['quote', x]


def program(name, params, body):
    return [['def', name, params, body]]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--skip-bootstrap', action='store_true', help='reuse stage0 and reader data')
    args = parser.parse_args()
    OUT.mkdir(parents=True, exist_ok=True)
    if not args.skip_bootstrap:
        import importlib.util
        spec = importlib.util.spec_from_file_location('rbel_language_bootstrap',HERE / 'bootstrap-language.py')
        bootstrap_module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(bootstrap_module)
        bootstrap_module.bootstrap()
    ast = json.loads((OUT / 'compiler.ast.json').read_text())
    stages = [OUT / 'stage0.wasm']
    for i in range(1, 4):
        stages.append(isolated(stages[-1], ast, f'stage{i}'))
        assert stages[-1].read_bytes() == stages[0].read_bytes(), f'stage{i} binary differs'
        assert stages[-1].with_suffix('.wat').read_bytes() == stages[0].with_suffix('.wat').read_bytes()
    print('PASS: three Wasm successor generations match bootstrap WAT and binary', flush=True)

    cases = [
        ('choose', program('test', ['x'], ['if', 'x', 't', None]), [0, 1]),
        ('changed', program('test', ['x'], ['if', 'x', None, None]), [0, 0]),
        ('nested', program('test', ['x'], ['if', ['if', 'x', None, 't'], 't', 'x']), [1, 1]),
        ('forward', [['def', 'test', ['x'], ['later', 'x']],
                     ['def', 'later', ['x'], ['if', 'x', None, 't']]], [1, 0]),
        ('multi', [['def', 'test', ['x'], ['later', 'x', 't']],
                   ['def', 'later', ['a', 'b'], ['if', 'a', 'b', None]]], [0, 1]),
        ('quote-identity', [['def', 'test', ['x'], ['id', ['datum'], ['datum']]],
                            ['def', 'datum', [], q(['a', {'char': 'z'}])]], [1, 1]),
        ('distinct-literals', program('test', ['x'], ['id', q(['a']), q(['a'])]), [0, 0]),
        ('types', program('test', ['x'], ['id', ['type', 'x'], q('symbol')]), [1, 1]),
        ('symbol-names', program('test', ['x'], ['car', ['nom', q('alpha')]]), [778, 778]),
        ('text', program('test', ['x'], ['car', [{'char': 'Q'}, {'char': '\n'}]]), [650, 650]),
        ('nil-access', program('test', ['x'], ['car', None]), [0, 0]),
    ]
    for name, source, expected in cases:
        binaries = [isolated(stage, source, f'{name}-stage{i}') for i, stage in enumerate(stages[1:], 1)]
        assert len({b.read_bytes() for b in binaries}) == 1
        for binary in binaries:
            inspect(binary, f'assert.deepEqual([e.choose(0),e.choose(1)],{json.dumps(expected)});')
    assert (OUT / 'choose-stage1.wasm').read_bytes() != (OUT / 'changed-stage1.wasm').read_bytes()
    copy = program('copy', ['x'], ['if', ['id', ['type', 'x'], q('pair')],
                                 ['join', ['copy', ['car', 'x']], ['copy', ['cdr', 'x']]], 'x'])
    for i, stage in enumerate(stages[1:], 1):
        binary = isolated(stage, copy, f'recursion-stage{i}')
        inspect(binary, '''const input=e.join(1,e.join(0,1)); const result=e.choose(input);
assert.notEqual(input,result); const v=new DataView(e.memory.buffer);
assert.equal(v.getInt32(result,true),1); const tail=v.getInt32(result+4,true);
assert.equal(v.getInt32(tail,true),0); assert.equal(v.getInt32(tail+4,true),1);''')
    print(f'PASS: {len(cases) + 1} program fixtures across all three successors', flush=True)

    # Mutate the compiler's export-name literal, not a precomputed output fixture.
    def mutate(x):
        if isinstance(x, list):
            if x and all(isinstance(c, dict) and set(c) == {'char'} for c in x):
                text = ''.join(c['char'] for c in x)
                return [{'char': c} for c in text.replace('(export "choose")', '(export "picked")')]
            return [mutate(v) for v in x]
        return x
    mutant = mutate(ast)
    assert mutant != ast
    mutated = isolated(stages[1], mutant, 'mutated-compiler')
    product = isolated(mutated, cases[0][1], 'mutated-compiler-product')
    inspect(product, 'assert.equal(e.choose,undefined); assert.deepEqual([e.picked(0),e.picked(1)],[0,1]);')
    print('PASS: changing compiler source changes generated program exports', flush=True)

    invalid = [None, 't', [], ['bad'],
        [['fn', 'x', [], None]], program('t', [], None), program('nil', [], None),
        program('let', [], None), program('apply', [], None), program('bad name', [], None),
        program('x)(func', [], None), program('🙂', [], None), program('', [], None),
        program('test', ['x','x'], 'x'), program('test', ['if'], 'if'),
        program('test', ['fn'], None), program('test', {'pair':['x',{'char':'z'}]}, None),
        program('test', ['x'], 'missing'), program('test', ['x'], ['missing', 'x']),
        program('test', ['x'], ['car','x','x']), program('test', ['x'], ['join','x']),
        program('test', ['x'], ['fn', ['a','a'], 'a']), program('test', ['x'], ['quote','t',None]),
        program('test', ['x'], ['if', 't', None, ['unsupported', 'x']]),
        program('test', ['x'], {'pair':['if','tail']}),
        [['def','test',['x'],['other']], ['def','other',['y'],'y']],
        [['def','test',[],None], ['def','test',[],None]],
    ]
    bad = OUT / 'invalid.json'
    output = OUT / 'recovery.wasm'
    for stage in stages[1:]:
        for source in invalid:
            bad.write_text(json.dumps(source))
            output.write_bytes(b'stale')
            output.with_suffix('.wat').write_text('stale')
            result = run(sys.executable, HERE / 'self-compile.py', stage, bad, output, success=False)
            assert 'Wasm emitter rejected input' in result.stderr
            assert not output.exists() and not output.with_suffix('.wat').exists()
        bad.write_text('{')
        run(sys.executable, HERE / 'self-compile.py', stage, bad, output, success=False)
        assert not output.exists() and not output.with_suffix('.wat').exists()
        bad.write_text(json.dumps(cases[0][1]))
        run(sys.executable, HERE / 'self-compile.py', stage, bad, output,
            '--wat2wasm', OUT / 'missing-assembler', success=False)
        assert not output.exists() and not output.with_suffix('.wat').exists()
        run(sys.executable, HERE / 'self-compile.py', stage, bad, output)
        inspect(output, 'assert.deepEqual([e.choose(0),e.choose(1)],[0,1]);')
        before = stage.read_bytes()
        run(sys.executable, HERE / 'self-compile.py', stage, bad, stage, success=False)
        assert stage.read_bytes() == before
    print(f'PASS: {len(invalid)} source rejections per successor; cleanup, recovery, overwrite protection', flush=True)
    manifest = {p.name: {'sha256': hashlib.sha256(p.read_bytes()).hexdigest(), 'bytes': p.stat().st_size}
                for p in stages}
    (OUT / 'selfhost-results.json').write_text(json.dumps(manifest, indent=2) + '\n')
    print('PASS: self-hosting verified; Rust used only before successor generation', flush=True)


if __name__ == '__main__':
    main()
