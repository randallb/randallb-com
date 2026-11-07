#!/usr/bin/env python3
"""Exercise source reloads through one long-lived native chat session."""
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


def document(name, body, expression, expected):
    return (f'```rbel definition={name}\n{body}\n```\n'
            f'```rbel example={name}-example\n{expression}\n=>\n{expected}\n```\n')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--executable', type=Path, default=ROOT / 'target/libcwr-embedding/libcwr')
    parser.add_argument('--rbel', type=Path, default=ROOT / 'target/rbel-command/bin/rbel')
    parser.add_argument('--report', type=Path, default=ROOT / 'target/reload-verification/results.json')
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

        def send(self, text):
            self.process.stdin.write(text.encode() + b'\n')
            self.process.stdin.flush()

        def until(self, expected, timeout=150):
            target = expected.encode()
            deadline = time.monotonic() + timeout
            while target not in self.stdout[self.offset:]:
                if time.monotonic() >= deadline:
                    raise AssertionError(('timeout', expected, self.stdout[-3000:], self.stderr[-3000:]))
                ready, _, _ = select.select([self.process.stdout, self.process.stderr], [], [], .1)
                for stream in ready:
                    data = os.read(stream.fileno(), 65536)
                    if stream is self.process.stdout:
                        self.stdout += data
                    else:
                        self.stderr += data
                if self.process.poll() is not None:
                    raise AssertionError(('chat exited', self.process.returncode, expected, self.stdout, self.stderr))
            end = self.stdout.index(target, self.offset) + len(target)
            result = self.stdout[self.offset:end]
            self.offset = end
            return result

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
                raise AssertionError(('chat exit', self.process.returncode, self.stdout, self.stderr))

        def kill(self):
            if self.process.poll() is None:
                self.process.kill()
                self.process.communicate()

    session = None
    try:
        with tempfile.TemporaryDirectory(prefix='rbel reload ') as temporary:
            work = Path(temporary)
            binary = work / 'libcwr'
            shutil.copy2(executable, binary)
            state = work / 'saved chat'
            source = work / "coworker $(touch SENTINEL); 'quoted'.rbel.md"
            dependency = work / 'shared helpers' / 'transform.rbel.md'
            dependency.parent.mkdir()

            def author(letter='A', bad_example=False):
                helper = ('(def change (xs) (if xs (join (if (id (car xs) \\a) \\' + letter +
                          ' (car xs)) (change (cdr xs))) nil))')
                dependency.write_text(document('change', helper, '(change "abc")',
                                               '"WRONG"' if bad_example else f'"{letter}bc"'))
                source.write_text(document('worker', '(import "shared helpers/transform.rbel.md")\n'
                                                      '(def worker (xs) (change xs))',
                                           '(worker "abc")', f'"{letter}bc"'))

            def command(*extra):
                return [binary, 'chat', '--directory', state, '--source', source, '--rbel', compiler, *extra]

            author()
            session = Session(command(), work)
            session.until('Conversation: chat')
            pid = session.process.pid
            session.send('abc')
            session.until('uppercase: Abc')
            check('source-with-relative-import-runs-in-native-chat', True)
            check('source-metacharacters-do-not-execute-shell', not (work / 'SENTINEL').exists())

            author('Z')
            session.send('abc')
            session.until('uppercase: Abc')
            check('editing-source-does-not-change-active-build-until-reload', True)
            session.send('/reload')
            session.until('Reloaded coworker source.')
            session.send('abc')
            session.until('uppercase: Zbc')
            check('reload-picks-up-edited-imported-definition', True)
            check('successful-reload-keeps-same-chat-process', session.process.pid == pid and session.process.poll() is None)

            long_text = 'a' * 300 + ' naïve 世界 🙂'
            session.send(long_text)
            session.until('uppercase: ' + long_text.replace('a', 'Z'))
            check('source-worker-preserves-utf8-and-crosses-256-byte-chunks', True)

            good_source = source.read_text()
            source.write_text(document('worker', '(def worker (xs) (', 'nil', 'nil'))
            session.send('/reload')
            session.send('abc')
            session.until('uppercase: Zbc')
            check('syntax-error-reload-keeps-last-working-worker', True)
            source.write_text(good_source.replace('shared helpers/transform.rbel.md', 'missing.rbel'))
            session.send('/reload')
            session.send('abc')
            session.until('uppercase: Zbc')
            check('missing-dependency-reload-keeps-last-working-worker', True)
            author('Z', bad_example=True)
            session.send('/reload')
            session.send('abc')
            session.until('uppercase: Zbc')
            check('failing-imported-example-reload-keeps-last-working-worker', True)
            check('failed-reloads-report-diagnostics', bool(session.stderr))

            author('Z')
            session.send('/new notes')
            session.until('Conversation: notes')
            session.send('/reload')
            session.until('Reloaded coworker source.')
            session.send('abc')
            session.until('uppercase: Zbc')
            session.send('/history')
            session.until('you: abc')
            check('reload-preserves-selected-conversation', True)
            session.finish()
            session = None

            def history(name):
                result = subprocess.run([binary, 'history', name, '--directory', state, '--json'],
                                        cwd=work, capture_output=True, text=True, check=True, timeout=30)
                value = json.loads(result.stdout)
                assert value['code'] == 'found', value
                return [dict(message) for message in value['value']]

            def text(value):
                return ''.join(c['char'] for c in value or [])

            chat, notes = history('chat'), history('notes')
            replies = [text(m['text']) for m in chat][1::2]
            check('history-retains-old-and-new-worker-results', replies[:3] == ['Abc', 'Abc', 'Zbc'])
            check('history-retains-message-and-reply-links-across-reload', all(
                chat[i + 1].get('reply') == chat[i].get('ref') and
                chat[i + 1].get('author') != chat[i].get('author')
                for i in range(0, len(chat), 2)))
            check('selected-conversation-keeps-own-history', [text(m['text']) for m in notes] == ['abc', 'Zbc'])
            count_before = len((state / 'executions.jsonl').read_text().splitlines())
            session = Session(command(), work)
            session.until('Conversation: chat')
            session.send('/history')
            session.until('uppercase: Abc')
            session.send('abc')
            session.until('uppercase: Zbc')
            session.finish()
            session = None
            check('restart-retains-history-and-source-worker', len(history('chat')) == len(chat) + 2)
            check('reload-and-restart-do-not-repeat-completed-actions',
                  len((state / 'executions.jsonl').read_text().splitlines()) == count_before + 1)

            builtin = subprocess.run([binary, 'chat', '--directory', work / 'builtin'],
                                     input='hello\n/reload\nhello again\n/exit\n', text=True,
                                     cwd=work, capture_output=True, timeout=30)
            check('builtin-mode-remains-usable-without-source', builtin.returncode == 0 and
                  'uppercase: HELLO' in builtin.stdout and 'uppercase: HELLO AGAIN' in builtin.stdout)
            check('reload-without-source-reports-unavailable',
                  bool(builtin.stderr) or 'source' in builtin.stdout.lower())
        report['passed'] = True
    finally:
        if session is not None:
            session.kill()
        args.report.write_text(json.dumps(report, indent=2) + '\n')
    print(f'{len(report["groups"])} native source reload checks passed')


if __name__ == '__main__':
    main()
