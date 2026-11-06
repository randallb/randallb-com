#!/usr/bin/env python3
"""Compile unchanged literate foundation and check its examples as native code."""
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import sys
import tempfile

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
OUT = ROOT / 'target/bel-foundation-native'
sys.path.insert(0, str(ROOT / 'literate'))
from check import check_pin, extract


def normalize(value):
    if isinstance(value, list):
        tail = None
        for head in reversed(value):
            tail = {'pair': [normalize(head), tail]}
        return tail
    if isinstance(value, dict) and 'pair' in value:
        return {'pair': [normalize(x) for x in value['pair']]}
    return value


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    chapter = (ROOT / 'literate/foundation.rbel.md').read_bytes().decode()
    definitions, rows, count = extract(chapter)
    check_pin(definitions)
    extra = [
        ('captured-return', "(((fn (outer) (fn (inner) (join outer inner))) 'a) 'b)", '(a . b)'),
        ('shadow-capture', "(((fn (x) (fn (x) x)) 'outer) 'inner)", 'inner'),
        ('returned-closure', "((fn (f) (f 'b)) ((fn (x) (fn (y) (join x y))) 'a))", '(a . b)'),
        ('distinct-closures', '(id (fn (x) x) (fn (x) x))', 'nil'),
        ('named-identity', '(id no no)', 't'),
        ('primitive-identity', '(id car car)', 't'),
        ('shadow-primitive', "((fn (car) (car 'a)) (fn (x) (join x 'b)))", '(a . b)'),
        ('shadow-named', "((fn (no) (no 'a)) (fn (x) x))", 'a'),
        ('captured-function', "(((fn (f) (fn (x) (f x))) car) '(a b))", 'a'),
        ('zero-arity', "((fn () 'zero))", 'zero'),
        ('dynamic-two-args', "((fn (f) (f 'a 'b)) join)", '(a . b)'),
        ('literal-identity', "((fn (f) (id (f) (f))) (fn () '(a)))", 't'),
        ('closure-truth', "(if (fn () nil) 'yes 'no)", 'yes'),
        ('transitive-capture', "((((fn (a) (fn (b) (fn (c) (join a (join b c))))) 'a) 'b) 'c)", '(a b . c)'),
        ('closure-type', '(type (fn (x) x))', 'pair'),
    ]
    traps = [
        ('too-many', "((fn (f) (f 'x 'y)) car)"),
        ('too-few', '((fn (f) (f)) no)'),
        ('non-function', "((fn (f) (f 'a)) 'not-function)"),
        ('closure-arity', '((fn (f) (f)) (fn (x) x))'),
    ]
    rows += ''.join(f'({name}\n{expression}\n{expected}\n)\n' for name, expression, expected in extra)
    row_path = OUT / 'examples.bel'
    row_path.write_text(rows)
    reader = ROOT / 'target/debug/bel-read-json'
    parsed = json.loads(subprocess.check_output([str(reader), str(row_path)]))
    # Recover exact expressions from extraction rows by reading each chapter fence;
    # expected data above comes independently from the complete row parse.
    import re
    expressions = []
    for name, body in re.findall(r'```rbel example=([^\n]+)\n(.*?)```', chapter, re.S):
        expression = re.split(r'(?m)^=>$', body)[0].strip()
        expressions.append((name, expression))
    expressions += [(name, expression) for name, expression, _ in extra]
    all_expressions = expressions + traps
    dispatch = '(def chapter-main (selector) (if\n' + ''.join(
        f"  (id selector '{name}) (example-{name})\n" for name, _ in all_expressions
    ) + "  (err 'unknown-selector)))\n"
    source = OUT / 'chapter.bel'
    source.write_text(dispatch + definitions.decode() + '\n' + ''.join(
        f'(def example-{name} ()\n {expression})\n' for name, expression in all_expressions))
    spec = importlib.util.spec_from_file_location('native_builder', HERE / 'native/build-native.py')
    builder = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(builder)
    binary = OUT / 'rbel-foundation-check'
    builder.build(source, binary, ROOT / 'target/bel-selfhost/stage1.wasm', reader,
                  ROOT / 'target/bel-wasm-tools/node_modules/.bin',
                  ROOT / 'target/wabt-native-1.0.39/wasm2c',
                  artifacts=OUT / 'build', adapter=HERE / 'native/value-main.c')
    assert len(parsed) == count + len(extra)
    with tempfile.TemporaryDirectory(prefix='rbel-foundation-isolated-') as directory:
        executable = Path(directory) / 'check'
        shutil.copy2(binary, executable)
        def execute(name):
            return subprocess.run([str(executable), name], capture_output=True, text=True,
                                  cwd=directory, env={'PATH': '', 'LC_ALL': 'C'}, timeout=30)
        for name, _, expected in parsed:
            result = execute(name)
            assert result.returncode == 0, (name, result.stderr)
            assert not result.stderr, (name, result.stderr)
            actual = json.loads(result.stdout)
            assert actual == normalize(expected), (name, actual, normalize(expected))
            print(f'PASS native {name}', flush=True)
        for name, _ in traps:
            result = execute(name)
            assert result.returncode == 1 and 'program trapped' in result.stderr, (name, result)
            assert not result.stdout, (name, result.stdout)
    invalid = [
        '(def test (x) (fn (a a) a))',
        '(def test (x) (fn rest rest))',
        '(def test (x) (fn (a) a a))',
        '(def test (x) (fn (a) missing))',
        '(def test (x) (if t x (fn (a) missing)))',
        '(def test (x) (fn (if) if))',
    ]
    for index, text in enumerate(invalid):
        bad_source = OUT / 'invalid.bel'
        bad_data = OUT / 'invalid.json'
        bad_source.write_text(text)
        bad_data.write_bytes(subprocess.check_output([str(reader), str(bad_source)]))
        result = subprocess.run(['node', str(HERE / 'self-wat.mjs'),
                                 str(ROOT / 'target/bel-selfhost/stage1.wasm'), str(bad_data)],
                                capture_output=True, text=True, timeout=30)
        assert result.returncode != 0 and 'rejected input' in result.stderr, (index, result)
        assert not result.stdout, (index, result.stdout)
    libraries = []
    if sys.platform == 'darwin':
        description = subprocess.check_output(['/usr/bin/file', binary], text=True)
        if platform.machine() == 'arm64':
            assert 'arm64' in description, description
        linkage = subprocess.check_output(['/usr/bin/otool', '-L', binary], text=True)
        libraries = [line.strip().split(' (')[0] for line in linkage.splitlines()[1:]]
        assert all(name.startswith(('/usr/lib/', '/System/Library/')) for name in libraries), linkage
    receipt = {'foundation_examples': count, 'closure_cases': len(extra), 'runtime_traps': len(traps),
               'compiler_rejections': len(invalid),
               'empty_path_execution': True, 'platform': platform.platform(),
               'sha256': hashlib.sha256(binary.read_bytes()).hexdigest(),
               'bytes': binary.stat().st_size, 'dynamic_libraries': libraries,
               'foundation_sha256': hashlib.sha256(definitions).hexdigest()}
    (OUT / 'results.json').write_text(json.dumps(receipt, indent=2) + '\n')
    print(f'PASS: {count} unchanged chapter examples, {len(extra)} closure cases, '
          f'{len(traps)} dynamic-call traps as isolated native code; {len(invalid)} compiler rejections', flush=True)


if __name__ == '__main__':
    main()
