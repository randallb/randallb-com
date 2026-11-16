#!/usr/bin/env python3
"""Install an extracted, reviewed invoice CWR bundle on this Mac."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import shlex
import shutil
import subprocess
import sys


def verify(root):
    manifest = json.loads((root / 'bundle.json').read_text())
    if manifest['format'] != 'invoice-cwr-local-v1':
        raise ValueError('unsupported bundle format')
    for name, digest in manifest['files'].items():
        path = root / name
        if path.is_symlink() or not path.resolve().is_relative_to(root.resolve()):
            raise ValueError('bundle contains a link or invalid path')
        if hashlib.sha256(path.read_bytes()).hexdigest() != digest:
            raise ValueError('bundle checksum mismatch: ' + name)
    return manifest


def install(args):
    if platform.system() != 'Darwin' or platform.machine() != 'arm64':
        raise RuntimeError('this pack requires Apple Silicon macOS')
    source = Path(__file__).resolve().parent
    manifest = verify(source)
    destination = args.destination.expanduser().absolute()
    if destination.exists():
        raise FileExistsError('installation destination already exists')
    cwr = Path(args.cwr).expanduser().resolve(strict=True)
    for name in ('cwp-runtime', 'cwr-adapter-sh'):
        if not os.access(cwr.with_name(name), os.X_OK):
            raise ValueError('matching CWR runtime executable missing: ' + name)
    python = Path(sys.executable).resolve()
    tart = Path(shutil.which('tart') or '/missing-tart').resolve(strict=True)
    payload = args.payload.expanduser().resolve(strict=True)
    sys.path.insert(0, str(source / 'runner'))
    import codex_payload
    codex_payload.verify(payload)
    for executable in (cwr, python, tart):
        if not os.access(executable, os.X_OK):
            raise ValueError('prerequisite is not executable')
    # Copy only the reviewed bundle inventory; private state and credentials never enter it.
    destination.mkdir(parents=True, mode=0o700)
    for name in (*manifest['files'], 'bundle.json'):
        target = destination / name
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(source / name, target)
    (destination / 'runs').mkdir(mode=0o700)
    (destination / 'scenarios').mkdir(mode=0o700)
    shutil.copytree(destination / 'scenario-template', destination / 'scenarios' / 'invoice-docs')
    # Keep existing pinned store outputs alive without copying credentials or a checkout.
    for name, dependency in (('codex-payload', payload), ('python', python)):
        if str(dependency).startswith('/nix/store/'):
            store_root = Path('/nix/store') / dependency.relative_to('/nix/store').parts[0]
            subprocess.run(['nix-store', '--add-root', str(destination / name),
                            '--indirect', '--realise', str(store_root)], check=True,
                           stdout=subprocess.DEVNULL)
    config = {'python': str(python), 'cwr': str(cwr), 'tart': str(tart),
              'payload': str(payload), 'bundle_version': manifest['version'],
              'tart_home': str((args.tart_home or destination / '.rbdc-evals' / 'tart').expanduser().absolute())}
    if args.prepared_base:
        config['prepared_base'] = str(args.prepared_base.expanduser().resolve(strict=True))
    (destination / 'config.json').write_text(json.dumps(config, indent=2) + '\n')
    runtime = destination / 'runtime'
    runtime.mkdir()
    (runtime / 'CWR.toml').write_text('''type = "cwp://org.coworkerprotocol/2026-09/schemas"
[[imports]]
path = "./operator.toml"
[[imports]]
path = "./coworker.toml"
as = "invoice_eval"
''')
    (runtime / 'operator.toml').write_text('''type = "cwp://org.coworkerprotocol/2026-09/schemas/coworker"
name = "Invoice evaluation operator"
slug = "operator"
''')
    command = shlex.join([str(python), str(destination / 'action.py')])
    (runtime / 'coworker.toml').write_text('''type = "cwp://org.coworkerprotocol/2026-09/schemas/coworker"
name = "Invoice evaluation"
slug = "invoice_eval"
[[roles]]
path = "./role.toml"
[[integrations]]
path = "./integration.toml"
as = "local"
[integrations.params]
fail_on_nonzero = true
timeout_ms = 3600000
[integrations.params.commands]
run--invoice = ''' + json.dumps(command) + '\n')
    with (runtime / 'coworker.toml').open('a') as handle:
        for verb in ('run-scenario', 'grade-result'):
            action_name = 'run--scenario' if verb == 'run-scenario' else 'grade--result'
            handle.write(action_name + ' = ' + json.dumps(command + ' --action ' + verb) + '\n')
    (runtime / 'integration.toml').write_text('''type = "cwp://org.coworkerprotocol/2026-09/schemas/integration"
[adapter]
ref = "cwp://org.coworkerprotocol/2026-09/adapters/sh"
''')
    (runtime / 'role.toml').write_text('''type = "cwp://org.coworkerprotocol/2026-09/schemas/role"
name = "Invoice evaluator"
[[adapters]]
ref = "cwp://org.coworkerprotocol/2026-09/adapters/sh"
[[actions]]
verb = "run"
object = "invoice"
title = "Run invoice evaluation"
description = "Run Codex in a fresh local VM; return five-check grade and saved evidence links."
[actions.input_schema]
path = "./input.schema.json"
[actions.output_schema]
path = "./output.schema.json"
''')
    with (runtime / 'role.toml').open('a') as handle:
        for verb, obj, title, schema in [('run', 'scenario', 'Run documented scenario and export workspace', 'scenario'),
                                          ('grade', 'result', 'Grade or regrade a saved scenario workspace', 'grade')]:
            handle.write('\n[[actions]]\nverb = ' + json.dumps(verb) + '\nobject = ' + json.dumps(obj)
                         + '\ntitle = ' + json.dumps(title)
                         + '\ndescription = ' + json.dumps(title + '; retain inspectable inputs and evidence.')
                         + '\n[actions.input_schema]\npath = "./' + schema + '.schema.json"'
                         + '\n[actions.output_schema]\npath = "./output.schema.json"\n')
    for schema, field, pattern in [('scenario', 'scenario', '^[A-Za-z0-9][A-Za-z0-9_-]{0,63}$'),
                                    ('grade', 'run_id', '^[a-f0-9]{32}$')]:
        (runtime / (schema + '.schema.json')).write_text(json.dumps({
            'type': 'object', 'additionalProperties': False, 'required': [field],
            'properties': {field: {'type': 'string', 'pattern': pattern}}}) + '\n')
    (runtime / 'input.schema.json').write_text(json.dumps({'type': 'object', 'additionalProperties': False}) + '\n')
    (runtime / 'output.schema.json').write_text(json.dumps({
        'type': 'object', 'required': ['stdout', 'exit_code'],
        'properties': {'stdout': {'type': 'string'}, 'exit_code': {'type': 'integer'}}}) + '\n')
    subprocess.run([str(cwr), '--workspace', str(runtime), 'check', str(runtime / 'CWR.toml')],
                   cwd=destination, check=True, stdout=subprocess.DEVNULL)
    (destination / 'invoke').write_text('#!/bin/sh\nexec ' + shlex.join([
        str(python), str(destination / 'invoke.py')]) + ' "$@"\n')
    (destination / 'invoke').chmod(0o700)
    print(destination / 'invoke')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--destination', type=Path, required=True)
    parser.add_argument('--payload', type=Path, required=True)
    parser.add_argument('--cwr', required=True)
    parser.add_argument('--tart-home', type=Path, help='optional shared local Tart image cache/state; run lock is shared')
    parser.add_argument('--prepared-base', type=Path, help='optional hash-verified local base descriptor from prepare_base.py')
    install(parser.parse_args())
