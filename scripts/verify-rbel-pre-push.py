#!/usr/bin/env python3
"""Exercise the installed rbel hook through real pushes to disposable local remotes."""
import argparse
from pathlib import Path
import hashlib
import json
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]
INSTALLER = ROOT / 'scripts/install-rbel-pre-push.py'
VALID = '# Example\n\n```rbel definition=example\n(def example (x) x)\n```\n'
MALFORMED = '# Broken\n```rbel definition=example\n(def example (x) x)\n'


def run(*args, cwd, ok=True):
    result = subprocess.run([str(arg) for arg in args], cwd=cwd,
                            text=True, capture_output=True)
    if (result.returncode == 0) != ok:
        raise AssertionError(f'{args!r}: exit {result.returncode}\n'
                             f'{result.stdout}{result.stderr}')
    return result


def git(repo, *args, ok=True):
    return run('git', *args, cwd=repo, ok=ok)


def commit(repo, message):
    git(repo, 'add', '-A')
    git(repo, 'commit', '-qm', message)


def write(repo, name, body):
    path = repo / name
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(body)


def install(repo, binary, ok=True):
    return run(sys.executable, INSTALLER, '--rbel', binary, cwd=repo, ok=ok)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--rbel', type=Path,
                        default=ROOT / 'projects/bel-wasm/target/rbel-command/bin/rbel')
    args = parser.parse_args()
    binary = args.rbel.resolve()
    if not binary.is_file():
        parser.error(f'native linter not found: {binary}')
    if not INSTALLER.is_file():
        parser.error(f'installer not found: {INSTALLER}')
    output = ROOT / 'projects/bel-wasm/target/pre-push-verification'
    output.mkdir(parents=True, exist_ok=True)
    groups = []
    with tempfile.TemporaryDirectory(prefix='rbel-pre-push-verification-', dir=output) as temporary:
        area = Path(temporary)
        remote, repo = area / 'remote.git', area / 'working tree'
        git(area, 'init', '--bare', '-q', remote)
        git(area, 'clone', '-q', remote, repo)
        git(repo, 'config', 'user.name', 'Local Hook Verification')
        git(repo, 'config', 'user.email', 'hook-test@example.invalid')
        git(repo, 'checkout', '-qb', 'main')
        write(repo, 'valid.rbel.md', VALID)
        commit(repo, 'Initial valid source')
        install(repo, binary)
        git(repo, 'push', '-q', 'origin', 'main')
        groups.append('installed hook permits valid push')

        write(repo, 'oversized.rbel.md', '# Oversized\n' + '\n' * 500)
        commit(repo, 'Advisory length warning')
        result = git(repo, 'push', 'origin', 'main')
        if 'warning' not in (result.stdout + result.stderr).lower():
            raise AssertionError('oversized committed file produced no visible warning')
        groups.append('length findings remain advisory')

        write(repo, 'valid.rbel.md', VALID + '\nCommitted change.\n')
        commit(repo, 'Valid committed change')
        write(repo, 'valid.rbel.md', MALFORMED)
        git(repo, 'push', '-q', 'origin', 'main')
        git(repo, 'checkout', '--', 'valid.rbel.md')
        groups.append('push checks committed blobs instead of dirty worktree')

        write(repo, 'valid.rbel.md', MALFORMED)
        commit(repo, 'Malformed committed fence')
        before = git(remote, 'rev-parse', 'refs/heads/main').stdout.strip()
        git(repo, 'push', 'origin', 'main', ok=False)
        after = git(remote, 'rev-parse', 'refs/heads/main').stdout.strip()
        if before != after:
            raise AssertionError('blocked malformed push changed remote ref')
        write(repo, 'valid.rbel.md', VALID)
        commit(repo, 'Repair committed source')
        git(repo, 'push', '-q', 'origin', 'main')
        groups.append('malformed commit blocks without changing remote; repair pushes')

        git(repo, 'checkout', '-qb', 'new-branch')
        write(repo, 'new.rbel.md', VALID)
        commit(repo, 'Source on new branch')
        git(repo, 'push', '-q', 'origin', 'new-branch')
        git(repo, 'checkout', '-q', 'main')
        git(repo, 'push', '-q', 'origin', '--delete', 'new-branch')
        groups.append('new refs and deleted refs')

        git(repo, 'checkout', '-qb', 'multi-good')
        write(repo, 'good.rbel.md', VALID)
        commit(repo, 'Good multi-ref tip')
        git(repo, 'checkout', '-qb', 'multi-bad', 'main')
        write(repo, 'bad.rbel.md', MALFORMED)
        commit(repo, 'Bad multi-ref tip')
        git(repo, 'push', 'origin', 'multi-good', 'multi-bad', ok=False)
        git(remote, 'show-ref', '--verify', 'refs/heads/multi-good', ok=False)
        git(remote, 'show-ref', '--verify', 'refs/heads/multi-bad', ok=False)
        write(repo, 'bad.rbel.md', VALID)
        commit(repo, 'Repair multi-ref tip')
        git(repo, 'push', '-q', 'origin', 'multi-good', 'multi-bad')
        git(repo, 'checkout', '-q', 'main')
        write(repo, 'valid.rbel.md', VALID + '\nMixed update and deletion.\n')
        commit(repo, 'Mixed reference update')
        git(repo, 'push', '-q', 'origin', 'main', ':multi-good', ':multi-bad')
        groups.append('multiple updates and mixed update/deletions inspected')

        literal = 'odd [*] $(touch SHOULD_NOT_EXIST).rbel.md'
        write(repo, literal, VALID)
        commit(repo, 'Literal metacharacter filename')
        git(repo, 'push', '-q', 'origin', 'main')
        write(repo, literal, MALFORMED)
        commit(repo, 'Malformed literal filename')
        result = git(repo, 'push', 'origin', 'main', ok=False)
        if literal not in result.stdout + result.stderr:
            raise AssertionError('literal source filename absent from diagnostic')
        if (repo / 'SHOULD_NOT_EXIST').exists():
            raise AssertionError('source filename was interpreted by a shell')
        write(repo, literal, VALID)
        commit(repo, 'Repair literal filename')
        git(repo, 'push', '-q', 'origin', 'main')
        groups.append('metacharacter paths treated literally')

        # A second local writer creates a remote tip absent from the hook repo.
        writer = area / 'other writer'
        git(area, 'clone', '-q', remote, writer)
        git(writer, 'config', 'user.name', 'Other Local Writer')
        git(writer, 'config', 'user.email', 'other@example.invalid')
        git(writer, 'checkout', '-qb', 'foreign', 'origin/main')
        write(writer, 'foreign.txt', 'This commit is absent from the original clone.\n')
        commit(writer, 'Unknown remote object')
        git(writer, 'push', '-q', 'origin', 'foreign')
        unknown = git(writer, 'rev-parse', 'HEAD').stdout.strip()
        git(repo, 'cat-file', '-e', unknown, ok=False)
        git(repo, 'checkout', '-qb', 'missing-remote', 'main')
        write(repo, 'fallback.rbel.md', MALFORMED)
        commit(repo, 'Malformed fallback candidate')
        result = git(repo, 'push', '--force', 'origin', 'missing-remote:foreign', ok=False)
        if 'fallback.rbel.md' not in result.stdout + result.stderr:
            raise AssertionError('unavailable remote object did not lint proposed tip')
        write(repo, 'fallback.rbel.md', VALID)
        commit(repo, 'Repair fallback candidate')
        git(repo, 'push', '-q', '--force', 'origin', 'missing-remote:foreign')
        git(repo, 'checkout', '-q', 'main')
        groups.append('unavailable remote object falls back to scanning proposed tip')

        common = Path(git(repo, 'rev-parse', '--git-common-dir').stdout.strip())
        if not common.is_absolute():
            common = repo / common
        installed = common / 'hooks/.rbel-lint/rbel'
        if not installed.is_file():
            raise AssertionError(f'installer did not create expected private tool: {installed}')
        held = installed.with_name('rbel-held')
        installed.rename(held)
        write(repo, 'valid.rbel.md', VALID + '\nAnother committed change.\n')
        commit(repo, 'Push while installed tool missing')
        git(repo, 'push', 'origin', 'main', ok=False)
        held.rename(installed)
        git(repo, 'push', '-q', 'origin', 'main')
        groups.append('missing bundled tool blocks; restoring tool recovers')

        # Replacement views must not hide bytes Git actually transfers.
        base = git(repo, 'rev-parse', 'main').stdout.strip()
        git(repo, 'checkout', '-qb', 'replacement-fixture', 'main')
        write(repo, 'replacement.rbel.md', MALFORMED)
        commit(repo, 'Malformed original object')
        original = git(repo, 'rev-parse', 'HEAD').stdout.strip()
        write(repo, 'replacement.rbel.md', VALID)
        git(repo, 'add', '-A')
        valid_tree = git(repo, 'write-tree').stdout.strip()
        replacement = git(repo, 'commit-tree', valid_tree, '-p', base,
                          '-m', 'Valid sibling replacement view').stdout.strip()
        git(repo, 'reset', '--hard', original)
        git(repo, 'replace', original, replacement)
        git(repo, 'branch', 'bad-tip', original)
        git(repo, 'checkout', '-qf', 'main')
        result = git(repo, 'push', 'origin', 'bad-tip', ok=False)
        if 'replacement.rbel.md' not in result.stdout + result.stderr:
            raise AssertionError('Git replacement hid malformed original blob from lint')
        git(remote, 'show-ref', '--verify', 'refs/heads/bad-tip', ok=False)
        git(repo, 'replace', '-d', original)
        groups.append('Git replacement refs cannot hide original pushed source')

        custom, custom_remote = area / 'custom', area / 'custom.git'
        git(area, 'init', '--bare', '-q', custom_remote)
        git(area, 'clone', '-q', custom_remote, custom)
        hook = custom / '.git/hooks/pre-push'
        original = b'#!/bin/sh\n# Custom hook must survive\nexit 0\n'
        hook.write_bytes(original)
        hook.chmod(0o755)
        install(custom, binary, ok=False)
        if hook.read_bytes() != original:
            raise AssertionError('installer overwrote an existing custom pre-push hook')
        groups.append('installer preserves existing custom hook')
        hook.unlink()
        custom_hooks = area / 'custom hooks path'
        custom_hooks.mkdir()
        git(custom, 'config', 'core.hooksPath', custom_hooks)
        install(custom, binary, ok=False)
        if (custom_hooks / 'pre-push').exists() or hook.exists():
            raise AssertionError('refused hooksPath installation wrote a hook')
        groups.append('installer refuses configured core.hooksPath')

    evidence = output / 'rbel-pre-push-verification.json'
    evidence.write_text(json.dumps({
        'status': 'passed',
        'groups': groups,
        'group_count': len(groups),
        'transport': 'real git push to disposable local bare remotes',
        'rbel_sha256': hashlib.sha256(binary.read_bytes()).hexdigest(),
        'installer_sha256': hashlib.sha256(INSTALLER.read_bytes()).hexdigest(),
        'hook_sha256': hashlib.sha256((ROOT / 'scripts/rbel-pre-push.py').read_bytes()).hexdigest(),
    }, indent=2) + '\n')
    print(f'Passed {len(groups)} local pre-push integration groups:')
    for group in groups:
        print(f'  {group}')


if __name__ == '__main__':
    main()
