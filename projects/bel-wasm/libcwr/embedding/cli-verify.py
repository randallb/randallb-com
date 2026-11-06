#!/usr/bin/env python3
"""Exercise the native user CLI through real processes and isolated local state."""
import argparse
import hashlib
import json
import os
import pty
import select
from pathlib import Path
import shutil
import subprocess
import tempfile
import time

ROOT = Path(__file__).resolve().parents[2]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--executable', type=Path, default=ROOT / 'target/libcwr-embedding/libcwr')
    parser.add_argument('--report', type=Path, default=ROOT / 'target/libcwr-cli-checks/verification.json')
    args = parser.parse_args()
    executable = args.executable.resolve()
    report_path = args.report.resolve()
    report_path.parent.mkdir(parents=True, exist_ok=True)
    report_path.unlink(missing_ok=True)
    report = {'passed': False, 'groups': [], 'executable_sha256': hashlib.sha256(executable.read_bytes()).hexdigest()}

    def check(name, truth):
        if not truth:
            raise AssertionError(name)
        report['groups'].append({'name': name, 'passed': True})
        print('PASS', name, flush=True)

    try:
        with tempfile.TemporaryDirectory(prefix='libcwr-cli-') as directory:
            root = Path(directory)
            binary = root / 'libcwr'
            shutil.copy2(executable, binary)
            env = dict(os.environ, PATH='/nonexistent', NODE_OPTIONS='--require=/nonexistent/injected.cjs', NODE_PATH='/nonexistent')
            workspace = root / 'workspace'
            transcript = []

            def run(*words, state=workspace, machine=False, expected=0):
                command = [str(binary), *map(str, words)]
                if state is not None:
                    command += ['--directory', str(state)]
                if machine:
                    command += ['--json']
                result = subprocess.run(command, cwd=root, env=env, capture_output=True, text=True, timeout=30)
                transcript.append({'arguments': command[1:], 'exit': result.returncode, 'stdout': result.stdout, 'stderr': result.stderr})
                if expected is not None and result.returncode != expected:
                    raise AssertionError(f'{words!r}: exit {result.returncode}, expected {expected}: {result.stderr or result.stdout}')
                return result

            def machine(*words, **options):
                result = run(*words, machine=True, **options)
                value = json.loads(result.stdout)
                if not isinstance(value, dict) or not isinstance(value.get('command'), str) or not isinstance(value.get('code'), str) or 'value' not in value:
                    raise AssertionError('CLI JSON reply requires command, code and value')
                return value

            def record(value):
                return dict(value or [])

            def execution_count(state=workspace):
                path = state / 'executions.jsonl'
                return len(path.read_text().splitlines()) if path.exists() else 0

            help_result = run('--help', state=None)
            check('help-explains-the-usable-local-workflow', all(word in help_result.stdout.lower() for word in ['init', 'submit', 'run', 'status']))
            check('no-arguments-gives-help', run(state=None).returncode == 0)
            version = machine('--version', state=None)
            check('version-identifies-build-and-core', version['code'] == 'ok' and bool(version['value'].get('revision')) and len(version['value'].get('core_sha256', '')) == 64)
            absent = root / 'not-initialized'
            missing = run('status', state=absent, expected=None)
            check('inspection-requires-initialized-workspace', missing.returncode != 0 and not (absent / 'context.json').exists())
            initialized = machine('init')
            check('relocated-native-initializes-without-runtime-sidecars', initialized['code'] == 'initialized' and (workspace / 'context.json').is_file())
            context_bytes = (workspace / 'context.json').read_bytes()
            original_files = {p.name: p.read_bytes() for p in workspace.iterdir() if p.is_file()}
            repeated_init = run('init', expected=None)
            check('repeat-init-preserves-existing-workspace', repeated_init.returncode != 0 and all((workspace / name).read_bytes() == data for name, data in original_files.items()))

            other = root / 'unrelated'
            other.mkdir()
            (other / 'notes.txt').write_text('preserve this unrelated folder\n')
            refused = run('init', state=other, expected=None)
            check('init-refuses-unrelated-nonempty-directory', refused.returncode != 0 and sorted(p.name for p in other.iterdir()) == ['notes.txt'] and (other / 'notes.txt').read_text() == 'preserve this unrelated folder\n')

            created = machine('conversation', 'create', 'journal')
            check('create-named-conversation', created['code'] == 'created' and record(created['value'])['ref'].endswith('/conversations/journal'))
            existing = machine('conversation', 'create', 'journal')
            check('conversation-retry-is-stable', existing['code'] == 'existing' and existing['value'] == created['value'])
            missing_conversation = machine('submit', 'not-created', 'no-conversation', '--text', 'must not run', expected=None)
            check('submission-does-not-create-a-conversation', missing_conversation['code'] == 'unavailable' and transcript[-1]['exit'] != 0 and execution_count() == 0)
            submitted = machine('submit', 'journal', 'first', '--text', 'hello from the native CLI')
            invocation = record(submitted['value'])
            check('submit-admits-without-executing', submitted['code'] == 'admitted' and invocation['phase'] == 'admitted' and invocation['ref'].endswith('/invocations/first') and execution_count() == 0)
            status = run('status')
            check('human-status-shows-admitted-work', 'first' in status.stdout and 'admitted' in status.stdout)
            listed = machine('list', 'invocations')
            check('machine-list-shows-admitted-work', listed['code'] == 'ok' and any(record(v).get('ref') == invocation['ref'] and record(v).get('phase') == 'admitted' for v in listed['value']))
            conversations = machine('list', 'conversations')
            check('list-shows-conversation', any(record(v).get('ref') == record(created['value'])['ref'] for v in conversations['value']))
            repeated = machine('submit', 'journal', 'first', '--text', 'hello from the native CLI')
            check('submission-retry-keeps-same-invocation', repeated['code'] == 'existing' and record(repeated['value'])['ref'] == invocation['ref'] and execution_count() == 0)
            conflict = machine('submit', 'journal', 'first', '--text', 'different action input', expected=None)
            check('conflicting-submission-is-an-error', conflict['code'] == 'conflict' and transcript[-1]['exit'] != 0 and execution_count() == 0)

            completed = run('run', 'first')
            check('human-run-shows-completion', 'first' in completed.stdout and 'completed' in completed.stdout and execution_count() == 1)
            artifact = run('artifact', 'first')
            check('artifact-prints-actual-text', artifact.stdout == 'hello from the native CLI')
            shown = machine('show', 'first')
            check('show-retains-completed-phase', shown['code'] == 'found' and record(shown['value'])['phase'] == 'completed')
            completed_again = machine('run', 'first')
            check('fresh-process-duplicate-does-not-repeat-action', completed_again['code'] == 'completed' and execution_count() == 1)
            history = run('history', 'journal')
            check('history-shows-submitted-message', 'hello from the native CLI' in history.stdout)
            recovered = machine('recover')
            check('recover-preserves-completed-work', recovered['code'] == 'recovered' and execution_count() == 1 and any(record(v).get('phase') == 'completed' for v in recovered['value']['invocations']))

            for index, text in enumerate(['', '--not-an-option', 'naïve 世界 🙂\nsecond line\n']):
                name = f'text-{index}'
                machine('submit', 'journal', name, '--text', text)
                machine('run', name)
                output = run('artifact', name)
                check(f'action-text-roundtrip-{index}', output.stdout == text)

            count_before = execution_count()
            for index, words in enumerate([
                ('conversation', 'create', '../escape'),
                ('conversation', 'create', 'x' * 65),
                ('submit', 'journal', 'missing-text'),
                ('run', 'missing'),
                ('run', 'first', '--test-host'),
                ('run', 'first', '--unknown'),
                ('run', 'first', '--directory', str(workspace)),
                ('list', 'unknown'),
                ('submit', 'journal', 'bad', '--text'),
            ]):
                rejected = run(*words, expected=None)
                check(f'cli-rejects-invalid-request-{index}', rejected.returncode != 0 and execution_count() == count_before)

            # Changing trusted local configuration must not expose retained records
            # through CLI inspection, even though it can enumerate local candidates.
            context = json.loads(context_bytes)
            for row in context:
                if row[0] == 'caller':
                    row[1] = 'cwp://org.example.libcwr/outsider'
            (workspace / 'context.json').write_text(json.dumps(context))
            for command in [('show', 'first'), ('history', 'journal'), ('artifact', 'first')]:
                denied = run(*command, expected=None)
                check('read-authority-' + command[0], denied.returncode != 0 and 'hello from the native CLI' not in denied.stdout)
            filtered = machine('list', 'invocations')
            check('list-filters-unavailable-records', filtered['value'] == [])
            check('conversation-list-filters-unavailable-records', machine('list', 'conversations')['value'] == [])
            hidden_status = machine('status')['value']
            check('status-filters-unavailable-records', hidden_status['conversations'] == [] and hidden_status['invocations'] == [])
            (workspace / 'context.json').write_bytes(context_bytes)

            # Use only the explicit fixture transport to create an uncertain
            # execution; the user-facing commands must then reconcile its receipt.
            uncertain = machine('submit', 'journal', 'interrupted', '--text', 'once, despite restart')
            reference = record(uncertain['value'])['ref']
            controls = [
                {'operation': 'configure', 'arguments': json.loads(context_bytes)},
                {'operation': 'fault', 'arguments': 'lose-completion'},
                {'operation': 'dispatch', 'arguments': [['op', 'run'], ['ref', reference]]},
            ]
            injected = subprocess.run([str(binary), '--test-host', '--directory', str(workspace)],
                                      input=''.join(json.dumps(v) + '\n' for v in controls), text=True,
                                      capture_output=True, env=env, timeout=30)
            if injected.returncode != 0:
                raise AssertionError(injected.stderr)
            check('fixture-created-interrupted-execution', json.loads(injected.stdout.splitlines()[-1])['value'][0] == 'running')
            count_uncertain = execution_count()
            uncertain_show = machine('show', 'interrupted')
            check('status-retains-uncertain-execution-after-restart', record(uncertain_show['value'])['phase'] == 'running' and record(uncertain_show['value']).get('uncertain') == 't')
            reconciled = machine('reconcile', 'interrupted')
            check('user-reconciles-existing-bound-receipt', reconciled['code'] == 'completed' and execution_count() == count_uncertain)
            machine('run', 'interrupted')
            check('reconciled-action-does-not-repeat', execution_count() == count_uncertain and run('artifact', 'interrupted').stdout == 'once, despite restart')

            # Existing owner and crashed owner locks remain an explicit operator
            # boundary; recover never silently removes a competing lock.
            owner = subprocess.Popen([str(binary), '--directory', str(workspace), '--context', str(workspace / 'context.json')],
                                     stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                     text=True, env=env)
            lock = workspace / '.libcwr-lock'
            try:
                for _ in range(200):
                    if lock.exists() or owner.poll() is not None:
                        break
                    time.sleep(0.01)
                locked = run('status', expected=None)
                check('cli-rejects-competing-owner', owner.poll() is None and locked.returncode != 0 and lock.exists())
                owner.kill()
                owner.communicate(timeout=5)
                stale = run('recover', expected=None)
                check('recover-preserves-crashed-owner-lock', stale.returncode != 0 and lock.exists())
                lock.unlink()  # Isolated fixture; owner death confirmed above.
                check('recover-after-confirmed-owner-cleanup', machine('recover')['code'] == 'recovered')
            finally:
                if owner.poll() is None:
                    owner.kill()
                    owner.communicate(timeout=5)

            default = root / 'default-workflow'
            default.mkdir()
            result = subprocess.run([str(binary), 'init'], cwd=default, env=env, capture_output=True, text=True, timeout=30)
            check('default-directory-is-local-dot-libcwr', result.returncode == 0 and (default / '.libcwr/context.json').is_file())
            # Exercise an ongoing session, including command recovery and retained
            # conversation selection. It starts before the workspace exists.
            repl_workspace = root / 'repl-workspace'
            repl_input = """
init
conversation create notes
submit first --text 'hello from one ongoing session'
status
run first
history
show first
artifact first
use missing
submit kept-selection --text "still in notes"
run kept-selection
conversation create other --json
submit json-selected --text 'selected by JSON create'
conversation create --help
submit help-preserved --text 'help keeps other selected'
use notes
conversation create other
submit existing-selected --text 'selected by existing create'
use notes
submit quoted --text "literal $(touch SENTINEL); `touch SENTINEL`"
run quoted
submit escaped --text escaped\\ space
run escaped
submit empty --text ''
run empty
submit broken --text 'unterminated
unknown-command
repl
status --directory forbidden
status
quit
conversation create after-quit
"""
            session = subprocess.run([str(binary), 'repl', '--directory', str(repl_workspace)],
                                     input=repl_input, cwd=root, env=env,
                                     capture_output=True, text=True, timeout=30)
            transcript.append({'arguments': ['repl', '--directory', str(repl_workspace)],
                               'stdin': repl_input, 'exit': session.returncode,
                               'stdout': session.stdout, 'stderr': session.stderr})
            check('repl-session-initializes-and-runs-work', session.returncode == 0 and
                  'completed' in session.stdout and 'hello from one ongoing session' in session.stdout)
            check('repl-selected-conversation-persists-after-failed-use',
                  run('artifact', 'kept-selection', state=repl_workspace).stdout == 'still in notes')
            for invocation, description in [
                ('json-selected', 'json-create-selects-conversation'),
                ('help-preserved', 'create-help-preserves-conversation'),
                ('existing-selected', 'existing-create-selects-conversation'),
            ]:
                selected = machine('show', invocation, state=repl_workspace)
                check('repl-' + description, selected['code'] == 'found' and
                      record(selected['value'])['conversation'].endswith('/conversations/other'))
            check('repl-quotes-do-not-execute-shell', not (root / 'SENTINEL').exists() and
                  run('artifact', 'quoted', state=repl_workspace).stdout == 'literal $(touch SENTINEL); `touch SENTINEL`')
            check('repl-backslash-escaping-and-empty-quoted-text',
                  run('artifact', 'escaped', state=repl_workspace).stdout == 'escaped space' and
                  run('artifact', 'empty', state=repl_workspace).stdout == '')
            check('repl-errors-leave-session-alive', bool(session.stderr) and
                  session.stdout.rfind('Invocations:') > session.stdout.find('completed') and
                  not (root / 'forbidden').exists())
            after_quit = machine('list', 'conversations', state=repl_workspace)['value']
            check('repl-quit-stops-reading-commands',
                  not any(record(v)['ref'].endswith('/after-quit') for v in after_quit))
            restart_input = "use notes\nhistory\nrun first\nexit\n"
            count_before = execution_count(repl_workspace)
            restarted = subprocess.run([str(binary), 'repl', '--directory', str(repl_workspace)],
                                       input=restart_input, cwd=root, env=env,
                                       capture_output=True, text=True, timeout=30)
            check('repl-restarts-with-retained-history-and-no-repeat', restarted.returncode == 0 and
                  'hello from one ongoing session' in restarted.stdout and
                  execution_count(repl_workspace) == count_before)
            eof = subprocess.run([str(binary), 'repl', '--directory', str(repl_workspace)],
                                 input='status\n', cwd=root, env=env,
                                 capture_output=True, text=True, timeout=30)
            check('repl-eof-exits-cleanly-and-releases-lock', eof.returncode == 0 and
                  not (repl_workspace / '.libcwr-lock').exists())
            malformed = subprocess.run([str(binary), 'repl', '--directory', str(repl_workspace)],
                                       input='bad\x00command\n' + 'x' * (256 * 1024 + 4097) + '\nstatus\nexit\n',
                                       cwd=root, env=env, capture_output=True, text=True, timeout=30)
            check('repl-rejects-nul-and-oversized-lines-without-exiting',
                  malformed.returncode == 0 and 'Invocations:' in malformed.stdout and bool(malformed.stderr))

            # The explicit legacy chat command shows
            # its current selection in the prompt. PTY output is retained below.
            master, slave = pty.openpty()
            terminal = subprocess.Popen([str(binary), 'chat'], stdin=slave, stdout=slave,
                                        stderr=slave, cwd=default, env=env)
            os.close(slave)
            terminal_output = bytearray()

            def until(marker, timeout=10):
                deadline = time.monotonic() + timeout
                while marker not in terminal_output and time.monotonic() < deadline:
                    ready, _, _ = select.select([master], [], [], 0.1)
                    if ready:
                        try:
                            chunk = os.read(master, 8192)
                        except OSError:
                            break
                        if not chunk:
                            break
                        terminal_output.extend(chunk)
                return marker in terminal_output

            try:
                check('terminal-explicit-chat-enters-legacy-chat', until(b'you['))
                os.write(master, b'/new terminal\n')
                check('repl-prompt-shows-selected-conversation', until(b'you[terminal]>'))
                os.write(master, b'/exit\n')
                terminal.wait(timeout=10)
                check('terminal-exit-is-clean', terminal.returncode == 0)
            finally:
                if terminal.poll() is None:
                    terminal.kill()
                    terminal.wait(timeout=5)
                os.close(master)
            report['terminal_transcript'] = terminal_output.decode(errors='replace')
            report['transcript'] = transcript
            report['passed'] = True
    except Exception as error:
        report['error'] = str(error)
        raise
    finally:
        report_path.write_text(json.dumps(report, indent=2) + '\n')
    print(f"Native CLI acceptance: {len(report['groups'])} groups passed")


if __name__ == '__main__':
    main()
