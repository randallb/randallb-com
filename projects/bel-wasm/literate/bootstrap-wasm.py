#!/usr/bin/env python3
"""Package the existing Rust syntax reader and Wasm rbel compiler for test CI.

This is bootstrap tooling, not a native rbel distribution or a protocol engine.
Run the source-bootstrap steps in libcwr/readme.md first.
"""
import hashlib
import json
from pathlib import Path
import shutil

ROOT = Path(__file__).resolve().parents[1]
BUNDLE = ROOT / 'target/rbel-wasm-bootstrap'

SHIM = r'''#!/usr/bin/env python3
"""Bootstrap-only syntax transport; compilation decisions execute in Wasm."""
import json
from pathlib import Path
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parent.parent
TOOLS = ROOT / 'libexec/rbel'


def pairs(value):
    if isinstance(value, list):
        tail = None
        for head in reversed(value):
            tail = {'pair': [pairs(head), tail]}
        return tail
    if isinstance(value, dict) and set(value) == {'pair'}:
        return {'pair': [pairs(v) for v in value['pair']]}
    return value


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
    syntax = subprocess.check_output([str(TOOLS / 'bel-read-json'), str(source)], timeout=60)
    if operation == 'read':
        # Match the native command's debug JSON representation. This is data
        # conversion only; no expression is interpreted by this Python shim.
        print(json.dumps(pairs(json.loads(syntax))))
        return
    with tempfile.TemporaryDirectory(prefix='rbel-bootstrap-emit-') as directory:
        data = Path(directory) / 'syntax.json'
        data.write_bytes(syntax)
        result = subprocess.run(['node', str(TOOLS / 'transport.mjs'), str(TOOLS / 'compiler.wasm'), str(data)],
                                stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=120)
        if result.returncode:
            raise ValueError(result.stderr.decode(errors='replace').strip())
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_bytes(result.stdout)


try:
    main()
except (OSError, ValueError, subprocess.SubprocessError) as error:
    print(f'rbel bootstrap: {error}', file=sys.stderr)
    sys.exit(1)
'''


def main():
    inputs = {
        'bel-read-json': ROOT / 'target/debug/bel-read-json',
        'compiler.wasm': ROOT / 'target/bel-selfhost/stage1.wasm',
        'transport.mjs': ROOT / 'experiments/bel-rust-repl/wasm/self-wat.mjs',
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
        'backend': 'Rust syntax reader; rbel compiler on Wasm; no Rust protocol evaluation',
        'files': {str(p.relative_to(BUNDLE)): hashlib.sha256(p.read_bytes()).hexdigest()
                  for p in [command, *[tools / name for name in inputs]]},
    }
    (BUNDLE / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
    print(f'Bootstrap compiler bundle: {command}')


if __name__ == '__main__':
    main()
