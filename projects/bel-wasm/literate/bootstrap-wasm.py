#!/usr/bin/env python3
"""Bootstrap the rbel syntax reader and compiler on Wasm for portable test CI.

This is bootstrap tooling, not a native rbel distribution or a protocol engine.
Run the source-bootstrap steps in libcwr/readme.md first.
"""
import argparse
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
"""Bootstrap-only source transport; semantics execute in the ABI2 rbel core."""
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
ROOT=Path(__file__).resolve().parent.parent
TOOLS=ROOT/'libexec/rbel'
sys.path.insert(0,str(TOOLS))
from module_loader import load_bundle,text

def pairs(value):
    if isinstance(value,list):
        tail=None
        for head in reversed(value):tail={'pair':[pairs(head),tail]}
        return tail
    if isinstance(value,dict) and set(value)=={'pair'}:
        return {'pair':[pairs(item) for item in value['pair']]}
    return value

def main():
    args=sys.argv[1:]
    if args==['--abi-version']:
        print('2');return
    if len(args) not in (2,4) or args[0] not in ('read','emit'):
        raise ValueError('bootstrap usage: rbel read SOURCE | rbel emit SOURCE -o OUTPUT.wat')
    operation,source=args[:2];source=Path(source).resolve();output=None
    if operation=='emit':
        if len(args)!=4 or args[2]!='-o':raise ValueError('emit requires -o OUTPUT.wat')
        output=Path(args[3]).resolve()
    elif len(args)!=2:raise ValueError('read takes exactly one source')
    with tempfile.TemporaryDirectory(prefix='rbel-source-bundle-') as temporary:
        request=Path(temporary)/'request.json'
        def query(value,mode='value',binary=None):
            request.write_text(json.dumps(value,ensure_ascii=False))
            result=subprocess.run(['node',str(TOOLS/'module-query.mjs'),str(binary or TOOLS/'core.wasm'),str(request),mode],
                capture_output=True,timeout=300)
            if result.returncode:raise ValueError(result.stderr.decode(errors='replace').strip())
            return result.stdout if mode=='wat' else json.loads(result.stdout)
        bundle=load_bundle([source],query)
        if output is not None:
            if output.suffix in ('.md','.rbel','.bel','.c','.h'):
                raise ValueError('output has a protected source extension')
            inputs=[(source.parent/text(unit[1])).resolve() for unit in bundle[2]]
            resource_alias=output.exists() and any(p.is_file() and output.samefile(p) for p in ROOT.rglob('*'))
            if output.is_relative_to(ROOT) or resource_alias or any(
                output==p or output.exists() and output.samefile(p) for p in inputs):
                raise ValueError('output aliases source or compiler bundle')
            output.unlink(missing_ok=True)
        if operation=='read':
            print(json.dumps(pairs(query(['read-bundle',bundle]))))
        else:
            # Snapshot/discovery and alias checks finish before clearing stale
            # output. All semantic/example failures below leave no old product.
            examples=bundle[4]
            if examples:
                check_wat=Path(temporary)/'examples.wat'
                check_wasm=Path(temporary)/'examples.wasm'
                check_wat.write_bytes(query(['check-bundle',bundle],'wat'))
                assembled=subprocess.run([str(TOOLS/'wat2wasm'),str(check_wat),'-o',str(check_wasm)],
                    capture_output=True,timeout=120)
                if assembled.returncode:
                    raise ValueError(assembled.stderr.decode(errors='replace').strip() or 'example module assembly failed')
                try:
                    evidence=query([f'mod-example-{index}' for index in range(len(examples))],
                        'selectors',check_wasm)
                except ValueError as error:
                    failed=re.search(r'example mod-example-([0-9]+) failed',str(error))
                    if failed and int(failed.group(1))<len(examples):
                        row=examples[int(failed.group(1))]
                        owner=next(unit for unit in bundle[2] if unit[0]==row[0])
                        raise ValueError(f'{text(owner[1])}:{text(row[1])}: example {row[2]} failed') from error
                    raise
                if not isinstance(evidence,dict) or set(evidence)!={'passed'} or type(evidence['passed']) is not int or evidence['passed']!=len(examples):
                    raise ValueError('example verification did not execute every discovered case')
            result=query(['compile-bundle',bundle],'wat')
            output.parent.mkdir(parents=True,exist_ok=True);temporary_output=None
            try:
                with tempfile.NamedTemporaryFile(dir=output.parent,delete=False) as file:
                    temporary_output=Path(file.name);file.write(result)
                os.replace(temporary_output,output)
            finally:
                if temporary_output is not None:temporary_output.unlink(missing_ok=True)
try:main()
except (OSError,ValueError,subprocess.SubprocessError,RecursionError) as error:
    print(f'rbel bootstrap: {error}',file=sys.stderr);sys.exit(1)
'''


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--reuse-core', action='store_true',
                        help='refresh the portable package from the existing verified core without rebuilding')
    args = parser.parse_args()
    command = ROOT / 'experiments/bel-rust-repl/wasm/command'
    core_directory = ROOT / 'target/rbel-core-bootstrap'
    if args.reuse_core:
        binary = core_directory / 'core.wasm'
        receipt = json.loads((core_directory / 'core-build.json').read_text())
        if receipt.get('abi_version') != 2 or receipt.get('language_version') != 2:
            raise ValueError('existing core receipt has an incompatible ABI or language version')
        from core_build import composition
        body, snapshots = composition()
        current_components = {str(path.relative_to(ROOT)): hashlib.sha256(data).hexdigest()
                              for path, data in snapshots.items()}
        if current_components != receipt.get('components') or hashlib.sha256(body).hexdigest() != receipt.get('source_sha256'):
            raise ValueError('core source changed since the verified build; rebuild before refreshing the package')
        if hashlib.sha256(binary.read_bytes()).hexdigest() != receipt.get('canonical_wasm_sha256'):
            raise ValueError('existing core differs from its verified build receipt')
        wat = core_directory / 'core.wat'
        if wat.exists() and hashlib.sha256(wat.read_bytes()).hexdigest() != receipt.get('canonical_wat_sha256'):
            raise ValueError('existing core WAT differs from its verified build receipt')
    else:
        from core_build import build_core
        _, binary, _ = build_core(core_directory)
    inputs = {
        'core.wasm': binary,
        'transport.mjs': ROOT / 'literate/core-transport.mjs',
        'module-query.mjs': command / 'module-query.mjs',
        'module_loader.py': command / 'module_loader.py',
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
        if name == 'module-query.mjs':
            (tools/name).write_text(source.read_text().replace("'../../../../literate/wasm-values.mjs'", "'./wasm-values.mjs'"))
        else:
            shutil.copy2(source.resolve(), tools / name)
    command = BUNDLE / 'bin/rbel'
    command.write_text(SHIM)
    command.chmod(0o755)
    (tools / 'wat2wasm').chmod(0o755)
    manifest = {
        'abi_version': 2, 'language_version': 2,
        'backend': 'rbel reader and compiler on Wasm; Rust used only to bootstrap the core',
        'files': {str(p.relative_to(BUNDLE)): hashlib.sha256(p.read_bytes()).hexdigest()
                  for p in [command, *[tools / name for name in inputs]]},
    }
    (BUNDLE / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
    print(f'Bootstrap compiler bundle: {command}')


if __name__ == '__main__':
    main()
