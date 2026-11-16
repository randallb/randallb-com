#!/usr/bin/env python3
"""Build a native program using unchanged foundation and higher-order calls."""
import subprocess
import sys
from check import ROOT, check_pin, extract


def main():
    output = ROOT / 'target/bel-native/rbel-foundation-uppercase'
    output.unlink(missing_ok=True)
    foundation, _, _ = extract((ROOT / 'literate/foundation.rbel.md').read_bytes().decode())
    check_pin(foundation)
    transform, _, _ = extract((ROOT / 'literate/unix-filter.rbel.md').read_bytes().decode())
    entry, rows, count = extract((ROOT / 'literate/foundation-filter.rbel.md').read_bytes().decode())
    directory = ROOT / 'target/literate'
    directory.mkdir(parents=True, exist_ok=True)
    source, examples = directory / 'foundation-filter.rbel', directory / 'foundation-filter-examples.bel'
    source.write_bytes(entry + b'\n' + foundation + b'\n' + transform)
    examples.write_text(rows)
    subprocess.run(['cargo', 'run', '--locked', '--quiet', '-p', 'bel-rust-repl',
                    '--bin', 'rbel-check-examples', '--', str(source), str(examples)],
                   cwd=ROOT, check=True)
    subprocess.run([sys.executable, str(ROOT / 'experiments/bel-rust-repl/wasm/native/build-native.py'),
                    str(source), str(output), '--artifacts',
                    str(ROOT / 'target/bel-native/foundation-build')], cwd=ROOT, check=True)
    print(f'Built foundation filter from pinned definitions and {count} checked examples')


if __name__ == '__main__':
    try:
        main()
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        print(f'error: {error}', file=sys.stderr)
        sys.exit(1)
