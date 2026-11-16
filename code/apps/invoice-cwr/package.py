#!/usr/bin/env python3
"""Build a standalone local CWR pack from the maintained eval-runner source."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import tarfile

RUNNER_FILES = ('acceptance_eval.py', 'cli.py', 'lifecycle.py', 'builder.py', 'guest.py',
                'tart_backend.py', 'codex_payload.py', 'codex-pin.json',
                'interactive.py', 'host_auth.py', 'workspace.py', 'NOTICE', 'LICENSE.wlp-mono')


def build(output):
    source = Path(__file__).resolve().parent
    output.mkdir(parents=True, exist_ok=False)
    runner = output / 'runner'
    runner.mkdir()
    for name in RUNNER_FILES:
        shutil.copyfile(source.parent / 'eval-runner' / name, runner / name)
    for name in ('install.py', 'action.py', 'invoke.py', 'workflow.py', 'prepare_base.py', 'README.md'):
        shutil.copyfile(source / name, output / name)
    shutil.copytree(source / 'scenario-template', output / 'scenario-template')
    hashes = {str(f.relative_to(output)): hashlib.sha256(f.read_bytes()).hexdigest()
              for f in sorted(output.rglob('*')) if f.is_file()}
    (output / 'bundle.json').write_text(json.dumps({
        'format': 'invoice-cwr-local-v1', 'version': '0.2.0',
        'platform': 'aarch64-darwin', 'files': hashes}, indent=2) + '\n')
    archive = Path(str(output) + '.tar.gz')
    if archive.exists():
        raise FileExistsError(archive)
    with tarfile.open(archive, 'w:gz') as tar:
        tar.add(output, arcname=output.name)
    print(archive)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    build(parser.parse_args().output.resolve())
