#!/usr/bin/env python3
"""Compile runtime syntax data with a Wasm Bel compiler, then assemble its WAT."""
import argparse
from pathlib import Path
import subprocess
import sys
import tempfile

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]


def compile_module(compiler, source, output, assembler):
    compiler, source, output = (p.resolve() for p in (compiler, source, output))
    wat = output.with_suffix('.wat')
    if output.suffix != '.wasm':
        raise ValueError('output must end in .wasm')
    if any(p in (output, wat) for p in (compiler, source)):
        raise ValueError('output must not overwrite compiler or input')
    output.unlink(missing_ok=True)
    wat.unlink(missing_ok=True)
    with tempfile.TemporaryDirectory(prefix='bel-self-compile-') as directory:
        work = Path(directory)
        result = subprocess.run(['node', str(HERE / 'self-wat.mjs'), str(compiler), str(source)],
                                capture_output=True, check=False)
        if result.returncode:
            raise ValueError(result.stderr.decode(errors='replace').strip())
        candidate = work / 'module.wat'
        candidate.write_bytes(result.stdout)
        binary = work / 'module.wasm'
        subprocess.run([str(assembler), str(candidate), '-o', str(binary)], check=True,
                       capture_output=True)
        output.parent.mkdir(parents=True, exist_ok=True)
        wat.write_bytes(result.stdout)
        output.write_bytes(binary.read_bytes())


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('compiler', type=Path)
    parser.add_argument('source', type=Path, help='reader data as JSON; never evaluated on the host')
    parser.add_argument('output', type=Path)
    parser.add_argument('--wat2wasm', type=Path,
                        default=ROOT / 'target/bel-wasm-tools/node_modules/.bin/wat2wasm')
    args = parser.parse_args()
    try:
        compile_module(args.compiler, args.source, args.output, args.wat2wasm)
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        print(f'compile failed: {error}', file=sys.stderr)
        return 1
    print(f'compiled {args.source} -> {args.output}')
    return 0


if __name__ == '__main__':
    sys.exit(main())
