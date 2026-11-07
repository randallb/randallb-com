#!/usr/bin/env python3
"""Exercise a real named source coworker beside builtin uppercase."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import select
import shutil
import subprocess
import tempfile
import time

ROOT = Path(__file__).resolve().parents[2]


def record(value):
    return dict(value or [])


def text(value):
    return ''.join(c['char'] for c in value or [])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--executable', type=Path, default=ROOT / 'target/libcwr-embedding/libcwr')
    parser.add_argument('--rbel', type=Path, default=ROOT / 'target/rbel-command/bin/rbel')
    parser.add_argument('--report', type=Path, default=ROOT / 'target/named-source-verification/results.json')
    args = parser.parse_args()
    binary, compiler = args.executable.resolve(), args.rbel.resolve()
    report = {'passed': False, 'groups': [], 'transcript': [],
              'executable_sha256': hashlib.sha256(binary.read_bytes()).hexdigest()}
    args.report.parent.mkdir(parents=True, exist_ok=True)

    def check(name, truth):
        if not truth:
            raise AssertionError(name)
        report['groups'].append(name)
        print('PASS', name, flush=True)

    class Session:
        def __init__(self, command, cwd):
            self.process = subprocess.Popen(list(map(str, command)), cwd=cwd, stdin=subprocess.PIPE,
                                            stdout=subprocess.PIPE, stderr=subprocess.STDOUT, bufsize=0)
            self.output = b''
            self.offset = 0

        def until(self, expected, timeout=150):
            marker = expected.encode()
            deadline = time.monotonic() + timeout
            while marker not in self.output[self.offset:]:
                if time.monotonic() >= deadline:
                    raise AssertionError(('timeout', expected, self.output[-6000:]))
                if select.select([self.process.stdout], [], [], .1)[0]:
                    chunk = os.read(self.process.stdout.fileno(), 65536)
                    self.output += chunk
                    if not chunk and self.process.poll() is not None:
                        raise AssertionError(('early exit', self.process.returncode, self.output))
            end = self.output.index(marker, self.offset) + len(marker)
            answer = self.output[self.offset:end]
            self.offset = end
            return answer.decode(errors='replace')

        def send(self, line):
            self.process.stdin.write((line + '\n').encode())
            self.process.stdin.flush()

        def command(self, line, prompt='you[demo]>'):
            self.send(line)
            return self.until(prompt)

        def close(self, force=False):
            if self.process.poll() is None:
                if force:
                    self.process.kill()
                else:
                    self.send('/exit')
            extra, _ = self.process.communicate(timeout=20)
            self.output += extra
            report['transcript'].append({'output': self.output.decode(errors='replace'),
                                         'exit': self.process.returncode, 'forced': force})
            if not force:
                assert self.process.returncode == 0, self.output

    session = None
    try:
        with tempfile.TemporaryDirectory(prefix='rbel named source ') as temporary:
            work = Path(temporary).resolve()
            executable = work / 'libcwr'
            shutil.copy2(binary, executable)
            state = work / 'saved workspace'
            source = work / 'greeter.rbel.md'
            original = (ROOT / 'libcwr/coworkers/greeter.rbel.md').read_text()
            source.write_text(original)
            command = [executable, 'workspace', '--directory', state, '--coworker', 'greeter',
                       '--source', source, '--rbel', compiler]

            def snapshot():
                return record(json.loads((state / 'state.json').read_text()))

            def conversations():
                return [record(c) for c in snapshot().get('conversations') or []]

            def room():
                return next(c for c in conversations() if c['ref'].endswith('/demo'))

            def invocations():
                saved = snapshot()
                return [record(v) for key in ['invocations', 'workspace-invocations']
                        for v in saved.get(key) or []]

            def reply(who, body, expected):
                session.send('@' + who + ' ' + body)
                session.until(who + ' [demo]: ' + expected)
                session.until('you[demo]>')
                messages = [record(v) for v in room()['messages']]
                found = next(m for m in messages if m.get('reply') and text(m.get('text')) == expected
                             and m['author'].endswith('/' + who))
                source_message = next(m for m in messages if m['ref'] == found['reply'])
                invocation = next(i for i in invocations() if i['ref'] == found['invocation'])
                return (found, source_message, invocation)

            session = Session(command, work)
            session.until('workspace>')
            output = session.command('/coworkers', 'workspace>')
            check('named-source-and-builtin-have-distinct-directory-identities', '- greeter (' in output
                  and '- uppercase (' in output)
            output = session.command('/actions', 'workspace>')
            check('named-source-exposes-discoverable-source-action', 'source--text' in output)
            session.command('/new demo', 'you[demo]>')
            output = session.command('/invite greeter')
            check('invitation-joins-without-starting-source-work', 'invite: invited' in output
                  and not any(i.get('target', [None] * 4)[3] == 'source--text' for i in invocations()))
            greeter, message, invocation = reply('greeter', 'Randall', 'Hello, Randall')
            check('greeter-runs-real-source-with-author-and-causal-links', text(message['text']) == '@greeter Randall'
                  and invocation['phase'] == 'completed' and invocation['target'][1] == greeter['author']
                  and invocation['target'][3] == 'source--text')
            uppercase, message, invocation = reply('uppercase', 'Randall', 'RANDALL')
            check('builtin-uppercase-remains-separate-from-configured-source', uppercase['author'] != greeter['author']
                  and text(message['text']) == '@uppercase Randall' and invocation['target'][3] == 'uppercase--text')
            retained = conversations()
            source.write_text(original.replace('Hello, ', 'Welcome, '))
            output = session.command('/check')
            check('check-validates-named-source-without-changing-conversation-facts', 'check: completed' in output
                  and conversations() == retained)
            reply('greeter', 'before reload', 'Hello, before reload')
            check('successful-check-does-not-activate-source-edit', True)
            output = session.command('/reload')
            check('reload-activates-named-source', 'reload: completed' in output)
            reply('greeter', 'after reload', 'Welcome, after reload')
            reply('uppercase', 'after reload', 'AFTER RELOAD')
            check('named-source-reload-leaves-builtin-implementation-unchanged', True)
            broken = original.replace('"Hello, Randall"', '"wrong greeting"')
            source.write_text(broken)
            output = session.command('/check')
            check('named-source-check-reports-example-location', 'check: failed' in output
                  and 'greeter-person' in output and str(source) + ':' in output)
            output = session.command('/reload')
            check('named-source-failed-reload-retains-last-good-build', 'reload: failed' in output)
            reply('greeter', 'after failure', 'Welcome, after failure')
            reply('uppercase', 'still independent', 'STILL INDEPENDENT')
            check('failed-source-check-and-reload-do-not-change-either-coworker', True)
            source.write_text(original.replace('Hello, ', 'Welcome, '))
            retained = conversations()
            old_context = record(json.loads((state / 'context.json').read_text()))
            target = next(record(row)['target'] for row in old_context['workspace-actions']
                          if record(row).get('name') == 'greeter')
            session.close()
            session = Session(command, work)
            session.until('workspace>')
            check('restart-preserves-named-identity-and-shared-history', conversations() == retained
                  and next(record(row)['target'] for row in record(json.loads((state / 'context.json').read_text()))['workspace-actions']
                           if record(row).get('name') == 'greeter') == target)
            session.command('/open demo')
            reply('greeter', 'again', 'Welcome, again')
            reply('uppercase', 'again', 'AGAIN')
            check('restart-routes-both-coworkers-correctly', True)
            session.close()
            session = None

            # A trusted caller without the exact action grant cannot bypass
            # admission through the raw transport or acquire participation.
            environment = record(json.loads((state / 'context.json').read_text()))
            row = next(record(v) for v in environment['workspace-actions'] if record(v).get('name') == 'greeter')
            denied = dict(environment, allow=None, binding=list(row.items()))
            trusted = work / 'denied caller.json'
            trusted.write_text(json.dumps(list(denied.items())))
            event = {'op': 'admit', 'ref': environment['context'] + '/invocations/denied-greeter',
                     'origin': environment['context'] + '/requests/denied-greeter',
                     'conversation': room()['ref'], 'message': environment['context'] + '/messages/denied-greeter',
                     'response-message': environment['context'] + '/replies/denied-greeter',
                     'target': row['target'], 'input': [{'char': c} for c in 'denied'],
                     'text': [{'char': c} for c in '@greeter denied']}
            before = (state / 'state.json').read_bytes()
            result = subprocess.run([executable, '--directory', state, '--context', trusted],
                                    input=json.dumps(list(event.items())) + '\n', text=True,
                                    capture_output=True, cwd=work, timeout=20)
            report['transcript'].append({'raw': event, 'stdout': result.stdout, 'stderr': result.stderr,
                                         'exit': result.returncode})
            check('named-source-admission-requires-exact-current-authority', result.returncode == 0
                  and json.loads(result.stdout)[0] == 'denied'
                  and (state / 'state.json').read_bytes() == before)

            # The same source action must execute through authorized JSONL,
            # without relying on terminal startup to compile its worker first.
            authorized = dict(environment, allow=[row['target']], binding=list(row.items()))

            def raw(events):
                trusted.write_text(json.dumps(list(authorized.items())))
                launch = [executable, '--directory', state, '--context', trusted,
                          '--source', source, '--rbel', compiler]
                result = subprocess.run(launch,
                                        input=''.join(json.dumps(list(e.items())) + '\n' for e in events),
                                        text=True, capture_output=True, cwd=work, timeout=150)
                report['transcript'].append({'raw_events': events, 'stdout': result.stdout,
                                             'stderr': result.stderr, 'exit': result.returncode})
                assert result.returncode == 0, result.stderr
                return [json.loads(line) for line in result.stdout.splitlines()]

            def source_request(suffix, body):
                return dict(event, ref=environment['context'] + '/invocations/' + suffix,
                            origin=environment['context'] + '/requests/' + suffix,
                            message=environment['context'] + '/messages/' + suffix,
                            **{'response-message': environment['context'] + '/replies/' + suffix},
                            input=[{'char': c} for c in body],
                            text=[{'char': c} for c in '@greeter ' + body])

            def executions():
                path = state / 'executions.jsonl'
                return path.read_text().splitlines() if path.exists() else []

            raw_request = source_request('raw-greeter', 'Raw caller')
            raw_run = {'op': 'run', 'ref': raw_request['ref']}
            answers = raw([raw_request, raw_run])
            raw_invocation = next(i for i in invocations() if i['ref'] == raw_request['ref'])
            messages = [record(m) for m in room()['messages']]
            raw_reply = next(m for m in messages if m.get('invocation') == raw_request['ref'] and m.get('reply'))
            check('authorized-raw-source-run-builds-and-executes-configured-worker',
                  answers[0][0] == 'admitted' and answers[1][0] == 'completed'
                  and raw_invocation['phase'] == 'completed'
                  and text(raw_invocation['result']) == 'Welcome, Raw caller')
            check('raw-source-reply-retains-requested-author-and-links',
                  raw_reply['ref'] == raw_request['response-message']
                  and raw_reply['reply'] == raw_request['message']
                  and raw_reply['author'] == row['target'][1]
                  and text(raw_reply['text']) == 'Welcome, Raw caller')
            recorded_executions = executions()
            retained_messages = room()['messages']
            replay = raw([raw_run, raw_request, raw_run])
            check('raw-source-completion-replays-after-restart-without-reexecution',
                  replay[0][0] == 'completed' and replay[1][0] in ['existing', 'completed']
                  and replay[2][0] == 'completed' and executions() == recorded_executions
                  and room()['messages'] == retained_messages)

            source.write_text(broken)
            bad_request = source_request('raw-greeter-bad-build', 'Never executed')
            answers = raw([bad_request, {'op': 'run', 'ref': bad_request['ref']}])
            bad_invocation = next(i for i in invocations() if i['ref'] == bad_request['ref'])
            check('raw-source-initial-build-failure-retains-diagnostics-without-fallback',
                  answers[0][0] == 'admitted' and answers[1][0] == 'reconciliation-required'
                  and bad_invocation.get('uncertain') and not bad_invocation.get('attempt')
                  and 'greeter-person' in text(bad_invocation.get('error'))
                  and str(source) + ':' in text(bad_invocation.get('error'))
                  and executions() == recorded_executions
                  and not any(record(m).get('ref') == bad_request['response-message'] for m in room()['messages']))
            replay = raw([{'op': 'run', 'ref': bad_request['ref']}])
            check('raw-source-failed-build-is-not-silently-retried-after-restart',
                  replay[0][0] == 'reconciliation-required' and executions() == recorded_executions)
            source.write_text(original.replace('Hello, ', 'Welcome, '))

            before = {name: (state / name).read_bytes() for name in ['context.json', 'state.json']}
            for name in ['uppercase', 'repl', 'language', 'bad/name']:
                bad = [executable, 'workspace', '--directory', state, '--coworker', name,
                       '--source', source, '--rbel', compiler]
                result = subprocess.run(bad, input='/exit\n', text=True, capture_output=True, timeout=150)
                check('invalid-or-reserved-name-is-rejected-before-state-change-' + name.replace('/', '-'),
                      result.returncode != 0 and all((state / path).read_bytes() == value
                                                    for path, value in before.items()))
            report['passed'] = True
    finally:
        if session is not None:
            session.close(force=True)
        args.report.write_text(json.dumps(report, indent=2) + '\n')
    print(f"{len(report['groups'])} named source acceptance checks passed")


if __name__ == '__main__':
    main()
