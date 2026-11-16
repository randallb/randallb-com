#!/usr/bin/env python3
"""Extract the literate byte filter, check its examples, and build a native command."""
from pathlib import Path
import subprocess
import sys

from check import ROOT, extract


def main():
    output = ROOT / 'target/bel-native/rbel-uppercase'
    # A failed extraction or example check must not leave a stale runnable product.
    output.unlink(missing_ok=True)
    chapter = ROOT / 'literate/unix-filter.rbel.md'
    definitions, examples, count = extract(chapter.read_bytes().decode('utf-8'))
    destination = ROOT / 'target/literate'
    destination.mkdir(parents=True, exist_ok=True)
    source = destination / 'unix-filter.rbel'
    rows = destination / 'unix-filter-examples.bel'
    source.write_bytes(definitions)
    rows.write_text(examples, encoding='utf-8')
    print(f'Extracted native program and {count} examples', flush=True)
    subprocess.run(['cargo', 'run', '--locked', '--quiet', '-p', 'bel-rust-repl',
                    '--bin', 'rbel-check-examples', '--', str(source), str(rows)],
                   cwd=ROOT, check=True)
    subprocess.run([sys.executable,
                    str(ROOT / 'experiments/bel-rust-repl/wasm/native/build-native.py'),
                    str(source), str(output),
                    '--artifacts', str(ROOT / 'target/bel-native/literate-build')],
                   cwd=ROOT, check=True)


if __name__ == '__main__':
    try:
        main()
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        print(f'error: {error}', file=sys.stderr)
        sys.exit(1)
