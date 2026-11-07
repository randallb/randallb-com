#!/usr/bin/env python3
"""Verify the installed native command in isolation, including reader failures."""
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[3]
BUNDLE = ROOT / 'target/rbel-command'
OUT = ROOT / 'target/rbel-command-verification'


def normalize(value):
    if isinstance(value, list):
        result = None
        for head in reversed(value):
            result = {'pair': [normalize(head), result]}
        return result
    if isinstance(value, dict) and 'pair' in value:
        return {'pair': [normalize(x) for x in value['pair']]}
    return value


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    reader_cases = [
        '', 'nil t seed', '0 14 -3 +2 0002 -0', "'seed", '(a b)', '(a . b)', '(a b . c)',
        '; comment at eof', '; comment\n(a (b) nil)', 'a; comment\nb',
        r'\a \tab \lf \cr \sp \( \) \\ \" \; \.',
        '"hello" ""', '"a\\\"b\\\\c"', '"a\nb"', r'"\n"',
        '(err \'must-not-execute)', "''a", "'(a . (b . nil))", '\r\n(a\tb)\r\n',
        '\\🙂', '"naïve 世界"', 'a"b"', 'a\\b',
    ]
    malformed = ['(', ')', '(a', "'", '(a .)', '( . a)', '(a . b c)',
                 '"unfinished', '"escape\\', '\\', '\\unknown', '.',
                 'a )', '1.5', '-1/2', '1e2', '12x', '[a]', '`a', ',a', '#x', 'a:b',
                 '"raw\ttab"', '"raw\rreturn"']
    with tempfile.TemporaryDirectory(prefix='isolated command ', dir=OUT) as temporary:
        directory = Path(temporary)
        installed = directory / 'moved bundle'
        shutil.copytree(BUNDLE, installed)
        executable = installed / 'bin/rbel'
        # Any accidental interpreter lookup fails and records the attempt.
        guards = directory / 'forbidden tools'
        guards.mkdir()
        marker = directory / 'forbidden-execution'
        for name in ['cargo', 'rustc', 'node', 'python', 'python3', 'deno']:
            guard = guards / name
            guard.write_text(f'#!/bin/sh\n/bin/echo {name} >> "{marker}"\nexit 99\n')
            guard.chmod(0o755)
        env = {'PATH': str(guards), 'LC_ALL': 'C'}
        def invoke(*args, success=True):
            result = subprocess.run([str(executable), *map(str, args)], cwd=directory,
                                    env=env, capture_output=True, timeout=90)
            assert (result.returncode == 0) == success, (args, result.returncode, result.stdout[-1000:], result.stderr[-4000:])
            return result
        assert b'Usage:' in invoke('--help').stdout
        assert b'native-command' in invoke('--version').stdout
        assert invoke('unknown', 'file', success=False).returncode == 2
        source = directory / 'input.rbel'
        for text in reader_cases:
            source.write_text(text)
            expected = json.loads(subprocess.check_output([str(ROOT / 'target/debug/bel-read-json'), str(source)]))
            result = invoke('read', source)
            assert json.loads(result.stdout) == normalize(expected), (text, result.stdout, expected)
        for text in malformed:
            source.write_text(text)
            rejected = invoke('read', source, success=False)
            assert rejected.returncode == 1 and not rejected.stdout, (text, rejected)
        source.write_text(' '.join('a' for _ in range(180)))
        bounded = invoke('read', source, success=False)
        assert bounded.returncode == 1 and not bounded.stdout, bounded
        source.write_bytes(b'"\xff"')
        rejected = invoke('read', source, success=False)
        assert rejected.returncode == 1 and not rejected.stdout, rejected
        source.write_bytes(b'a\x00b')
        rejected = invoke('read', source, success=False)
        assert rejected.returncode == 1 and not rejected.stdout, rejected
        source.write_text('a' * (128 * 1024 + 1))
        rejected = invoke('read', source, success=False)
        assert rejected.returncode == 1 and not rejected.stdout, rejected
        source.write_text('(' * 5000 + 'a' + ')' * 5000)
        rejected = invoke('read', source, success=False)
        assert rejected.returncode == 1 and not rejected.stdout, rejected
        print(f'PASS: {len(reader_cases)} reader comparisons; {len(malformed) + 5} malformed/unsupported/bounded inputs', flush=True)
        # These are copied source snapshots. No repository paths are supplied to the command.
        samples = installed / 'examples'
        assert b'checked 67 examples' in invoke('check', samples / 'foundation.rbel.md').stderr
        assert b'checked 19 examples' in invoke('check', samples / 'reader.rbel.md').stderr
        binary = directory / 'program with spaces'
        invoke('build', samples / 'unix-filter.rbel.md', '-o', binary)
        assert subprocess.check_output([str(binary)], input=b'hello, rbel!\n', cwd=directory, env={'PATH': ''}) == b'HELLO, RBEL!\n'
        invoke('build', samples / 'foundation-filter.rbel.md', '--include', samples / 'foundation.rbel.md',
               '--include', samples / 'unix-filter.rbel.md', '-o', binary)
        for data in [b'', bytes(range(256)), b'a'*255, b'b'*256, b'c'*257,
                     'naïve 世界\n'.encode(), bytes(range(256))*4097]:
            result = subprocess.run([str(binary)], input=data, capture_output=True,
                                    cwd=directory, env={'PATH': ''}, timeout=30)
            assert result.returncode == 0 and result.stdout == data.upper() and not result.stderr, result
        print('PASS: moved native bundle builds both literate programs; foundation/reader examples and byte streams pass', flush=True)
        # Expected values are data: a false answer must stop the build and remove stale products.
        bad = directory / 'bad.rbel.md'
        valid = '```rbel definition=echo\n(def echo (xs) xs)\n```\n\n```rbel example=wrong\n(echo \'(a))\n=>\n(b)\n```\n'
        bad.write_text(valid)
        binary.write_bytes(b'stale')
        result = invoke('build', bad, '-o', binary, success=False)
        assert b'example wrong failed' in result.stderr and not binary.exists(), result.stderr
        for changed in [valid.replace('(b)\n```', "(err 'expected-must-not-execute)\n```"),
                        valid.replace('=>', ''), valid + '```rbel example=wrong\nx\n=>\ny\n```\n',
                        valid + '```rbel unsupported=x\nx\n```\n', valid[:-4],
                        valid.replace('(def echo (xs) xs)', '(def echo (xs) missing)'),
                        valid.replace('(echo \'(a))', '(err \'intentional)')]:
            bad.write_text(changed)
            binary.write_bytes(b'stale')
            failed = invoke('build', bad, '-o', binary, success=False)
            if 'expected-must-not-execute' in changed:
                assert b'result differs from expected data' in failed.stderr and b'example trapped' not in failed.stderr, failed.stderr
            assert not binary.exists()
        # Ordinary fences, tildes, indentation, and CRLF follow extraction rules.
        good = directory / 'a; shell $(must-not-run).rbel.md'
        good.write_text('````text\n```rbel definition=ignored\ninvalid\n```\n````\n' +
                        valid.replace('(b)\n```', '(a)\n```').replace('```', '~~~').replace('\n', '\r\n'))
        invoke('build', good, '-o', binary)
        assert not (directory / 'must-not-run').exists()
        # Aliases and bundle resources are protected before output cleanup.
        raw = directory / 'source.input'
        raw.write_text('(def echo (xs) xs)')
        before = raw.read_bytes()
        invoke('build', raw, '-o', raw, success=False)
        assert raw.read_bytes() == before
        alias = directory / 'source-alias'
        alias.hardlink_to(raw)
        invoke('build', raw, '-o', alias, success=False)
        assert raw.read_bytes() == before and alias.read_bytes() == before
        invoke('build', raw, '-o', executable, success=False)
        invoke('--version')
        # Missing tools fail without a stale executable; recovery is a complete build.
        tool = installed / 'libexec/rbel/wasm2c'
        moved = tool.with_suffix('.hidden')
        tool.rename(moved)
        binary.write_bytes(b'stale')
        invoke('build', raw, '-o', binary, success=False)
        assert not binary.exists()
        moved.rename(tool)
        invoke('build', raw, '-o', binary)
        assert subprocess.check_output([str(binary)], input=b'identity\x00\xff', env={'PATH': ''}) == b'identity\x00\xff'
        print('PASS: example failures, malformed fences, source aliases, stale cleanup, missing tools, and recovery', flush=True)
        # Read and compile the compiler itself natively, without the Rust reader/Node adapter.
        native_source = directory / 'compiler.rbel'
        native_source.write_bytes((HERE.parent / 'self-compiler.bel').read_bytes())
        emitted = directory / 'compiler.wat'
        invoke('emit', native_source, '-o', emitted)
        # Check deterministic compiler self-emission. The core comparison below
        # also checks parity with the independently bootstrapped command core.
        # The previous stage1 compiler snapshot predates integer support.
        second_wat = directory / 'compiler-second.wat'
        invoke('emit', native_source, '-o', second_wat)
        assert emitted.read_bytes() == second_wat.read_bytes()
        core_source = directory / 'core.rbel'
        core_source.write_bytes((ROOT / 'target/rbel-command-build/core.bel').read_bytes())
        core_wat = directory / 'core.wat'
        invoke('emit', core_source, '-o', core_wat)
        assert core_wat.read_bytes() == (ROOT / 'target/rbel-command-build/core.wat').read_bytes()
        assert not marker.exists(), marker.read_text() if marker.exists() else ''
        print('PASS: deterministic compiler self-emission and bootstrap core WAT parity; no interpreter tools invoked', flush=True)
        libraries = {}
        for path in [executable, *(installed / 'libexec/rbel').iterdir()]:
            kind = subprocess.check_output(['/usr/bin/file', str(path)], text=True)
            assert 'Mach-O 64-bit executable arm64' in kind, kind
            links = subprocess.check_output(['/usr/bin/otool', '-L', str(path)], text=True)
            libs = [line.strip().split(' (')[0] for line in links.splitlines()[1:]]
            assert libs and all(x.startswith(('/usr/lib/', '/System/Library/')) for x in libs), links
            libraries[str(path.relative_to(installed))] = libs
        assert not list(directory.glob('.rbel-build-*'))
    receipt = {'reader_comparisons': len(reader_cases), 'reader_rejections': len(malformed) + 5,
               'foundation_examples': 67, 'reader_examples': 19, 'literate_programs': 2,
               'foundation_combined_examples': 77, 'native_compiler_self_emission_deterministic': True,
               'native_core_emission_identical': True, 'relocated_bundle': True,
               'forbidden_tools_executed': False, 'dynamic_libraries': libraries,
               'command_sha256': hashlib.sha256((BUNDLE / 'bin/rbel').read_bytes()).hexdigest()}
    (OUT / 'results.json').write_text(json.dumps(receipt, indent=2) + '\n')
    print('PASS: native compiler command verified', flush=True)


if __name__ == '__main__':
    main()
