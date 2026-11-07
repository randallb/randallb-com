#!/usr/bin/env python3
"""Verify rooted control flow, errors, closures, rest/apply and proper tail calls.

Fixtures are syntax data transported to the maintained source compiler. Python
never evaluates a fixture. The optional native tail checks disable sibling-call
optimization and keep the ordinary physical Wasm stack cap.
"""
import argparse
import hashlib
import json
from pathlib import Path
import platform
import subprocess

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]


def quote(value):
    return ['quote', value]


def definition(body, params=None, name='test'):
    return ['def', name, ['x'] if params is None else params, body]


def fixtures():
    q, f = quote, definition
    garbage = None
    for _ in range(8):
        garbage = ['join', None, garbage]
    return {
        'identity': [f('x')],
        'join-roots': [f(['join', ['join', q('left'), None], ['join', q('right'), None]])],
        'closure': [f([[['fn', ['a'], ['fn', ['b'], ['join', 'a', 'b']]], q('left')], q(['right'])])],
        'all-rest': [f('args', 'args')],
        'dotted-rest': [f('rest', {'pair': ['first', 'rest']})],
        'apply': [f(['apply', 'join', q(['a', 'b'])])],
        'primitive-shadow': [f([['fn', ['apply'], ['apply', q('a'), q('b')]], 'join'])],
        'primitive-shadow-car': [f(['let', 'car', ['fn', ['a', 'b'], ['join', 'a', 'b']],
                                    ['car', q('a'), q('b')]])],
        'let-shadow': [f(['let', 'x', q('outer'), ['let', 'x', 'x', 'x']])],
        'do-empty': [f(['do'])],
        'multi-body': [['def', 'test', ['x'], q('ignored'), q('value')]],
        'catch-payload': [f(['try', ['err', q('oops')], ['catch', 'e', ['join', 'e', None]]])],
        'catch-type': [f(['try', ['car', q('symbol')], ['catch', 'e', 'e']])],
        'catch-recovery-error': [f(['try', ['try', ['err', q('inner')], ['catch', 'e', ['err', q('outer')]]], ['catch', 'e', 'e']])],
        'catch-arity': [f(['try', [['fn', ['target'], ['target']], ['fn', ['a'], 'a']], ['catch', 'e', 'e']])],
        'catch-apply-tail': [f(['try', ['apply', 'join', q({'pair': ['a', 'b']})], ['catch', 'e', 'e']])],
        'catch-noncallable': [f(['try', ['x'], ['catch', 'e', 'e']])],
        'error-reuse': [f(['if', 'x', ['err', q('failed')], q('recovered')])],
        'direct-tail': [f(['if', 'x', ['test', ['cdr', 'x']], 't'])],
        'mutual-tail': [f(['if', 'x', ['other', ['cdr', 'x']], 't']), f(['if', 'x', ['test', ['cdr', 'x']], 't'], name='other')],
        'closure-tail': [f(['if', 'x', [['if', ['car', 'x'], 'test', 'other'], ['cdr', 'x']], 't']), f(['if', 'x', ['test', ['cdr', 'x']], 't'], name='other')],
        'apply-tail': [f(['if', 'x', ['apply', 'test', ['join', ['cdr', 'x'], None]], 't'])],
        'non-tail-bound': [f(['if', 'x', ['try', ['recurse', 'x'], ['catch', 'e', 'e']], q('recovered')]),
                           f(['join', 'x', ['recurse', 'x']], name='recurse')],
        'intermediate-gc': [f(['join', ['join', q('left'), None], ['burn', 'x']]),
                            f(['if', 'x', ['do', garbage, ['burn', ['cdr', 'x']]], q(['right'])], name='burn')],
        'escaping-capture': [f(['fn', ['y'], ['join', 'x', 'y']])],
        'wide-entry': [f(None, ['x' + str(i) for i in range(64)])],
        'do-discard-handoff': [f(['do', ['build', 'x', None], ['fn', [], None]],
                                ['x', *['p' + str(i) for i in range(63)]]),
                               f(['if', 'xs', ['build', ['cdr', 'xs'], ['join', None, 'acc']], 'acc'],
                                 ['xs', 'acc'], 'build')],
    }


def rejected_fixtures():
    q, f = quote, definition
    return {
        'dead-primitive-arity': [f(['if', 't', 'x', ['car', 'x', 'x']])],
        'dead-named-arity': [f(['if', 't', 'x', ['other']]), f('a', ['a'], 'other')],
        'immediate-lambda-arity': [f([['fn', ['a'], 'a']])],
        'required-rest-prefix-arity': [f(['other']), f('rest', {'pair':['a', 'rest']}, 'other')],
        'primitive-global-redefinition': [f('x', name='car')],
        'dead-duplicate-parameters': [f(['if', 't', 'x', ['fn', ['a', 'a'], 'a']])],
    }


def run(*arguments, **options):
    return subprocess.run(list(map(str, arguments)), check=True, **options)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--compiler', type=Path, default=ROOT / 'target/bel-selfhost/stage1.wasm')
    parser.add_argument('--wat2wasm', type=Path, default=ROOT / 'target/bel-wasm-tools/node_modules/.bin/wat2wasm')
    parser.add_argument('--output', type=Path, default=ROOT / 'target/language-compiler-verification')
    parser.add_argument('--native', action='store_true')
    parser.add_argument('--native-tools', type=Path, default=ROOT / 'target/wabt-native-build')
    parser.add_argument('--runtime', type=Path, default=ROOT / 'target/wabt-native-1.0.39/wasm2c')
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    for name, ast in fixtures().items():
        source, wat, wasm = (output / (name + extension) for extension in ('.json', '.wat', '.wasm'))
        source.write_text(json.dumps(ast))
        with wat.open('wb') as stream:
            run('node', HERE / 'self-wat.mjs', args.compiler.resolve(), source, stdout=stream)
        run(args.wat2wasm.resolve(), wat, '-o', wasm)
    rejected = []
    for name, ast in rejected_fixtures().items():
        source = output / (name + '.json')
        source.write_text(json.dumps(ast))
        result = subprocess.run(['node', str(HERE / 'self-wat.mjs'),
                                 str(args.compiler.resolve()), str(source)], capture_output=True)
        if result.returncode == 0 or result.stdout or not result.stderr:
            raise AssertionError((name, 'invalid known call/definition was accepted', result))
        rejected.append(name)
        print('PASS compile rejection ' + name, flush=True)
    run('node', HERE / 'verify-language-compiler.mjs', output)
    native = {}
    if args.native:
        if platform.system() != 'Darwin' or platform.machine() != 'arm64':
            parser.error('--native currently verifies macOS ARM64 only')
        runtime = args.runtime.resolve()
        for name in ('direct-tail', 'mutual-tail', 'closure-tail', 'apply-tail', 'non-tail-bound'):
            work = output / ('native-' + name)
            work.mkdir(exist_ok=True)
            run(args.native_tools.resolve() / 'wasm2c', output / (name + '.wasm'), '-n', 'bel', '-o', work / 'module.c')
            run('/usr/bin/clang', '-std=c99', '-O2', '-Wall', '-Wextra', '-Werror',
                '-Wno-unused-function', '-Wno-unused-parameter', '-Wno-unused-but-set-variable',
                '-fno-optimize-sibling-calls', '-frounding-math',
                *(['-DRBEL_TEST_NONTAIL=1'] if name == 'non-tail-bound' else []), '-DWASM_RT_USE_MMAP=0',
                '-DWASM_RT_MEMCHECK_BOUNDS_CHECK=1', '-DWASM_RT_MEMCHECK_GUARD_PAGES=0',
                '-DWASM_RT_STACK_DEPTH_COUNT=1', '-DWASM_RT_STACK_EXHAUSTION_HANDLER=0',
                '-DWASM_RT_MAX_CALL_STACK_DEPTH=4096', '-I', runtime, '-I', work,
                HERE / 'language-tail-main.c', work / 'module.c', runtime / 'wasm-rt-impl.c',
                runtime / 'wasm-rt-mem-impl.c', runtime / 'wasm-rt-exceptions-impl.c',
                '-lm', '-pthread', '-o', work / 'run')
            result = run(work / 'run', capture_output=True, text=True)
            native[name] = json.loads(result.stdout)
            print('PASS native ' + name, flush=True)
    report = {'passed': True, 'compiler_sha256': hashlib.sha256(args.compiler.read_bytes()).hexdigest(),
              'sources': {name: hashlib.sha256((HERE / name).read_bytes()).hexdigest()
                          for name in ('self-compiler.bel', 'core-runtime.bel')},
              'wasm': json.loads((output / 'wasm-results.json').read_text()),
              'compile_rejections': rejected, 'native': native, 'native_verified': bool(native)}
    (output / 'results.json').write_text(json.dumps(report, indent=2) + '\n')


if __name__ == '__main__':
    main()
