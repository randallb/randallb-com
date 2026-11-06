#!/usr/bin/env python3
"""Install the repository's advisory rbel pre-push hook without replacing other hooks."""
from pathlib import Path
import argparse
import os
import subprocess
import sys
import tempfile

MARKER = b'RBEL_PRE_PUSH_V1'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--rbel', required=True, type=Path, help='native rbel command with lint support')
    args = parser.parse_args()
    configured = subprocess.run(['git', 'config', '--get', 'core.hooksPath'], capture_output=True)
    if configured.returncode == 0:
        raise ValueError('core.hooksPath is already configured; preserve it and integrate the rbel hook manually')
    if configured.returncode != 1:
        raise ValueError('cannot inspect Git hook configuration')
    common = Path(subprocess.check_output(['git', 'rev-parse', '--git-common-dir'], text=True).strip()).resolve()
    hooks = common / 'hooks'
    hooks.mkdir(exist_ok=True)
    hook = hooks / 'pre-push'
    if hook.is_symlink() or (hook.exists() and MARKER not in hook.read_bytes()):
        raise ValueError('existing pre-push hook preserved; integrate the rbel check manually')
    source = args.rbel.resolve()
    # Exercise lint without needing any compiler resources or touching repository files.
    with tempfile.TemporaryDirectory(prefix='rbel-hook-install-') as directory:
        empty = Path(directory) / 'empty.rbel.md'
        empty.touch()
        subprocess.run([str(source), 'lint', str(empty)], check=True, capture_output=True)
    resources = hooks / '.rbel-lint'
    resources.mkdir(exist_ok=True)
    # Stage each file in its destination directory and publish by atomic rename.
    for destination, data in [(resources / 'rbel', source.read_bytes()),
                              (hook, Path(__file__).with_name('rbel-pre-push.py').read_bytes())]:
        fd, name = tempfile.mkstemp(prefix='.rbel-install-', dir=destination.parent)
        try:
            with os.fdopen(fd, 'wb') as output:
                output.write(data)
            os.chmod(name, 0o755)
            os.replace(name, destination)
        finally:
            if os.path.exists(name):
                os.unlink(name)
    print(f'Installed rbel pre-push check: {hook}')
    print('Active for this repository and its linked worktrees. Length warnings are advisory.')


if __name__ == '__main__':
    try:
        main()
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        print(f'rbel hook install: {error}', file=sys.stderr)
        sys.exit(1)
