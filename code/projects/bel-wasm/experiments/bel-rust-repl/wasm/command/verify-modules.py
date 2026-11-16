#!/usr/bin/env python3
"""Check real source-unit linking through both native and portable semantic frontend.

Hosts perform only source snapshots/fence extraction/graph discovery. Namespace,
visibility, lexical renaming, and located parsing execute in modules.bel.
"""
import argparse
import json
from pathlib import Path
import subprocess
import tempfile
from module_loader import load_bundle, chars, text

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[3]
OUTPUT = ROOT / 'target/module-verification'


def document(definitions, examples):
    return ('```rbel definition=module-source\n' + definitions + '\n```\n' +
            '\n'.join(f'```rbel example={name}\n{expression}\n=>\n{expected}\n```\n'
                      for name, expression, expected in examples))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--core', type=Path, default=ROOT / 'target/rbel-core-bootstrap/core.wasm')
    parser.add_argument('--rbel', type=Path, default=ROOT / 'target/rbel-command/bin/rbel')
    parser.add_argument('--portable-only', action='store_true')
    parser.add_argument('--wat2wasm', type=Path, default=ROOT / 'target/bel-wasm-tools/node_modules/.bin/wat2wasm')
    args = parser.parse_args()
    OUTPUT.mkdir(parents=True, exist_ok=True)
    checks = []
    (OUTPUT / 'results.json').write_text(json.dumps({'passed': False, 'checks': []})+'\n')

    with tempfile.TemporaryDirectory(prefix='module acceptance ', dir=OUTPUT) as directory:
        work = Path(directory)
        request = work / 'request.json'
        def query(value, mode='value', binary=None, success=True):
            request.write_text(json.dumps(value, ensure_ascii=False))
            result = subprocess.run(['node', str(HERE / 'module-query.mjs'), str(binary or args.core), str(request), mode],
                                    text=True, capture_output=True, timeout=180)
            if (result.returncode == 0) != success:
                raise AssertionError((value[0] if value else value, result.returncode, result.stderr[-4000:]))
            if not success:
                return result.stderr
            return result.stdout if mode == 'wat' else json.loads(result.stdout)
        def write(name, body):
            path = work / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(body)
            return path
        def verify(label, root):
            bundle = load_bundle([root], query)
            wat = query(['check-bundle', bundle], 'wat')
            output = work / (label + '.wat')
            binary = output.with_suffix('.wasm')
            output.write_text(wat)
            subprocess.run([str(args.wat2wasm), str(output), '-o', str(binary)], check=True, capture_output=True)
            query([f'mod-example-{i}' for i in range(len(bundle[4]))], 'selectors', binary)
            if not args.portable_only:
                result = subprocess.run([str(args.rbel), 'check', str(root)], capture_output=True, text=True, timeout=180)
                if result.returncode:
                    raise AssertionError((label, result.stderr[-4000:]))
                native = work / (label + '-native.wat')
                result = subprocess.run([str(args.rbel), 'emit', str(root), '-o', str(native)], capture_output=True, text=True, timeout=180)
                if result.returncode:
                    raise AssertionError((label, result.stderr[-4000:]))
                if native.read_text() != query(['compile-bundle', bundle], 'wat'):
                    raise AssertionError((label, 'native/portable linked output differs'))
            checks.append(label)
            print('PASS', label, flush=True)
            return bundle

        write('a.rbel', '(module a)\n(export run)\n(def helper (x) (join \'a x))\n(def run (x) (helper x))\n')
        write('b.rbel', '(module b)\n(export run)\n(def helper (x) (join \'b x))\n(def run (x) (helper x))\n')
        root = write('main.rbel.md', document(
            '(import "a.rbel" as a)\n(import "b.rbel" as b)\n(def main (x) (join (a/run x) (b/run x)))', [
                ('private-helper-names', "(main '(x))", '((a x) b x)'),
                ('qualified-quote', '(id \'a/run (sym "a/run"))', 't'),
                ('alias-independent-binding', "(let a 'ignored (a/run nil))", '(a)'),
            ]))
        main_bundle = verify('private-names-and-qualified-quote', root)
        write('shared.rbel', '(module shared)\n(export identify)\n(def identify (x) x)\n')
        write('left.rbel', '(module left)\n(import "shared.rbel" as shared)\n(export get)\n(def get () shared/identify)\n')
        write('right.rbel', '(module right)\n(import "shared.rbel" as shared)\n(export get)\n(def get () shared/identify)\n')
        root = write('diamond.rbel.md', document(
            '(import "left.rbel" as left)\n(import "right.rbel" as right)\n(def main () (id (left/get) (right/get)))',
            [('diamond-function-identity', '(main)', 't')]))
        verify('diamond-identity', root)
        write('flat.rbel', '(def helper (x) (join \'global x))\n')
        root = write('scope.rbel.md', document(
            '(import "flat.rbel")\n(def main (x) (helper x))', [
                ('legacy-import', '(main nil)', '(global)'),
                ('let-initializer-outer', '(let helper (helper nil) helper)', '(global)'),
                ('named-shadow-different-arity', "(let helper (fn () 'local) (helper))", 'local'),
                ('primitive-shadow-different-arity', "(let car (fn () 'shadow) (car))", 'shadow'),
                ('let-body-inner', "(let helper (fn (x) (join 'local x)) (helper nil))", '(local)'),
                ('fn-parameter-shadow', "((fn (helper) (helper nil)) (fn (x) (join 'local x)))", '(local)'),
                ('rest-binding-shadow', "((fn helper helper) 'local)", '(local)'),
                ('catch-binding-shadow', "(try (err 'raised) (catch helper helper))", 'raised'),
            ]))
        verify('legacy-and-lexical-scopes', root)
        root = write('hygiene.rbel.md', document(
            '(import "flat.rbel")\n(def main (sc-global-0 sc-global-1) (helper sc-global-1))', [
                ('parameter-hygiene', "(main 'ignored '(ok))", '(global ok)'),
                ('let-global-name-hygiene', "(let sc-global-1 'poison (helper nil))", '(global)'),
                ('parameter-global-name-hygiene', "((fn (sc-global-1) (helper nil)) 'poison)", '(global)'),
                ('catch-global-name-hygiene', "(try (err 'poison) (catch sc-global-1 (helper nil)))", '(global)'),
                ('nested-generated-names', "(let sc-local-0 'outer ((fn (sc-global-1) (helper (join sc-local-0 nil))) 'poison))", '(global outer)'),
                ('catch-scope-hygiene', "(let sc-local-0 'outer (join (try (err 'inner) (catch sc-local-0 sc-local-0)) (join sc-local-0 nil)))", '(inner outer)'),
                ('let-initializer-hygiene', "(let sc-global-1 'outer (let sc-global-1 sc-global-1 sc-global-1))", 'outer'),
                ('rest-hygiene', "(let sc-local-0 'outer ((fn sc-global-1 (join sc-local-0 sc-global-1)) 'a 'b))", '(outer a b)'),
            ]))
        verify('lexical-and-global-alpha-hygiene', root)
        root = write('root-module.rbel.md', document(
            '(module root)\n(export)\n(def main (x) x)', [('private-entry', "(main 'ok)", 'ok')]))
        verify('private-module-entry', root)
        write('math.rbel', '(module math)\n(export + /)\n(def + (a b) (join a b))\n(def / (a b) (join b a))\n')
        root = write('operators.rbel.md', document(
            '(import "math.rbel" as math)\n(def main (x) x)', [
                ('qualified-plus', "(math/+ 'a 'b)", '(a . b)'),
                ('qualified-division', "(math// 'a 'b)", '(b . a)'),
                ('fraction-remains-number', '1/3', '1/3'),
                ('unicode-string', '"é世界🦉"', '"é世界🦉"'),
                ('scalar-escape-nul', '(char-code \\u{0})', '0'),
                ('scalar-escape-max', '(char-code \\u{10FFFF})', '1114111'),
                ('actual-unicode-character', '(char-code \\é)', '233'),
                ('string-backslash-rule', '"\\u{41}"', '"u{41}"'),
            ]))
        verify('qualified-arithmetic-and-unicode', root)

        root = write('many-definitions.rbel.md', document(
            '\n'.join(f'(def function-{i} (x) x)' for i in range(600)),
            [('many-top-level-forms', "(function-0 'ok)", 'ok')]))
        verify('six-hundred-located-definitions', root)

        # Located source parsing counts LF once in CRLF and skips original comments.
        located = ['unit-0', chars('located.rbel.md'), [[chars('40'), chars('; comment\r\n\r\n(def main (x) x)\n')]]]
        rows = query(['read-unit', located])
        if text(rows[0][1][1]) != '42':
            raise AssertionError(('located-form-line', rows))
        checks.append('located-form-line')
        control_unit = ['unit-0', chars('control.rbel'), [[chars('42'), chars('(def main (x) "' + chr(1) + '")')]]]
        diagnostic = query(['read-unit', control_unit], success=False)
        if 'control.rbel:42' not in diagnostic:
            raise AssertionError(('bundle-raw-control', diagnostic))
        raw_control = write('raw-control.rbel', '(def main (x) "' + chr(1) + '")')
        try:
            load_bundle([raw_control], query)
        except ValueError as error:
            if 'control' not in str(error): raise
        else:
            raise AssertionError('file raw source control accepted')
        checks.append('bundle-and-file-raw-controls')

        bad_sources = [
            ('private-export', '(import "a.rbel" as a)\n(def main (x) (a/helper x))', 'private'),
            ('duplicate-alias', '(import "a.rbel" as a)\n(import "b.rbel" as a)\n(def main (x) x)', 'alias'),
            ('aliased-legacy', '(import "flat.rbel" as flat)\n(def main (x) x)', 'declared module'),
            ('bare-module', '(import "a.rbel")\n(def main (x) x)', 'alias'),
            ('unknown-export', '(module bad)\n(export missing)\n(def main (x) x)', 'missing'),
            ('duplicate-export', '(module bad)\n(export main main)\n(def main (x) x)', 'duplicate'),
            ('late-module', '(def main (x) x)\n(module late)', 'first'),
            ('known-dead-arity', '(def helper (x) x)\n(def main (x) (if t x (helper x x)))', 'arity'),
            ('primitive-arity', '(def main (x) (car x x))', 'arity'),
            ('syntactic-fn-arity', '(def main (x) ((fn (a) a) x x))', 'arity'),
            ('unknown-binding', '(def main (x) missing)', 'missing'),
            ('bad-scalar-surrogate', '(def main (x) \\u{D800})', 'reading'),
            ('bad-scalar-overflow', '(def main (x) \\u{110000})', 'reading'),
            ('bad-scalar-empty', '(def main (x) \\u{})', 'reading'),
        ]
        for label, source, needle in bad_sources:
            path = write(label + '.rbel', source)
            try:
                bundle = load_bundle([path], query)
                diagnostic = query(['compile-bundle', bundle], 'wat', success=False)
            except (ValueError, AssertionError) as error:
                diagnostic = str(error)
            if needle not in diagnostic or path.name not in diagnostic:
                raise AssertionError((label, diagnostic))
            checks.append(label)
            print('PASS', label, flush=True)
        write('cycle-a.rbel', '(import "cycle-b.rbel")\n(def cycle-a (x) x)')
        cycle = write('cycle-b.rbel', '(import "cycle-a.rbel")\n(def cycle-b (x) x)')
        try:
            load_bundle([cycle], query)
        except ValueError as error:
            if 'cycle' not in str(error): raise
        else:
            raise AssertionError('import cycle accepted')
        checks.append('import-cycle')
        forged = json.loads(json.dumps(main_bundle))
        forged[3].append(['unit-0', chars('invented.rbel'), 'unit-1'])
        diagnostic = query(['compile-bundle', forged], 'wat', success=False)
        if 'edge' not in diagnostic:
            raise AssertionError(('forged-edge', diagnostic))
        checks.append('forged-edge-rejected')
    (OUTPUT / 'results.json').write_text(json.dumps({'passed': True, 'native_checked': not args.portable_only, 'checks': checks}, indent=2)+'\n')
    print(f'{len(checks)} module/source verification groups passed')


if __name__ == '__main__':
    main()
