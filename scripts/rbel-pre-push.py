#!/usr/bin/env python3
"""RBEL_PRE_PUSH_V1: advisory line lint of exact Git objects being pushed."""
from pathlib import Path
import os
import re
import subprocess
import sys
import tempfile


# Push sends original objects, even when this checkout has local replacement refs.
GIT_ENV = {**os.environ, 'GIT_NO_REPLACE_OBJECTS': '1'}


def git(*args, check=True):
    return subprocess.run(['git', '--literal-pathspecs', *args], check=check, capture_output=True, env=GIT_ENV).stdout


def paths_at(commit, remote):
    # New refs and missing remote objects get a complete tip scan, without fetching.
    if not remote.strip('0') or subprocess.run(
            ['git', 'cat-file', '-e', remote + '^{commit}'],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, env=GIT_ENV).returncode:
        return git('ls-tree', '-r', '--name-only', '-z', commit).split(b'\0')
    return git('diff', '--name-only', '-z', '--diff-filter=ACMRT', remote, commit).split(b'\0')


def main():
    entries = []
    for line in sys.stdin:
        fields = line.split()
        if len(fields) != 4:
            raise ValueError('invalid pre-push ref update')
        _, local, _, remote = fields
        if not all(re.fullmatch(r'[0-9a-f]{40}|[0-9a-f]{64}', oid) for oid in (local, remote)):
            raise ValueError('invalid pre-push object ID')
        if not local.strip('0'):
            continue
        # Peel annotated tags too; non-commit pushes have no source tree to lint.
        resolved = subprocess.run(['git', 'rev-parse', '--verify', local + '^{commit}'], capture_output=True, env=GIT_ENV)
        if resolved.returncode:
            continue
        commit = resolved.stdout.decode().strip()
        for raw in paths_at(commit, remote):
            if raw.endswith(b'.rbel.md'):
                entries.append((commit, os.fsdecode(raw)))
    entries = list(dict.fromkeys(entries))
    if not entries:
        return 0
    binary = Path(os.environ.get('RBEL_LINT', str(Path(__file__).resolve().parent / '.rbel-lint/rbel'))).resolve()
    if not binary.is_file() or not os.access(binary, os.X_OK):
        raise ValueError('lint command missing; reinstall with scripts/install-rbel-pre-push.py --rbel PATH')
    checked = 0
    with tempfile.TemporaryDirectory(prefix='rbel-pre-push-') as directory:
        for commit, path in entries:
            record = git('ls-tree', '-z', commit, '--', path)
            if not record:
                continue
            metadata, _ = record.split(b'\t', 1)
            mode, kind, blob = metadata.split()
            if kind != b'blob' or mode not in (b'100644', b'100755'):
                continue
            source = Path(directory) / 'source.rbel.md'
            source.write_bytes(git('cat-file', 'blob', blob.decode()))
            result = subprocess.run([str(binary), 'lint', str(source)], capture_output=True, text=True)
            label = f'{path} [{commit[:12]}]'
            for output in (result.stdout, result.stderr):
                if output:
                    sys.stderr.write(output.replace(str(source), label))
            if result.returncode:
                print(f'rbel pre-push: lint failed for {label}; push stopped', file=sys.stderr)
                return 1
            checked += 1
    print(f'rbel pre-push: checked {checked} source file(s); length warnings are advisory', file=sys.stderr)
    return 0


if __name__ == '__main__':
    try:
        sys.exit(main())
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        print(f'rbel pre-push: {error}; push stopped', file=sys.stderr)
        sys.exit(1)
