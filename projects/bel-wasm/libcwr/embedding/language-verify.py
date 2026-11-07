#!/usr/bin/env python3
"""Exercise source checking, retained diagnostics and safe activation with real rbel."""
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
    if isinstance(value, str):
        return value
    return ''.join(item['char'] for item in value or [])


def result_text(invocation):
    retained = invocation.get('result')
    return text(retained[1]) if retained else ''


def document(name, definition, expression, expected):
    return (f'```rbel definition={name}\n{definition}\n```\n\n'
            f'```rbel example={name}-example\n{expression}\n=>\n{expected}\n```\n')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--executable', type=Path, default=ROOT / 'target/libcwr-embedding/libcwr')
    parser.add_argument('--rbel', type=Path, default=ROOT / 'target/rbel-command/bin/rbel')
    parser.add_argument('--report', type=Path, default=ROOT / 'target/language-verification/results.json')
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
            deadline = time.monotonic() + timeout
            marker = expected.encode()
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

        def command(self, line, prompt='workspace>'):
            self.send(line)
            return self.until(prompt)

        def close(self, force=False):
            if self.process.poll() is None:
                if force:
                    self.process.kill()
                else:
                    self.send('/exit')
            out, _ = self.process.communicate(timeout=20)
            self.output += out
            report['transcript'].append({'stdout': self.output.decode(errors='replace'),
                                         'exit': self.process.returncode, 'forced': force})
            if not force:
                assert self.process.returncode == 0, self.output

    session = None
    try:
        with tempfile.TemporaryDirectory(prefix='rbel language ') as temporary:
            work = Path(temporary)
            executable = work / 'libcwr'
            shutil.copy2(binary, executable)
            state = work / 'saved workspace'
            support = work / 'support'
            support.mkdir()
            helper = support / 'helper.rbel.md'
            source = work / 'worker.rbel.md'

            def write_source(value='OLD', helper_expected=None):
                helper.write_text(document('helper-transform', f'(def helper-transform (xs) "{value}")',
                                           '(helper-transform "sample")', f'"{helper_expected or value}"'))
                source.write_text(document('source-worker', '(import "support/helper.rbel.md")\n'
                                           '(def source-worker (xs) (helper-transform xs))',
                                           '(source-worker "sample")', f'"{value}"'))

            def snapshot():
                return record(json.loads((state / 'state.json').read_text()))

            def invocations():
                saved = snapshot()
                return [record(v) for key in ['invocations', 'workspace-invocations']
                        for v in saved.get(key) or []]

            def language_invocations(action='check'):
                return [v for v in invocations() if v.get('target', [None] * 4)[3] == 'language--do--' + action]

            def saved_conversations():
                return [record(v) for v in snapshot().get('conversations') or []]

            def worker_reply(expected):
                session.send('@uppercase sample')
                session.until('uppercase [notes]: ' + expected)
                session.until('you[notes]>')

            command = [executable, 'workspace', '--directory', state, '--source', source, '--rbel', compiler]
            write_source()
            session = Session(command, work)
            session.until('workspace>')
            output = session.command('/actions')
            check('check-is-a-discoverable-exact-language-action', 'language--do--check' in output)
            output = session.command('/help')
            check('help-advertises-source-check', '/check' in output)
            output = session.command('/check')
            checks = language_invocations()
            check('home-check-completes-without-creating-a-conversation', 'check: completed' in output
                  and not saved_conversations() and len(checks) == 1 and checks[0]['phase'] == 'completed')
            check('successful-check-retains-readable-result', bool(result_text(checks[0])))
            session.command('/new notes', 'you[notes]>')
            worker_reply('OLD')
            before = saved_conversations()
            write_source('NEW')
            output = session.command('/check', 'you[notes]>')
            check('changed-source-and-relative-import-examples-pass-check', 'check: completed' in output)
            check('check-does-not-change-history-or-participation', saved_conversations() == before)
            worker_reply('OLD')
            check('check-does-not-activate-the-new-worker', True)
            output = session.command('/reload', 'you[notes]>')
            check('reload-validates-and-activates-source', 'reload: completed' in output)
            worker_reply('NEW')
            write_source('BROKEN', 'WRONG')
            output = session.command('/check', 'you[notes]>')
            failed = language_invocations()[-1]
            failure_text = result_text(failed)
            check('import-example-failure-is-readable-with-source-location', 'check: failed' in output
                  and str(helper) + ':5:' in output and 'helper-transform-example' in output)
            check('failed-check-retains-source-diagnostics-in-invocation', failed['phase'] == 'failed'
                  and str(helper) + ':5:' in failure_text and 'helper-transform-example' in failure_text)
            worker_reply('NEW')
            output = session.command('/reload', 'you[notes]>')
            check('failed-reload-does-not-activate-broken-source', 'reload: failed' in output)
            worker_reply('NEW')
            check('failed-check-and-reload-keep-last-working-worker', True)
            output = session.command('/check unexpected', 'you[notes]>')
            check('check-rejects-unexpected-input-before-admission', 'unexpected command argument' in output
                  and len(language_invocations()) == 3)
            write_source('NEW')
            retained = saved_conversations()
            session.close()
            # Simulate a saved workspace produced before the check action existed.
            legacy_environment = record(json.loads((state / 'context.json').read_text()))
            legacy_catalog = [record(v) for v in legacy_environment['workspace-actions']]
            check_target = next(v['target'] for v in legacy_catalog if v['target'][3] == 'language--do--check')
            legacy_environment['workspace-actions'] = [list(v.items()) for v in legacy_catalog
                                                       if v['target'] != check_target]
            legacy_environment['allow'] = [v for v in legacy_environment['allow'] if v != check_target]
            (state / 'context.json').write_text(json.dumps(list(legacy_environment.items())))
            session = Session(command, work)
            session.until('workspace>')
            check('restart-preserves-history-and-membership', saved_conversations() == retained)
            output = session.command('/check')
            check('existing-workspace-gains-check-with-equivalent-reload-authority', 'check: completed' in output)
            session.close()
            session = None

            # Trusted context fixtures model a second authorized coworker.
            # No caller identity or grant is accepted from the event itself.
            environment = record(json.loads((state / 'context.json').read_text()))
            catalog = [record(v) for v in environment['workspace-actions']]
            action = next(v for v in catalog if v['target'][3] == 'language--do--check')
            bob = environment['context'] + '/coworkers/test-bob'
            environment['caller'] = bob
            environment['available'] = list(dict.fromkeys([*environment['available'], bob]))
            environment['allow'] = [action['target']]
            environment['binding'] = [[k, v] for k, v in action.items() if k != 'name']
            trusted = work / 'trusted caller.json'

            def raw(event, env, configured=False):
                trusted.write_text(json.dumps(list(env.items())))
                launcher = [executable, '--directory', state, '--context', trusted]
                if configured:
                    launcher += ['--source', source, '--rbel', compiler]
                result = subprocess.run(launcher,
                                        input=json.dumps(list(event.items())) + '\n', text=True,
                                        capture_output=True, cwd=work, timeout=150)
                report['transcript'].append({'raw': event, 'stdout': result.stdout, 'stderr': result.stderr,
                                             'exit': result.returncode})
                assert result.returncode == 0, result.stderr
                return json.loads(result.stdout)

            request = {'op': 'workspace-call', 'ref': environment['context'] + '/invocations/bob-check',
                       'origin': environment['context'] + '/requests/bob-check',
                       'target': action['target'], 'input': None}
            before = len(invocations())
            denied = raw(request, dict(environment, allow=None), configured=True)
            check('nonterminal-check-obeys-current-caller-authority', denied[0] == 'denied'
                  and len(invocations()) == before)
            answer = raw(request, environment)
            retained_check = next(v for v in invocations() if v['ref'] == request['ref'])
            check('authorized-nonterminal-check-settles-and-retains-unconfigured-diagnostic',
                  answer[0] == 'failed' and retained_check['phase'] == 'failed'
                  and 'source' in result_text(retained_check).lower()
                  and retained_check['author'] == bob)
            count = len(invocations())
            replay = raw(request, environment)
            check('same-check-request-recovers-original-retained-result', replay == retained_check['result']
                  and len(invocations()) == count)
            write_source('RAW')
            configured_request = dict(request, ref=environment['context'] + '/invocations/bob-configured-check',
                                      origin=environment['context'] + '/requests/bob-configured-check')
            answer = raw(configured_request, environment, configured=True)
            retained_check = next(v for v in invocations() if v['ref'] == configured_request['ref'])
            check('authorized-nonterminal-check-runs-configured-source-and-examples', answer[0] == 'completed'
                  and retained_check['phase'] == 'completed' and retained_check['author'] == bob)
            write_source('RAW', 'WRONG')
            bad_request = dict(request, ref=environment['context'] + '/invocations/bob-bad-check',
                               origin=environment['context'] + '/requests/bob-bad-check')
            answer = raw(bad_request, environment, configured=True)
            retained_check = next(v for v in invocations() if v['ref'] == bad_request['ref'])
            check('nonterminal-check-retains-import-failure-locations', answer[0] == 'failed'
                  and str(helper) + ':5:' in result_text(retained_check)
                  and 'helper-transform-example' in result_text(retained_check))
            report['passed'] = True
    finally:
        if session is not None:
            session.close(force=True)
        args.report.write_text(json.dumps(report, indent=2) + '\n')
    print(f"{len(report['groups'])} language acceptance checks passed")


if __name__ == '__main__':
    main()
