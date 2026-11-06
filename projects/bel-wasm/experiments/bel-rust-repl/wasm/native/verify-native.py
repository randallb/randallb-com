#!/usr/bin/env python3
"""Build real Unix executables, then test them with no build tools on PATH."""
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
ROOT = HERE.parents[3]
OUT = ROOT / 'target/bel-native'
sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location('native_builder', HERE / 'build-native.py')
builder = importlib.util.module_from_spec(spec)
spec.loader.exec_module(builder)


def build(source, output, **kwargs):
    builder.build(source, output, ROOT / 'target/bel-selfhost/stage1.wasm',
                  ROOT / 'target/debug/bel-read-json',
                  ROOT / 'target/bel-wasm-tools/node_modules/.bin',
                  ROOT / 'target/wabt-native-1.0.39/wasm2c', **kwargs)


def execute(binary, data=b'', args=(), success=True):
    # Only the native executable is present. An empty PATH prevents Node, Rust,
    # Python, WABT, or a separate Wasm runtime from being found at execution.
    with tempfile.TemporaryDirectory(prefix='bel-native-external-') as directory:
        copied = Path(directory) / 'filter'
        shutil.copy2(binary, copied)
        result = subprocess.run([str(copied), *args], input=data, capture_output=True,
                                cwd=directory, env={'PATH':'', 'LC_ALL':'C'}, timeout=30)
        assert (result.returncode == 0) == success, result
        return result


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    binary = OUT / 'bel-uppercase'
    build(HERE / 'uppercase.bel', binary, artifacts=OUT / 'uppercase-build')
    magic = binary.read_bytes()[:4]
    assert magic in (b'\x7fELF', b'\xcf\xfa\xed\xfe', b'\xfe\xed\xfa\xcf'), magic
    if sys.platform == 'darwin':
        linkage = subprocess.check_output(['/usr/bin/otool', '-L', binary], text=True)
        libraries = [line.strip().split(' (')[0] for line in linkage.splitlines()[1:]]
        assert libraries and all(name.startswith(('/usr/lib/', '/System/Library/'))
                                 for name in libraries), linkage
        kind = subprocess.check_output(['/usr/bin/file', binary], text=True)
        if platform.machine() == 'arm64':
            assert 'arm64' in kind, kind
        print('PASS: macOS architecture and system-only dynamic libraries', flush=True)
    cases = [b'', b'hello, Bel!\n', bytes(range(256)),
             b'a' * 255, b'b' * 256, b'c' * 257,
             'naïve café — hello 世界\n'.encode(), bytes(range(256)) * 4097]
    for data in cases:
        result = execute(binary, data)
        assert result.stdout == data.upper()
        assert result.stderr == b''
    assert b'Usage:' in execute(binary, args=['--help']).stdout
    assert execute(binary, args=['unexpected'], success=False).returncode == 2
    print('PASS: native executable; 8 stdin fixtures including all bytes, UTF-8, chunk boundaries, 1 MiB stream', flush=True)
    print('PASS: isolated execution with empty PATH; help and usage exit status', flush=True)

    # Same native adapter, changed Bel source: output must change.
    source = OUT / 'identity.bel'
    source.write_text('(def identity (xs) xs)\n')
    identity = OUT / 'bel-identity'
    build(source, identity)
    data = b'hello, Bel!\x00\xff'
    assert execute(identity, data).stdout == data
    assert execute(binary, data).stdout != data
    assert identity.read_bytes() != binary.read_bytes()
    print('PASS: changing Bel source changes native program behavior', flush=True)

    rejected = [
        ('trap', "(def test (xs) (err 'intentional))", b'program trapped'),
        ('stack', '(def test (xs) (test xs))', b'program trapped'),
        ('atom', '(def test (xs) t)', b'proper byte list'),
        ('head', '(def test (xs) (join t nil))', b'non-byte character'),
        ('tail', '(def test (xs) (join \\a t))', b'proper byte list'),
    ]
    for name, text, diagnostic in rejected:
        path = OUT / f'{name}.bel'
        path.write_text(text)
        product = OUT / f'bel-{name}'
        build(path, product)
        result = execute(product, b'x', success=False)
        assert result.returncode == 1 and result.stdout == b''
        assert diagnostic in result.stderr, result.stderr
    print('PASS: explicit errors, recursive stack trap, invalid result atoms/heads/tails return status 1 without chunk output', flush=True)

    # Real Unix read/write errors, with SIGPIPE converted to an I/O diagnostic.
    child = subprocess.Popen([str(binary)], stdin=subprocess.PIPE,
                             stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    child.stdout.close()
    child.stdin.write(b'hello')
    child.stdin.close()
    error = child.stderr.read()
    assert child.wait(timeout=10) == 1 and b'stdout' in error
    fd = os.open(OUT, os.O_RDONLY)
    try:
        result = subprocess.run([str(binary)], stdin=fd, capture_output=True, timeout=10)
    finally:
        os.close(fd)
    assert result.returncode == 1 and result.stdout == b'' and b'stdin' in result.stderr
    print('PASS: stdin read error and closed stdout pipe report status 1', flush=True)

    output = OUT / 'bel-recovery'
    for text in ['(def broken (xs) (missing xs))', '(def broken (xs)',
                 '(def wrong-arity (a b) a)']:
        path = OUT / 'invalid.bel'
        path.write_text(text)
        output.write_bytes(b'stale executable')
        try:
            build(path, output)
            raise AssertionError('accepted invalid input or native entry signature')
        except ValueError:
            pass
        assert not output.exists()
    output.write_bytes(b'stale')
    try:
        build(source, output, cc=str(OUT / 'missing-cc'))
        raise AssertionError('accepted missing C compiler')
    except OSError:
        pass
    assert not output.exists()
    original = source.read_bytes()
    try:
        build(source, source)
        raise AssertionError('overwrote source')
    except ValueError:
        pass
    assert source.read_bytes() == original
    build(source, output)
    assert execute(output, data).stdout == data
    print('PASS: source/signature/build failures remove stale executables; source protection and successful recovery', flush=True)
    report = {'platform': platform.platform(), 'machine': platform.machine(),
              'native_format': magic.hex(), 'runtime': 'WABT 1.0.39 linked C support',
              'runtime_archive_sha256': '8aaafc02afdb8832bd51140b833816512a834601a65348e1f433cece6bbf2db7',
              'binary': str(binary), 'binary_bytes': binary.stat().st_size,
              'binary_sha256': hashlib.sha256(binary.read_bytes()).hexdigest(),
              'build_path': 'Bel -> reader data -> self-hosted Wasm compiler -> Wasm -> wasm2c -> C compiler -> native executable',
              'runtime_tools_required': [], 'system_libraries_required': True,
              'passed': True, 'other_platforms_tested': False}
    (OUT / 'native-results.json').write_text(json.dumps(report, indent=2) + '\n')
    print('PASS: native Unix filter verified', flush=True)


if __name__ == '__main__':
    main()
