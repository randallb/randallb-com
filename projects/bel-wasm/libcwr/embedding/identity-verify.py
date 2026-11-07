#!/usr/bin/env python3
"""Exercise fresh hostname identities and preservation of saved local identities."""
import argparse
import hashlib
import json
import platform
from pathlib import Path
import re
import shutil
import socket
import subprocess
import tempfile
import uuid

ROOT = Path(__file__).resolve().parents[2]


def record(value):
    return dict(value or [])


def strings(value):
    if isinstance(value, str):
        yield value
    elif isinstance(value, (list, tuple)):
        for item in value:
            yield from strings(item)
    elif isinstance(value, dict):
        for item in value.values():
            yield from strings(item)


def replace_root(value, before, after):
    if isinstance(value, str):
        return after + value[len(before):] if value == before or value.startswith(before + '/') else value
    if isinstance(value, list):
        return [replace_root(item, before, after) for item in value]
    return value


def normalized_host():
    host = socket.gethostname()
    if platform.system() == 'Darwin':
        result = subprocess.run(['/usr/sbin/scutil', '--get', 'LocalHostName'],
                                capture_output=True, text=True, timeout=5)
        if result.returncode == 0 and result.stdout.strip():
            host = result.stdout
    short = host.strip().split('.')[0]
    return re.sub('[^a-z0-9-]', '-', short.lower(), flags=re.ASCII).strip('-')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--executable', type=Path, default=ROOT / 'target/libcwr-embedding/libcwr')
    parser.add_argument('--legacy-executable', type=Path,
                        default=ROOT.parents[2] / 'shared-preview/libcwr-before-language-check')
    parser.add_argument('--report', type=Path, default=ROOT / 'target/identity-verification/results.json')
    args = parser.parse_args()
    binary = args.executable.resolve()
    report = {'passed': False, 'groups': [], 'transcript': [],
              'executable_sha256': hashlib.sha256(binary.read_bytes()).hexdigest()}
    args.report.parent.mkdir(parents=True, exist_ok=True)

    def check(name, truth):
        if not truth:
            raise AssertionError(name)
        report['groups'].append(name)
        print('PASS', name, flush=True)

    def run(executable, *arguments, lines='', success=True):
        result = subprocess.run([str(executable), *map(str, arguments)], input=lines,
                                capture_output=True, text=True, timeout=40)
        report['transcript'].append({'arguments': list(map(str, arguments)),
                                     'stdout': result.stdout, 'stderr': result.stderr,
                                     'exit': result.returncode})
        if success and result.returncode:
            raise AssertionError((arguments, result.returncode, result.stderr))
        return result

    def context(state):
        return record(json.loads((state / 'context.json').read_text()))

    def state_value(state):
        return record(json.loads((state / 'state.json').read_text()))

    try:
        with tempfile.TemporaryDirectory(prefix='libcwr identity ') as temporary:
            work = Path(temporary)
            executable = work / 'libcwr'
            shutil.copy2(binary, executable)
            first, second = work / 'first', work / 'second'
            run(executable, 'init', '--directory', first)
            run(executable, 'init', '--directory', second)
            one, two = context(first), context(second)
            runtime = one['runtime']
            shape = re.fullmatch(r'cwp://local\.([a-z0-9-]{1,63})/runtimes/([0-9a-f-]{36})', runtime)
            check('fresh-runtime-uses-hostname-and-canonical-uuid', bool(shape)
                  and shape[1] == normalized_host() and str(uuid.UUID(shape[2])) == shape[2]
                  and uuid.UUID(shape[2]).version == 4)
            check('separate-workspaces-have-distinct-runtime-identities', runtime != two['runtime'])
            chat_state = work / 'chat'
            output = run(executable, 'chat', '--directory', chat_state, lines='hello identity\n/exit\n')
            chat_root = context(chat_state)['runtime']
            chat_refs = [s for s in strings(state_value(chat_state)) if s.startswith('cwp://')]
            check('legacy-chat-mode-creates-and-links-refs-under-fresh-runtime-root',
                  chat_root.startswith('cwp://local.' + normalized_host() + '/runtimes/')
                  and bool(chat_refs) and all(s == chat_root or s.startswith(chat_root + '/') for s in chat_refs)
                  and 'HELLO IDENTITY' in output.stdout)
            check('fresh-context-refs-share-the-runtime-root',
                  all(value == runtime or value.startswith(runtime + '/')
                      for value in strings(list(one.items())) if value.startswith('cwp://')))
            saved = (first / 'context.json').read_bytes()
            denied = run(executable, 'init', '--directory', first, success=False)
            check('repeat-init-preserves-existing-identity', denied.returncode != 0
                  and (first / 'context.json').read_bytes() == saved)

            output = run(executable, 'workspace', '--directory', first,
                         lines='/new notes\nretained identity note\n/check\n/status\n/exit\n')
            saved_context = (first / 'context.json').read_bytes()
            saved_state = (first / 'state.json').read_bytes()
            snapshot = state_value(first)
            refs = [s for s in strings(list(snapshot.items())) if s.startswith('cwp://')]
            check('conversation-message-invocation-and-claim-refs-use-saved-root',
                  bool(refs) and all(s == runtime or s.startswith(runtime + '/') for s in refs)
                  and any('/claims/' in s for s in refs))
            run(executable, 'workspace', '--directory', first, lines='')
            check('restart-keeps-identities-authority-and-history-exact',
                  (first / 'context.json').read_bytes() == saved_context
                  and (first / 'state.json').read_bytes() == saved_state)
            moved = work / 'moved'
            shutil.move(first, moved)
            run(executable, 'workspace', '--directory', moved, lines='')
            check('moving-workspace-keeps-saved-context-and-history-exact',
                  (moved / 'context.json').read_bytes() == saved_context
                  and (moved / 'state.json').read_bytes() == saved_state)

            # A trusted saved identity from a differently named machine models a
            # rename without changing this computer's system configuration.
            original = two['runtime']
            foreign = 'cwp://local.saved-before-rename/runtimes/' + str(uuid.uuid4())
            identity_fixture = replace_root(json.loads((second / 'context.json').read_text()), original, foreign)
            (second / 'context.json').write_text(json.dumps(identity_fixture))
            run(executable, 'workspace', '--directory', second, lines='/new after-rename\n/check\n/exit\n')
            check('different-current-hostname-does-not-replace-persisted-runtime-root',
                  context(second)['runtime'] == foreign)
            all_saved = [*strings(json.loads((second / 'context.json').read_text())),
                         *strings(json.loads((second / 'state.json').read_text()))]
            check('new-work-and-claims-follow-persisted-root-after-hostname-change',
                  all(s == foreign or s.startswith(foreign + '/')
                      for s in all_saved if s.startswith('cwp://')))

            # Generate actual prior-version state when the delivered old binary
            # is available. Otherwise create a trusted legacy context fixture.
            legacy = work / 'legacy'
            if args.legacy_executable.is_file():
                old = work / 'old-libcwr'
                shutil.copy2(args.legacy_executable, old)
                report['legacy_executable_sha256'] = hashlib.sha256(old.read_bytes()).hexdigest()
                run(old, 'workspace', '--directory', legacy,
                    lines='/new retained\nlegacy attributed note\n/exit\n')
            else:
                run(executable, 'workspace', '--directory', legacy,
                    lines='/new retained\nlegacy attributed note\n/exit\n')
                before = context(legacy)['runtime']
                old_root = 'cwp://org.example.libcwr/local/' + str(uuid.uuid4())
                for name in ['context.json', 'state.json']:
                    path = legacy / name
                    path.write_text(json.dumps(replace_root(json.loads(path.read_text()), before, old_root)))
            old_context = context(legacy)
            old_state = (legacy / 'state.json').read_bytes()
            old_runtime = context(legacy)['runtime']
            check('legacy-fixture-retains-example-authority', old_runtime.startswith('cwp://org.example.libcwr/'))
            run(executable, 'workspace', '--directory', legacy, lines='')
            check('upgraded-legacy-workspace-is-not-automatically-renamed',
                  all(context(legacy).get(k) == v for k, v in old_context.items()
                      if k not in ['allow', 'workspace-actions'])
                  and all(v in context(legacy).get('allow', []) for v in old_context.get('allow') or [])
                  and (legacy / 'state.json').read_bytes() == old_state)
            output = run(executable, 'workspace', '--directory', legacy,
                         lines='/open retained\n/history\n/check\n/exit\n')
            check('legacy-history-remains-readable-under-original-authority',
                  'legacy attributed note' in output.stdout and context(legacy)['runtime'] == old_runtime)
            latest = state_value(legacy)
            prior_refs = set(strings(json.loads(old_state)))
            new_claims = [s for s in strings(list(latest.items())) if '/claims/' in s and s not in prior_refs]
            check('new-legacy-session-claims-use-its-existing-runtime-root',
                  bool(new_claims) and all(s.startswith(old_runtime + '/claims/') for s in new_claims))
            report['passed'] = True
    finally:
        args.report.write_text(json.dumps(report, indent=2) + '\n')
    print(f"Identity acceptance: {len(report['groups'])} groups passed")


if __name__ == '__main__':
    main()
