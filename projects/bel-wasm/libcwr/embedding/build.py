#!/usr/bin/env python3
"""Build an import-free libcwr core and browser/CommonJS embedding adapters."""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[2]
HERE = Path(__file__).resolve().parent


def digest(data):
    return hashlib.sha256(data).hexdigest()


def source_revision():
    try:
        revision = subprocess.check_output(['git', '-C', str(ROOT), 'rev-parse', 'HEAD'], text=True, stderr=subprocess.DEVNULL).strip()
        dirty = subprocess.check_output(['git', '-C', str(ROOT), 'status', '--porcelain', '--untracked-files=normal'], text=True)
        return revision + ('-dirty' if dirty.strip() else '')
    except (OSError, subprocess.SubprocessError):
        return 'unversioned-source'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--rbel', type=Path, default=ROOT / 'target/rbel-command/bin/rbel')
    parser.add_argument('--output', type=Path, default=ROOT / 'target/libcwr-embedding')
    args = parser.parse_args()
    output, rbel = args.output.resolve(), args.rbel.resolve()
    target = (ROOT / 'target').resolve()
    if not output.is_relative_to(target) or output == target or output.is_relative_to(rbel.parent.parent):
        raise ValueError('build output must be a dedicated target directory outside the compiler bundle')
    if output.exists():
        marker = output / 'manifest.json'
        if not marker.is_file() or json.loads(marker.read_text()).get('format') != 'libcwr-embedding-v1':
            raise ValueError('refusing to replace an unrecognized output directory')
    source = ROOT / 'libcwr/workflow.rbel.md'
    inputs = [source, HERE / 'core.mjs', HERE / 'host.cjs', *[HERE / n for n in ['native-values.h', 'native-values.c', 'native-core.c', 'native-host.c', 'native-host.h', 'native-cli.c']],
              ROOT / 'literate/wasm-values.mjs', ROOT / 'literate/test.py', Path(__file__).resolve(),
              HERE / 'build-native.py', HERE / 'verify.py', HERE / 'regressions.py', HERE / 'cli-verify.py',
              *[ROOT / name for name in ['LICENSE', 'NOTICE.txt', 'ORIGIN.json']],
              *[HERE / name for name in ['readme.md', 'native-build.md', 'cli-guide.md', 'example-context.json', 'example-events.jsonl']]]
    snapshots = {p: p.read_bytes() for p in inputs}
    spec = importlib.util.spec_from_file_location('literate_tests', ROOT / 'literate/test.py')
    extractor = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(extractor)
    definitions = [b['body'] for b in extractor.blocks(source, snapshots[source]) if b['kind'] == 'definition' and b['name'] != 'acceptance-fixtures']
    if not definitions:
        raise ValueError('no protocol definitions')
    # The compiler exports the first definition as choose. The transport uses
    # the same (call request) envelope as the adjacent-test entry adapter.
    authored = '(def cwr-module (request) (cw-engine (car (cdr request))))\n' + '\n'.join(definitions)
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='libcwr-package-', dir=output.parent) as temporary:
        staging = Path(temporary) / 'package'
        staging.mkdir()
        raw = Path(temporary) / 'core.rbel'
        wat = Path(temporary) / 'core.wat'
        raw.write_text(authored)
        subprocess.run([str(rbel), 'emit', str(raw), '-o', str(wat)], check=True, timeout=120)
        assembler = rbel.parent.parent / 'libexec/rbel/wat2wasm'
        subprocess.run([str(assembler), str(wat), '-o', str(staging / 'core.wasm')], check=True, timeout=60)
        values = snapshots[ROOT / 'literate/wasm-values.mjs'].decode()
        core = snapshots[HERE / 'core.mjs'].decode()
        if values.count('export class WasmValues') != 1 or core.count('export class Core') != 1:
            raise ValueError('unsupported adapter export shape')
        original_import = "import {WasmValues} from '../../literate/wasm-values.mjs';"
        if core.count(original_import) != 1:
            raise ValueError('unsupported adapter import shape')
        (staging / 'values.mjs').write_text(values)
        (staging / 'values.cjs').write_text(values.replace('export class WasmValues', 'class WasmValues') + '\nmodule.exports = {WasmValues};\n')
        (staging / 'core.mjs').write_text(core.replace(original_import, "import {WasmValues} from './values.mjs';"))
        (staging / 'core.cjs').write_text(core.replace(original_import, "const {WasmValues} = require('./values.cjs');").replace('export class Core', 'class Core') + '\nmodule.exports = {Core};\n')
        for name in ['host.cjs']:
            (staging / name).write_bytes(snapshots[HERE / name])
        # Source attribution and redistribution notices accompany both products.
        for name in ['LICENSE', 'NOTICE.txt', 'ORIGIN.json']:
            (staging / name).write_bytes(snapshots[ROOT / name])
        (staging / 'workflow.rbel.md').write_bytes(snapshots[source])
        for name in ['readme.md', 'native-build.md', 'cli-guide.md', 'example-context.json', 'example-events.jsonl']:
            (staging / name).write_bytes(snapshots[HERE / name])
        manifest = {
            'format': 'libcwr-embedding-v1', 'abi': 'libcwr-transition-v1',
            'entry': 'Core.transition(state,event,trustedEnvironment) -> [nextState,response,effect]',
            'source_sha256': digest(snapshots[source]),
            'excluded_definition_blocks': ['acceptance-fixtures'],
            'inputs': {str(p.relative_to(ROOT)): digest(data) for p, data in snapshots.items()},
            'compiler_sha256': digest(rbel.read_bytes()),
            'source_revision': source_revision(),
            'files': {p.name: digest(p.read_bytes()) for p in sorted(staging.iterdir())},
        }
        (staging / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
        if any(p.read_bytes() != data for p, data in snapshots.items()):
            raise ValueError('source changed while building package')
        # Build privately, then replace only a recognized prior package.
        backup = Path(temporary) / 'previous'
        if output.exists():
            os.replace(output, backup)
        try:
            os.replace(staging, output)
        except BaseException:
            if backup.exists():
                os.replace(backup, output)
            raise
    print(f'Built embedding package: {output}')


if __name__ == '__main__':
    main()
