#!/usr/bin/env python3
"""Bootstrap the rbel syntax reader and compiler on Wasm for portable test CI.

This is bootstrap tooling, not a native rbel distribution or a protocol engine.
Run the source-bootstrap steps in libcwr/readme.md first.
"""
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
BUNDLE = ROOT / 'target/rbel-wasm-bootstrap'

SHIM = r'''#!/usr/bin/env python3
"""Bootstrap-only transport; reading and compilation execute in Wasm."""
import os
from pathlib import Path
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parent.parent
TOOLS = ROOT / 'libexec/rbel'



def main():
    arguments = sys.argv[1:]
    if len(arguments) not in (2, 4) or arguments[0] not in ('read', 'emit'):
        raise ValueError('bootstrap usage: rbel read SOURCE | rbel emit SOURCE -o OUTPUT.wat')
    operation, source = arguments[:2]
    source = Path(source).resolve()
    output = None
    if operation == 'emit':
        if len(arguments) != 4 or arguments[2] != '-o':
            raise ValueError('emit requires -o OUTPUT.wat')
        output = Path(arguments[3]).resolve()
        resource_alias = output.exists() and any(
            path.is_file() and output.samefile(path) for path in ROOT.rglob('*'))
        if output == source or (output.exists() and output.samefile(source)) or output.is_relative_to(ROOT) or resource_alias:
            raise ValueError('output aliases source or compiler bundle')
        output.unlink(missing_ok=True)
    elif len(arguments) != 2:
        raise ValueError('read takes exactly one source')
    if source.stat().st_size > 131072:
        raise ValueError('source exceeds 128 KiB')
    result = subprocess.run(['node', str(TOOLS / 'transport.mjs'), str(TOOLS / 'core.wasm'),
                             'read' if operation == 'read' else 'compile', str(source)],
                            capture_output=True, timeout=120)
    if result.returncode:
        raise ValueError(result.stderr.decode(errors='replace').strip())
    if operation == 'read':
        sys.stdout.buffer.write(result.stdout)
    else:
        output.parent.mkdir(parents=True, exist_ok=True)
        temporary = None
        try:
            with tempfile.NamedTemporaryFile(dir=output.parent, delete=False) as file:
                temporary = Path(file.name)
                file.write(result.stdout)
            os.replace(temporary, output)
        finally:
            if temporary is not None:
                temporary.unlink(missing_ok=True)



try:
    main()
except (OSError, ValueError, subprocess.SubprocessError) as error:
    print(f'rbel bootstrap: {error}', file=sys.stderr)
    sys.exit(1)
'''


def main():
    sys.path.insert(0, str(ROOT / 'literate'))
    from check import extract
    work = ROOT / 'target/rbel-core-bootstrap'
    work.mkdir(parents=True, exist_ok=True)
    source = work / 'core.rbel'
    command = ROOT / 'experiments/bel-rust-repl/wasm/command'
    source.write_bytes((command / 'frontend.bel').read_bytes() + b'\n' + b'\n'.join(
        extract((ROOT / 'literate' / name).read_text())[0]
        for name in ['reader.rbel.md', 'integers.rbel.md', 'floats.rbel.md']) + b'\n' +
        (command.parent / 'self-compiler.bel').read_bytes())
    syntax = work / 'core.json'
    syntax.write_bytes(subprocess.check_output([str(ROOT / 'target/debug/bel-read-json'), str(source)]))
    binary = work / 'core.wasm'
    subprocess.run([sys.executable, str(command.parent / 'self-compile.py'),
                    str(ROOT / 'target/bel-selfhost/stage1.wasm'), str(syntax), str(binary)], check=True)
    inputs = {
        'core.wasm': binary,
        'transport.mjs': ROOT / 'literate/core-transport.mjs',
        'wasm-values.mjs': ROOT / 'literate/wasm-values.mjs',
        'wat2wasm': ROOT / 'target/bel-wasm-tools/node_modules/.bin/wat2wasm',
    }
    for source in inputs.values():
        if not source.is_file():
            raise ValueError(f'missing bootstrap input: {source}')
    tools = BUNDLE / 'libexec/rbel'
    tools.mkdir(parents=True, exist_ok=True)
    (BUNDLE / 'bin').mkdir(exist_ok=True)
    for name, source in inputs.items():
        shutil.copy2(source.resolve(), tools / name)
    command = BUNDLE / 'bin/rbel'
    command.write_text(SHIM)
    command.chmod(0o755)
    (tools / 'wat2wasm').chmod(0o755)
    manifest = {
        'backend': 'rbel reader and compiler on Wasm; Rust used only to bootstrap the core',
        'files': {str(p.relative_to(BUNDLE)): hashlib.sha256(p.read_bytes()).hexdigest()
                  for p in [command, *[tools / name for name in inputs]]},
    }
    (BUNDLE / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
    print(f'Bootstrap compiler bundle: {command}')


if __name__ == '__main__':
    main()
