#!/usr/bin/env python3
"""Build a native Unix byte filter from a one-argument Bel core function."""
import argparse
from pathlib import Path
import os
import shutil
import subprocess
import sys
import tempfile

HERE = Path(__file__).resolve().parent
WASM = HERE.parent
ROOT = HERE.parents[3]
# Avoid package-manager wrappers adding non-system dylibs on macOS.
DEFAULT_CC = '/usr/bin/clang' if sys.platform == 'darwin' else 'cc'


def run(*args):
    result = subprocess.run(list(map(str, args)), capture_output=True)
    if result.returncode:
        raise ValueError(result.stderr.decode(errors='replace').strip() or
                         f'command failed: {args[0]}')
    return result.stdout


def build(source, output, compiler, reader, tools, runtime, cc=DEFAULT_CC, artifacts=None, adapter=None):
    source, output, compiler, reader, tools, runtime = (
        p.resolve() for p in (source, output, compiler, reader, tools, runtime))
    if output.suffix in ('.bel', '.c', '.h', '.py', '.md', '.wasm', '.json'):
        raise ValueError('choose an executable output name, without a source/artifact extension')
    adapter = (adapter or HERE / 'filter-main.c').resolve()
    protected = [source, compiler, reader, adapter, HERE / 'filter-main.c',
                 *HERE.glob('*.py'), *runtime.glob('*')]
    if output in protected:
        raise ValueError('output must not overwrite source, compiler, reader, or runtime')
    if artifacts:
        artifacts = artifacts.resolve()
        if any((artifacts / name).resolve() in protected or (artifacts / name).resolve() == output
               for name in ('module.c', 'module.h', 'module.wat', 'module.wasm', 'source.ast.json')):
            raise ValueError('artifacts must not overwrite inputs or executable')
    output.unlink(missing_ok=True)
    if run(tools / 'wasm2c', '--version').decode().strip() != '1.0.39':
        raise ValueError('requires wasm2c 1.0.39 to match the runtime sources')
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='bel-native-', dir=output.parent) as directory:
        work = Path(directory)
        data = work / 'source.ast.json'
        data.write_bytes(run(reader, source))
        wat = work / 'module.wat'
        wat.write_bytes(run('node', WASM / 'self-wat.mjs', compiler, data))
        binary = work / 'module.wasm'
        run(tools / 'wat2wasm', wat, '-o', binary)
        run(tools / 'wasm-validate', binary)
        run(tools / 'wasm2c', binary, '-n', 'bel', '-o', work / 'module.c')
        candidate = work / 'filter'
        run(cc, '-std=c99', '-O2', '-Wall', '-Wextra', '-Werror',
            '-Wno-unused-function', '-Wno-unused-parameter',
            '-Wno-unused-but-set-variable',  # wasm2c preserves unused source parameters
            '-fno-optimize-sibling-calls', '-frounding-math',
            '-DWASM_RT_USE_MMAP=0', '-DWASM_RT_MEMCHECK_BOUNDS_CHECK=1',
            '-DWASM_RT_MEMCHECK_GUARD_PAGES=0', '-DWASM_RT_STACK_DEPTH_COUNT=1',
            '-DWASM_RT_STACK_EXHAUSTION_HANDLER=0', '-DWASM_RT_MAX_CALL_STACK_DEPTH=4096',
            '-I', runtime, '-I', work, adapter, work / 'module.c',
            runtime / 'wasm-rt-impl.c', runtime / 'wasm-rt-mem-impl.c',
            runtime / 'wasm-rt-exceptions-impl.c',
            '-lm', '-pthread', '-o', candidate)
        if artifacts:
            artifacts.mkdir(parents=True, exist_ok=True)
            for name in ('module.c', 'module.h', 'module.wat', 'module.wasm', 'source.ast.json'):
                shutil.copyfile(work / name, artifacts / name)
        candidate.chmod(0o755)
        os.replace(candidate, output)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('source', type=Path)
    parser.add_argument('output', type=Path)
    parser.add_argument('--compiler', type=Path, default=ROOT / 'target/bel-selfhost/stage1.wasm')
    parser.add_argument('--reader', type=Path, default=ROOT / 'target/debug/bel-read-json')
    parser.add_argument('--tools', type=Path, default=ROOT / 'target/bel-wasm-tools/node_modules/.bin')
    parser.add_argument('--runtime', type=Path, default=ROOT / 'target/wabt-native-1.0.39/wasm2c')
    parser.add_argument('--cc', default=DEFAULT_CC)
    parser.add_argument('--artifacts', type=Path)
    parser.add_argument('--adapter', type=Path, help='generic C entry adapter; defaults to byte filter')
    args = parser.parse_args()
    try:
        build(args.source, args.output, args.compiler, args.reader, args.tools,
              args.runtime, args.cc, args.artifacts, args.adapter)
    except (OSError, ValueError) as error:
        print(f'native build failed: {error}', file=sys.stderr)
        return 1
    print(f'built {args.output}')
    return 0


if __name__ == '__main__':
    sys.exit(main())
