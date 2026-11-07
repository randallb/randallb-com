#!/usr/bin/env python3
"""Whole-slice checks, meaningful negative runs, and existing language regressions."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]
HERE = Path(__file__).resolve().parent
RUNNER = ROOT / 'literate/test.py'
SOURCE = HERE / 'workflow.rbel.md'
OUTPUT = ROOT / 'target/libcwr'
OUTPUT.mkdir(parents=True, exist_ok=True)
results = []
COMPILER = ROOT / 'target/rbel-command/bin/rbel'
SKIP_NATIVE = False


def check(name, arguments, expected=0, contains=None):
    result = subprocess.run(list(map(str, arguments)), cwd=ROOT, text=True,
                            stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=180)
    valid = (result.returncode == 0) == (expected == 0)
    if contains and contains not in result.stdout:
        valid = False
    results.append(dict(name=name, passed=valid, exit=result.returncode, output=result.stdout))
    print(('PASS ' if valid else 'FAIL ') + name, flush=True)
    if not valid:
        print(result.stdout)
        raise ValueError(f'{name}: unexpected verification result')
    return result


def test_command(path, *extra):
    return [sys.executable, RUNNER, path, '--rbel', COMPILER, *extra]


def main():
    check('complete-workflow', test_command(SOURCE, '--entry', 'cw-engine', '--host', HERE / 'host.mjs',
          '--report', OUTPUT / 'results.json', '--docs', OUTPUT / 'protocol.md'))
    authored = SOURCE.read_text()
    docs = (OUTPUT / 'protocol.md').read_text()
    if not docs.endswith(authored) or hashlib.sha256(SOURCE.read_bytes()).hexdigest() not in docs:
        raise ValueError('generated documentation does not match authored source')
    with tempfile.TemporaryDirectory(prefix='libcwr-negative-') as directory:
        work = Path(directory)
        mutation = work / 'mutated.rbel.md'
        needle = "(cw-member target (cw-get 'allow environment))"
        if authored.count(needle) != 1:
            raise ValueError('authority mutation anchor drift')
        mutation.write_text(authored.replace(needle, 't'))
        check('actual-definition-mutation-fails', test_command(mutation, '--entry', 'cw-engine', '--host', HERE / 'host.mjs'),
              expected=1, contains='participation-is-not-authority')
        mutation.write_text(authored.replace("(rt-assert 'completed (car answer) 'completed", "(rt-assert 'completed (car answer) 'failed", 1))
        check('false-expectation-fails', test_command(mutation, '--entry', 'cw-engine', '--host', HERE / 'host.mjs'),
              expected=1, contains='assertion completed')
        base = '```rbel definition=identity\n(def identity (x) x)\n```\n'
        tests = {
            'no-cases': (base, 'source requires definitions and examples or tests'),
            'unclosed-test': (base + '```rbel test=broken\n(rt-done)\n', 'unclosed fence'),
            'duplicate-test': (base + ('```rbel test=duplicate\n(rt-done)\n```\n' * 2), 'duplicate block'),
            'unknown-attribute': (base + '```rbel test=bad extra=yes\n(rt-done)\n```\n', 'unsupported rbel fence'),
            'empty-test': (base + '```rbel test=empty\n\n```\n', 'empty block'),
            'malformed-expression': (base + '```rbel test=bad\n(fn (x)\n```\n', 'test failure'),
            'malformed-example': (base + '```rbel example=bad\n(identity t)\n```\n', 'malformed example'),
            'explicit-language-error': (base + "```rbel test=trap\n(err 'deliberate-trap)\n```\n", 'deliberate-trap'),
            'heap-resource-bound': ('```rbel definition=consume\n(def consume (x) (consume (join x x)))\n```\n'
                              '```rbel test=bounded\n(consume nil)\n```\n', 'heap-exhausted'),
            'false-assertion': (base + "```rbel test=false\n(rt-assert 'named-failure t nil (fn (ignored) (rt-done)))\n```\n", 'named-failure'),
            'no-assertions': (base + '```rbel test=empty-proof\n(rt-done)\n```\n', 'without an assertion'),
            'reserved-definition': ('```rbel definition=bad\n(def rt-evil (x) x)\n```\n'
                                    '```rbel test=one\n(rt-done)\n```\n', 'reserved or duplicate'),
        }
        for name, (source, message) in tests.items():
            mutation.write_text(source)
            check(name, test_command(mutation), expected=1, contains=message)
        # A clean case after traps proves isolated state/output recovery.
        mutation.write_text(base + "```rbel test=recovered\n(rt-assert 'clean (identity t) t (fn (ignored) (rt-done)))\n```\n")
        check('runner-recovers-after-failure', test_command(mutation), contains='1 assertions passed')
        proof, docs = work / 'proof.json', work / 'docs.md'
        check('evidence-output-success', test_command(mutation, '--report', proof, '--docs', docs))
        if not proof.exists() or not docs.exists():
            raise ValueError('missing successful evidence')
        mutation.write_text(base + '```rbel test=broken\n(rt-done)\n')
        check('stale-evidence-cleared', test_command(mutation, '--report', proof, '--docs', docs),
              expected=1, contains='unclosed fence')
        if proof.exists() or docs.exists():
            raise ValueError('failed run retained prior success evidence')
        before = mutation.read_bytes()
        check('source-output-alias-protected', test_command(mutation, '--report', mutation),
              expected=1, contains='must not overwrite')
        if mutation.read_bytes() != before:
            raise ValueError('output alias damaged source')
        alias = work / 'source-hardlink.md'
        alias.hardlink_to(mutation)
        check('hardlink-alias-protected', test_command(mutation, '--docs', alias),
              expected=1, contains='must not overwrite')
        if mutation.read_bytes() != before:
            raise ValueError('hardlink alias damaged source')
        mutation.write_bytes((base + "~~~rbel test=tilde\n(rt-assert 'ok t t (fn (ignored) (rt-done)))\n~~~\n").replace('\n', '\r\n').encode())
        check('crlf-tilde-format', test_command(mutation), contains='1 assertions passed')
        # Reproduce unsafe evidence destinations without touching the real bundle.
        import shutil
        bundle = work / 'compiler-bundle'
        shutil.copytree(COMPILER.resolve().parent.parent, bundle)
        assembler = bundle / 'libexec/rbel/wat2wasm'
        before_tool = assembler.read_bytes()
        check('compiler-resource-output-protected', test_command(mutation, '--rbel', bundle / 'bin/rbel', '--report', assembler),
              expected=1, contains='must not overwrite')
        if assembler.read_bytes() != before_tool:
            raise ValueError('compiler resource was altered')
        tool_alias = work / 'assembler-hardlink'
        tool_alias.hardlink_to(assembler)
        check('compiler-resource-hardlink-protected', test_command(mutation, '--rbel', bundle / 'bin/rbel', '--docs', tool_alias),
              expected=1, contains='must not overwrite')
        if assembler.read_bytes() != before_tool:
            raise ValueError('compiler resource alias was altered')
        check('compiler-bundle-new-output-protected', test_command(mutation, '--rbel', bundle / 'bin/rbel', '--docs', bundle / 'new-docs.md'),
              expected=1, contains='must not overwrite')
        mutation.write_text(base + "```rbel test=captured-source\n(rt-request 'rewrite nil (fn (ignored) (rt-assert 'snapshot t t (fn (ignored) (rt-done)))))\n```\n")
        host = work / 'rewrite-host.mjs'
        host.write_text("import {readFileSync,writeFileSync} from 'node:fs';\n" +
            "export function createHost() { return { request() { const p=" + json.dumps(str(mutation)) +
            "; writeFileSync(p,readFileSync(p,'utf8').replace(\"rt-assert 'snapshot t t\",\"rt-assert 'snapshot t nil\")); return null; } }; }\n")
        check('source-drift-invalidates-evidence', test_command(mutation, '--host', host, '--report', proof, '--docs', docs),
              expected=1, contains='source changed during verification')
        if proof.exists() or docs.exists():
            raise ValueError('source drift left success evidence')


    check('pinned-foundation', [sys.executable, ROOT / 'literate/check.py', '--extract-only'])
    check('extractor-regressions', [sys.executable, ROOT / 'literate/check_test.py'])
    check('existing-foundation-on-wasm', test_command(ROOT / 'literate/foundation.rbel.md'))
    check('existing-reader-on-wasm', test_command(ROOT / 'literate/reader.rbel.md', '--include', ROOT / 'literate/integers.rbel.md', '--include', ROOT / 'literate/floats.rbel.md'))
    if not SKIP_NATIVE:
        check('native-foundation-regression', [COMPILER, 'check', ROOT / 'literate/foundation.rbel.md'])
        check('native-reader-regression', [COMPILER, 'check', ROOT / 'literate/reader.rbel.md', '--include', ROOT / 'literate/integers.rbel.md', '--include', ROOT / 'literate/floats.rbel.md'])
    else:
        print('UNVERIFIED here: native compiler regressions (portable Wasm bootstrap selected); run default verification on macOS ARM64')
    print(f'{len(results)} verification groups passed')


parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--rbel', type=Path, default=COMPILER)
parser.add_argument('--skip-native', action='store_true', help='portable bootstrap only; explicitly leave native checks unverified here')
arguments = parser.parse_args()
COMPILER = arguments.rbel.resolve()
SKIP_NATIVE = arguments.skip_native

try:
    main()
except (ValueError, OSError, subprocess.SubprocessError) as error:
    print(f'verification failed: {error}', file=sys.stderr)
    sys.exitCode = 1
finally:
    (OUTPUT / 'verification.json').write_text(json.dumps({'passed': bool(results) and all(r['passed'] for r in results)
        and not getattr(sys, 'exitCode', 0), 'source_sha256': hashlib.sha256(SOURCE.read_bytes()).hexdigest(),
        'native_checks_requested': not SKIP_NATIVE,
        'native_checks_performed': all(any(g['name'] == name for g in results) for name in ('native-foundation-regression', 'native-reader-regression')), 'compiler': str(COMPILER),
        'groups': results}, indent=2) + '\n')
if getattr(sys, 'exitCode', 0):
    sys.exit(1)
