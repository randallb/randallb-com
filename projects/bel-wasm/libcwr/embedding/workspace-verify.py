#!/usr/bin/env python3
"""Independent native acceptance for workspace-first coworker conversations."""
import argparse
import hashlib
import json
import os
import pty
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
    if value is None:
        return ''
    if isinstance(value, str):
        return value
    return ''.join(item['char'] for item in value)


def document(name, body, expression, expected):
    return (f'```rbel definition={name}\n{body}\n```\n'
            f'```rbel example={name}-example\n{expression}\n=>\n{expected}\n```\n')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--executable', type=Path, default=ROOT / 'target/libcwr-embedding/libcwr')
    parser.add_argument('--rbel', type=Path, default=ROOT / 'target/rbel-command/bin/rbel')
    parser.add_argument('--report', type=Path, default=ROOT / 'target/workspace-verification/results.json')
    args = parser.parse_args()
    executable, compiler = args.executable.resolve(), args.rbel.resolve()
    report = {'passed': False, 'groups': [], 'transcript': [],
              'executable_sha256': hashlib.sha256(executable.read_bytes()).hexdigest()}
    args.report.parent.mkdir(parents=True, exist_ok=True)

    def check(name, truth):
        if not truth:
            raise AssertionError(name)
        report['groups'].append(name)
        print('PASS', name, flush=True)

    class Session:
        def __init__(self, command, cwd):
            self.process = subprocess.Popen(list(map(str, command)), cwd=cwd,
                                            stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                            stderr=subprocess.PIPE, bufsize=0)
            self.stdout, self.stderr = b'', b''
            self.offset = 0

        def drain(self, pause=.05):
            streams = [s for s in [self.process.stdout, self.process.stderr] if not s.closed]
            ready, _, _ = select.select(streams, [], [], pause)
            for stream in ready:
                data = os.read(stream.fileno(), 65536)
                if stream is self.process.stdout:
                    self.stdout += data
                else:
                    self.stderr += data

        def send(self, line):
            self.drain(0)
            self.offset = len(self.stdout)
            self.process.stdin.write(line.encode() + b'\n')
            self.process.stdin.flush()

        def until(self, expected, timeout=15):
            target = expected.encode()
            deadline = time.monotonic() + timeout
            while target not in self.stdout[self.offset:]:
                if time.monotonic() >= deadline:
                    raise AssertionError(('timeout', expected, self.stdout[-4000:], self.stderr[-3000:]))
                self.drain()
                if self.process.poll() is not None:
                    raise AssertionError(('workspace exited', self.process.returncode, expected, self.stdout, self.stderr))
            end = self.stdout.index(target, self.offset) + len(target)
            answer = self.stdout[self.offset:end]
            self.offset = end
            return answer

        def wait(self, condition, timeout=15):
            deadline = time.monotonic() + timeout
            while time.monotonic() < deadline:
                self.drain()
                answer = condition()
                if answer:
                    return answer
                if self.process.poll() is not None:
                    raise AssertionError(('workspace exited', self.process.returncode, self.stdout, self.stderr))
            raise AssertionError(('condition timeout', self.stdout[-4000:], self.stderr[-3000:]))

        def finish(self):
            if self.process.poll() is None:
                self.send('/exit')
            out, err = self.process.communicate(timeout=15)
            self.stdout += out
            self.stderr += err
            report['transcript'].append({'stdout': self.stdout.decode(errors='replace'),
                                         'stderr': self.stderr.decode(errors='replace'),
                                         'exit': self.process.returncode})
            if self.process.returncode:
                raise AssertionError(('workspace exit', self.process.returncode, self.stdout, self.stderr))

        def kill(self):
            if self.process.poll() is None:
                self.process.kill()
                out, err = self.process.communicate()
                self.stdout += out; self.stderr += err
                report['transcript'].append({'stdout': self.stdout.decode(errors='replace'), 'stderr': self.stderr.decode(errors='replace'), 'killed': True})

    session = None
    try:
        with tempfile.TemporaryDirectory(prefix='rbel workspace ') as temporary:
            work = Path(temporary)
            binary = work / 'libcwr'
            shutil.copy2(executable, binary)
            state = work / 'saved workspace'

            def snapshot(directory=state):
                path = directory / 'state.json'
                return record(json.loads(path.read_text())) if path.exists() else {}

            def conversations(directory=state):
                return [record(c) for c in snapshot(directory).get('conversations') or []]

            def conversation(name, directory=state):
                return next((c for c in conversations(directory) if c['ref'].endswith('/' + name)), None)

            def invocations(directory=state):
                value = snapshot(directory)
                return [record(i) for key in ['invocations', 'workspace-invocations']
                        for i in value.get(key) or []]

            def messages(name, directory=state):
                current = conversation(name, directory)
                return [record(m) for m in (current or {}).get('messages') or []]

            def command(directory=state, *extra):
                return [binary, 'workspace', '--directory', directory, *extra]

            session = Session(command(), work)
            session.until('workspace>')
            check('launch-opens-workspace-home-without-a-conversation', not conversations())
            session.send('/actions')
            output = session.until('workspace>')
            check('workspace-discovers-repl-and-language-actions', b'repl' in output.lower() and b'language' in output.lower())
            session.send('/coworkers')
            directory_output = session.until('workspace>')
            available = record(json.loads((state / 'context.json').read_text()))['available']
            check('coworker-directory-lists-unique-currently-available-identities',
                  all(directory_output.count(('(' + ref + ')').encode()) == 1 for ref in available))
            # The directory can contain concrete people/software refs outside
            # the generated /coworkers/ subtree. Handles must resolve to those refs.
            current_context = record(json.loads((state / 'context.json').read_text()))
            legacy_worker = next(ref for ref in available if ref != current_context['caller']
                                 and '/coworkers/' not in ref)
            session.send('/new directory-room')
            session.until('you[directory-room]>')
            session.send('/invite ' + legacy_worker.rsplit('/', 1)[-1])
            session.until('invite: invited')
            session.until('you[directory-room]>')
            check('directory-handle-invites-the-actual-available-identity',
                  legacy_worker in conversation('directory-room')['participants'])
            session.send('/remove ' + legacy_worker)
            session.until('remove: removed')
            session.until('you[directory-room]>')
            check('canonical-directory-reference-removes-the-same-identity',
                  legacy_worker not in conversation('directory-room')['participants'])
            session.send('/home')
            session.until('workspace>')
            session.send('/help')
            session.until('workspace>')
            check('help-is-a-real-audited-repl-action', any(
                i.get('target', [None] * 4)[3] == 'repl--do--help'
                and i.get('phase') == 'completed' for i in invocations()))
            session.send('/new alpha')
            session.until('you[alpha]>')
            controls = invocations()
            check('new-is-an-audited-exact-target-coworker-action', any(
                len(i.get('target') or []) == 6 and 'new' in i['target'][3]
                and i.get('phase') == 'completed' and i.get('author') for i in controls))
            check('new-does-not-add-control-coworker-as-participant', len(conversation('alpha')['participants']) == 1)
            session.send('hello room')
            session.wait(lambda: any(text(m.get('text')) == 'hello room' for m in messages('alpha')))
            check('untargeted-message-publishes-without-work', len(messages('alpha')) == 1
                  and not any(i.get('conversation') == conversation('alpha')['ref']
                              and 'uppercase' in str(i.get('target')) for i in invocations()))
            session.send('/invite uppercase')
            session.until('invite: invited')
            session.until('you[alpha]>')
            check('explicit-invitation-adds-idle-coworker', len(conversation('alpha')['participants']) == 2
                  and len(messages('alpha')) == 1)
            session.send('@uppercase hello coworker')
            session.wait(lambda: any(text(m.get('text')) == 'HELLO COWORKER' for m in messages('alpha')))
            source = next(m for m in messages('alpha') if text(m.get('text')) == '@uppercase hello coworker')
            reply = next(m for m in messages('alpha') if text(m.get('text')) == 'HELLO COWORKER')
            check('mention-runs-actual-worker-with-attributed-linked-reply', source['author'] != reply['author']
                  and reply.get('reply') == source['ref'] and source.get('invocation') == reply.get('invocation'))
            session.send('/home')
            session.until('workspace>')
            session.send('/new beta')
            session.until('you[beta]>')
            session.send('@uppercase beta only')
            session.wait(lambda: any(text(m.get('text')) == 'BETA ONLY' for m in messages('beta')))
            check('mention-autojoins-an-available-coworker', len(conversation('beta')['participants']) == 2)
            check('named-conversations-keep-separate-history', len(messages('beta')) == 2 and len(messages('alpha')) == 3)
            before = json.dumps(conversations(), sort_keys=True)
            session.send('/invite unavailable')
            session.until('invite: unavailable')
            session.until('you[beta]>')
            session.send('@unavailable do not run')
            session.until('you[beta]>')
            check('unavailable-target-does-not-gain-membership-or-message', json.dumps(conversations(), sort_keys=True) == before)
            session.send('/open alpha')
            session.until('Conversation: alpha')
            session.until('you[alpha]>')
            previous_messages = json.dumps(messages('alpha'), sort_keys=True)
            session.send('/remove uppercase')
            session.until('remove: removed')
            session.until('you[alpha]>')
            check('remove-preserves-existing-messages-and-results', json.dumps(messages('alpha'), sort_keys=True) == previous_messages
                  and len(conversation('alpha')['participants']) == 1)
            session.send('/invite uppercase')
            session.until('invite: invited')
            session.until('you[alpha]>')
            session.send('/history')
            history = session.until('you[alpha]>')
            check('reinvite-can-read-shared-history-without-replaying-it', b'HELLO COWORKER' in history
                  and json.dumps(messages('alpha'), sort_keys=True) == previous_messages)
            # Read controls must not recursively retain prior status/history results.
            start_size = (state / 'state.json').stat().st_size
            for _ in range(8):
                session.send('/status')
                session.until('status: found')
                session.until('you[alpha]>')
                session.send('/history')
                session.until('history: found')
                session.until('you[alpha]>')
            check('repeated-status-history-storage-grows-linearly',
                  (state / 'state.json').stat().st_size < start_size + 200000)
            retained = json.dumps(conversations(), sort_keys=True)
            session.finish()
            check('exit-is-a-real-audited-repl-action', any(
                i.get('target', [None] * 4)[3] == 'repl--do--exit'
                and i.get('phase') == 'completed' for i in invocations()))
            session = Session(command(), work)
            session.until('workspace>')
            check('restart-opens-home-and-preserves-conversations', json.dumps(conversations(), sort_keys=True) == retained)
            session.finish()
            session = None

            # The same exact target can be invoked by another authorized caller
            # without any terminal-specific dispatch path.
            environment = record(json.loads((state / 'context.json').read_text()))
            catalog = [record(a) for a in environment.get('workspace-actions') or []]
            new_action = next(a for a in catalog if a['target'][3] == 'repl--do--new')
            bob = environment['context'] + '/coworkers/test-bob'
            environment['caller'] = bob
            environment['available'] = list(dict.fromkeys([*(environment['available'] or []), bob]))
            environment['allow'] = [new_action['target']]
            environment['binding'] = [[k, v] for k, v in new_action.items() if k != 'name']
            trusted = work / 'trusted caller.json'

            def raw(event, env=environment, directory=state):
                trusted.write_text(json.dumps(list(env.items())))
                result = subprocess.run([binary, '--directory', directory, '--context', trusted],
                                        input=json.dumps(list(event.items())) + '\n', text=True,
                                        capture_output=True, cwd=work, timeout=20)
                report['transcript'].append({'raw': event, 'stdout': result.stdout,
                                             'stderr': result.stderr, 'exit': result.returncode})
                assert result.returncode == 0, (result.returncode, result.stderr)
                return json.loads(result.stdout)

            request = {'op': 'workspace-call', 'ref': environment['context'] + '/invocations/raw-new',
                       'origin': environment['context'] + '/requests/raw-new',
                       'target': new_action['target'],
                       'input': [['conversation', environment['context'] + '/conversations/by-bob']]}
            answer = raw(request)
            check('another-authorized-coworker-calls-same-new-action', conversation('by-bob') is not None
                  and bob in conversation('by-bob')['participants'])
            retained_count = len(invocations())
            raw(request)
            check('same-action-request-recovers-without-duplicate-work', len(invocations()) == retained_count)
            changed = dict(request, input=[['conversation', environment['context'] + '/conversations/conflict']])
            rejected = raw(changed)
            check('changed-input-cannot-reuse-action-identity', rejected[0] == 'conflict'
                  and conversation('conflict') is None)
            denied = dict(environment, allow=None)
            denied_request = dict(request, ref=environment['context'] + '/invocations/denied',
                                  origin=environment['context'] + '/requests/denied',
                                  input=[['conversation', environment['context'] + '/conversations/denied']])
            rejected = raw(denied_request, denied)
            check('denied-caller-cannot-bypass-action-authority-with-raw-dispatch', rejected[0] == 'denied'
                  and conversation('denied') is None and len(invocations()) == retained_count)
            malformed = dict(request, ref=environment['context'] + '/invocations/wrong-target',
                             origin=environment['context'] + '/requests/wrong-target',
                             target=[*new_action['target'][:-1], environment['context'] + '/wrong-contract'])
            rejected = raw(malformed)
            check('changed-exact-target-does-not-fall-back', rejected[0] == 'denied'
                  and len(invocations()) == retained_count)

            hidden = dict(environment, available=[v for v in environment['available'] if v != bob])
            before = json.dumps(conversation('by-bob'), sort_keys=True)
            rejected = raw({'op': 'workspace-publish', 'conversation': conversation('by-bob')['ref'],
                            'message': environment['context'] + '/messages/unavailable-caller',
                            'text': [{'char': c} for c in 'must not publish']}, hidden)
            check('publication-checks-current-caller-availability', rejected[0] in ('denied', 'unavailable')
                  and json.dumps(conversation('by-bob'), sort_keys=True) == before)

            language_action = next(a for a in catalog if a['target'][3] == 'language--do--reload')
            language_env = dict(environment, allow=[language_action['target']],
                                binding=[[k, v] for k, v in language_action.items() if k != 'name'])
            reload_request = {'op': 'workspace-call', 'ref': environment['context'] + '/invocations/raw-reload',
                              'origin': environment['context'] + '/requests/raw-reload',
                              'target': language_action['target'], 'input': None}
            answer = raw(reload_request, language_env)
            check('generic-language-action-without-config-settles-failed-not-running',
                  answer[0] == 'failed' and not record(answer[1]).get('attempt'))

            # A retained legacy route cannot be silently retargeted, but also
            # must not starve later work that is executable in this workspace.
            legacy_state = work / 'retained old binding'
            legacy_start = subprocess.run([binary, 'chat', '--directory', legacy_state],
                                          input='/exit\n', capture_output=True, text=True, timeout=20)
            assert legacy_start.returncode == 0, legacy_start.stderr
            legacy_env = record(json.loads((legacy_state / 'context.json').read_text()))
            base_binding = record(legacy_env['binding'])
            base_target = base_binding['target']
            old_target = [base_target[0], legacy_env['context'] + '/coworkers/uppercase', base_target[2],
                          'uppercase--text', legacy_env['context'] + '/bindings/uppercase',
                          legacy_env['context'] + '/contracts/uppercase']
            base_binding['target'] = old_target
            base_binding['route'] += '/uppercase'
            legacy_env['binding'] = list(base_binding.items())
            legacy_env['available'] = list(dict.fromkeys([*legacy_env['available'], old_target[1]]))
            legacy_env['allow'] = [old_target]
            legacy_request = {'op': 'admit', 'ref': legacy_env['context'] + '/invocations/legacy-queued',
                              'origin': legacy_env['context'] + '/requests/legacy-queued',
                              'conversation': legacy_env['context'] + '/conversations/chat',
                              'message': legacy_env['context'] + '/messages/legacy-queued',
                              'response-message': legacy_env['context'] + '/replies/legacy-queued',
                              'target': old_target, 'input': [{'char': c} for c in 'old request'],
                              'text': [{'char': c} for c in '@uppercase old request']}
            admitted = raw(legacy_request, legacy_env, legacy_state)
            assert admitted[0] == 'admitted', admitted
            session = Session(command(legacy_state), work)
            session.until('workspace>')
            session.send('/new runnable')
            session.until('you[runnable]>')
            session.send('@uppercase still-runs')
            session.wait(lambda: any(text(m.get('text')) == 'STILL-RUNS'
                                     for m in messages('runnable', legacy_state)))
            check('nonexecutable-old-binding-does-not-starve-new-work', True)
            old = next(i for i in invocations(legacy_state) if i['ref'] == legacy_request['ref'])
            check('old-admitted-action-is-not-retargeted-or-replayed',
                  old['phase'] == 'admitted' and old['target'] == old_target
                  and len(messages('chat', legacy_state)) == 1)
            session.finish()
            session = None

            # Real source builds, imported helper changes and last-good reload.
            source_state = work / 'source workspace'
            source = work / "worker $(touch SENTINEL); 'quoted'.rbel.md"
            helper = work / 'helper.rbel'
            def author(letter):
                helper.write_text('(def helper (xs) (join ' + chr(92) + letter + ' xs))\n')
                source.write_text(document('worker', '(import "helper.rbel")\n(def worker (xs) (helper xs))',
                                           '(worker "abc")', '"' + letter + 'abc"'))
            author('A')
            session = Session(command(source_state, '--source', source, '--rbel', compiler), work)
            session.until('workspace>', timeout=150)
            session.send('/new editing')
            session.until('you[editing]>')
            session.send('@uppercase abc')
            session.wait(lambda: any(text(m.get('text')) == 'Aabc' for m in messages('editing', source_state)))
            check('workspace-runs-compiled-source-with-relative-imports', not (work / 'SENTINEL').exists())
            author('B')
            session.send('/reload')
            session.until('you[editing]>', timeout=150)
            session.send('@uppercase abc')
            session.wait(lambda: any(text(m.get('text')) == 'Babc' for m in messages('editing', source_state)))
            check('reload-runs-language-coworker-action', any(
                i.get('target', [None] * 4)[3] == 'language--do--reload'
                and i.get('phase') == 'completed' for i in invocations(source_state)))
            source.write_text('(def broken (xs) (')
            session.send('/reload')
            session.until('you[editing]>', timeout=150)
            before_b = sum(text(m.get('text')) == 'Babc' for m in messages('editing', source_state))
            session.send('@uppercase abc')
            session.wait(lambda: sum(text(m.get('text')) == 'Babc' for m in messages('editing', source_state)) > before_b)
            check('failed-language-build-keeps-last-working-worker', any(
                i.get('target', [None] * 4)[3] == 'language--do--reload'
                and i.get('phase') == 'failed' for i in invocations(source_state)))
            session.finish()
            session = None

            # A deliberate test compiler emits a slow worker. Its external effect
            # marker lets us distinguish cancellation from merely hiding a reply.
            slow_state = work / 'slow workspace'
            slow_source = work / 'slow.rbel'
            slow_source.write_text('(def slow (xs) xs)')
            slow_compiler = work / 'slow compiler'
            marker = work / 'worker-started'
            effect = work / 'worker-finished'
            script = ('#!/bin/sh\n/bin/echo started >> ' + repr(str(marker)) + '\n'
                      '/bin/sleep 3\n/bin/echo finished >> ' + repr(str(effect)) + '\n/bin/cat\n')
            slow_compiler.write_text('#!/bin/sh\nwhile [ "$#" -gt 0 ]; do\n'
                                     'if [ "$1" = "-o" ]; then shift; output=$1; fi\nshift\ndone\n'
                                     '/bin/cat > "$output" <<\'WORKER\'\n' + script +
                                     'WORKER\n/bin/chmod +x "$output"\n')
            slow_compiler.chmod(0o755)
            session = Session(command(slow_state, '--source', slow_source, '--rbel', slow_compiler), work)
            session.until('workspace>', timeout=30)
            session.send('/new fifo')
            session.until('you[fifo]>')
            session.send('@uppercase first-in')
            session.wait(lambda: marker.exists())
            session.send('@uppercase second-in')
            session.send('@uppercase third-in')
            session.wait(lambda: len(messages('fifo', slow_state)) == 6, timeout=20)
            fifo_caller = record(json.loads((slow_state / 'context.json').read_text()))['caller']
            replies = [text(m.get('text')) for m in reversed(messages('fifo', slow_state))
                       if m.get('author') != fifo_caller]
            check('queued-requests-execute-in-admission-order', replies == ['first-in', 'second-in', 'third-in'])
            marker.unlink()
            effect.unlink()
            session.send('/new cancellation')
            session.until('you[cancellation]>')
            session.send('@uppercase first')
            session.wait(lambda: marker.exists())
            session.send('@uppercase queued')
            session.send('/remove uppercase')
            session.wait(lambda: len((conversation('cancellation', slow_state) or {}).get('participants') or []) == 1)
            session.wait(lambda: all(i.get('phase') == 'cancelled' for i in invocations(slow_state)
                                    if i.get('target', [None] * 4)[3] == 'uppercase--text'
                                    and i.get('conversation') == conversation('cancellation', slow_state)['ref']))
            workers = [i for i in invocations(slow_state) if i.get('target', [None] * 4)[3] == 'uppercase--text'
                       and i.get('conversation') == conversation('cancellation', slow_state)['ref']]
            check('prompt-remains-responsive-while-worker-runs', session.process.poll() is None)
            check('removal-cancels-queued-and-running-work', len(workers) == 2
                  and all(i.get('phase') == 'cancelled' for i in workers))
            check('cancelled-worker-does-not-publish-a-reply', len(messages('cancellation', slow_state)) == 2)
            session.send('/invite uppercase')
            session.until('invite: invited')
            session.until('you[cancellation]>')
            # Wait past the old execution deadline. Cancelled work must not revive.
            deadline = time.monotonic() + 3.2
            while time.monotonic() < deadline:
                session.drain(.05)
            check('reinvite-does-not-resume-cancelled-worker-or-repeat-effects',
                  not effect.exists() and len(marker.read_text().splitlines()) == 1
                  and len(messages('cancellation', slow_state)) == 2)
            session.finish()
            session = None

            outside_environment = record(json.loads((slow_state / 'context.json').read_text()))
            outside_catalog = [record(a) for a in outside_environment['workspace-actions']]
            uppercase_action = next(a for a in outside_catalog if a['target'][3] == 'uppercase--text')
            outsider = outside_environment['context'] + '/coworkers/outsider'
            outside_environment['caller'] = outsider
            outside_environment['available'] = [*outside_environment['available'], outsider]
            outside_environment['allow'] = [uppercase_action['target']]
            outside_environment['binding'] = [[k, v] for k, v in uppercase_action.items() if k != 'name']
            cancelled_ref = workers[0]['ref']
            for operation in ['run', 'complete']:
                rejected = raw({'op': operation, 'ref': cancelled_ref, 'claim': 'forged-claim'},
                               outside_environment, slow_state)
                check('outsider-' + operation + '-cannot-read-cancelled-invocation',
                      rejected[0] in ('denied', 'unavailable') and rejected[1] is None)

            # Worker runtime failure settles the attempt visibly instead of
            # retaining a phantom live process or automatically replaying it.
            copying_compiler = work / 'copying test compiler'
            copying_compiler.write_text("#!/bin/sh\nsource_file=$2\nwhile [ \"$#\" -gt 0 ]; do\n"
                                        "if [ \"$1\" = \"-o\" ]; then shift; output=$1; fi\nshift\ndone\n"
                                        "/bin/cp \"$source_file\" \"$output\"\n/bin/chmod +x \"$output\"\n")
            copying_compiler.chmod(0o755)
            for label, body in [('nonzero', '#!/bin/sh\nexit 7\n'),
                                ('invalid-utf8', '#!/bin/sh\nprintf \"\\377\"\n')]:
                failed_state = work / ('failed-' + label)
                failed_source = work / (label + '.worker')
                failed_source.write_text(body)
                session = Session(command(failed_state, '--source', failed_source, '--rbel', copying_compiler), work)
                session.until('workspace>')
                session.send('/new failure')
                session.until('you[failure]>')
                session.send('@uppercase no replay')
                def failed_attempt():
                    workers = [i for i in invocations(failed_state)
                               if i.get('target', [None] * 4)[3] == 'uppercase--text']
                    return workers and not workers[0].get('attempt') and workers[0].get('uncertain')
                session.wait(failed_attempt)
                check('worker-' + label + '-ends-live-attempt-with-visible-uncertainty',
                      len(messages('failure', failed_state)) == 1)
                session.finish()
                session = None
                retained_failed = json.dumps(invocations(failed_state), sort_keys=True)
                session = Session(command(failed_state, '--source', failed_source, '--rbel', copying_compiler), work)
                session.until('workspace>')
                session.send('/open failure')
                session.until('you[failure]>')
                session.send('/status')
                session.until('status: found')
                session.until('you[failure]>')
                check('worker-' + label + '-is-not-replayed-after-restart',
                      len(messages('failure', failed_state)) == 1 and failed_attempt())
                session.finish()
                session = None

            # The actual no-argument terminal entry uses workspace home too.
            noargs = work / 'no arguments'
            noargs.mkdir()
            master, slave = pty.openpty()
            process = subprocess.Popen([binary], stdin=slave, stdout=slave, stderr=slave, cwd=noargs)
            os.close(slave)
            terminal = b''
            try:
                deadline = time.monotonic() + 10
                while b'workspace>' not in terminal and time.monotonic() < deadline:
                    ready, _, _ = select.select([master], [], [], .1)
                    if ready:
                        terminal += os.read(master, 65536)
                check('no-argument-terminal-starts-at-workspace-home', b'workspace>' in terminal
                      and not conversations(noargs / '.libcwr'))
                os.write(master, b'/exit\n')
                process.wait(timeout=10)
                check('workspace-terminal-exit-is-clean', process.returncode == 0)
            finally:
                if process.poll() is None:
                    process.kill()
                    process.wait()
                os.close(master)
            report['passed'] = True
    except BaseException as error:
        report['error'] = str(error)
        raise
    finally:
        if session is not None:
            session.kill()
        args.report.write_text(json.dumps(report, indent=2) + '\n')
    print(f"{len(report['groups'])} workspace acceptance checks passed")


if __name__ == '__main__':
    main()
