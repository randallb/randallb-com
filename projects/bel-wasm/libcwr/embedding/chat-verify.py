#!/usr/bin/env python3
"""Exercise the native chat with isolated state and retained protocol records."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import pty
import select
import shutil
import subprocess
import tempfile
import time

ROOT = Path(__file__).resolve().parents[2]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--executable', type=Path, default=ROOT / 'target/libcwr-embedding/libcwr')
    parser.add_argument('--report', type=Path, default=ROOT / 'target/libcwr-chat-checks/verification.json')
    args = parser.parse_args()
    executable = args.executable.resolve()
    output = args.report.resolve()
    output.parent.mkdir(parents=True, exist_ok=True)
    output.unlink(missing_ok=True)
    report = {'passed': False, 'executable_sha256': hashlib.sha256(executable.read_bytes()).hexdigest(),
              'groups': [], 'transcript': []}

    def check(name, truth):
        if not truth:
            raise AssertionError(name)
        report['groups'].append({'name': name, 'passed': True})
        print('PASS', name, flush=True)

    def record(value):
        return dict(value or [])

    def text(value):
        return ''.join(c['char'] for c in value or [])

    try:
        with tempfile.TemporaryDirectory(prefix='libcwr-chat-') as directory:
            root = Path(directory)
            binary = root / 'libcwr'
            shutil.copy2(executable, binary)
            env = dict(os.environ, PATH='/nonexistent', NODE_OPTIONS='--require=/nonexistent/injected.cjs', NODE_PATH='/nonexistent')
            state = root / 'chat state'

            def run(*words, workspace=state, body=None, machine=False, expected=0):
                command = [str(binary), *map(str, words), '--directory', str(workspace)]
                if machine:
                    command.append('--json')
                result = subprocess.run(command, input=body, text=True, capture_output=True,
                                        cwd=root, env=env, timeout=40)
                report['transcript'].append({'arguments': command[1:], 'stdin': body,
                                             'exit': result.returncode, 'stdout': result.stdout, 'stderr': result.stderr})
                if expected is not None and result.returncode != expected:
                    raise AssertionError(f'{words}: exit {result.returncode}, expected {expected}: {result.stderr or result.stdout}')
                return result

            def machine(*words, **options):
                return json.loads(run(*words, machine=True, **options).stdout)

            def history(name='chat', **options):
                reply = machine('history', name, **options)
                if reply['code'] != 'found':
                    raise AssertionError(f'history unavailable: {name}')
                return [record(v) for v in reply['value']]

            def executions(workspace=state):
                path = workspace / 'executions.jsonl'
                return len(path.read_text().splitlines()) if path.exists() else 0

            first = run('chat', body="hello\nsecond thought\n/history\n/coworker\n/status\n/exit\n")
            check('chat-starts-in-new-directory-with-local-coworker',
                  (state / 'context.json').is_file() and 'uppercase' in first.stdout.lower())
            check('ordinary-text-runs-without-submit-or-run',
                  'HELLO' in first.stdout and 'SECOND THOUGHT' in first.stdout and executions() == 2)
            messages = history()
            check('chat-retains-two-messages-for-each-turn',
                  [text(m.get('text')) for m in messages] == ['hello', 'HELLO', 'second thought', 'SECOND THOUGHT'])
            invocations = [record(v) for v in machine('list', 'invocations')['value']]
            context_bytes = (state / 'context.json').read_bytes()
            caller = record(json.loads(context_bytes))['caller']
            for index in range(2):
                source, reply = messages[index * 2:index * 2 + 2]
                invocation = next(v for v in invocations if text(v.get('input')) == text(source['text']))
                check(f'chat-source-and-coworker-authorship-{index}',
                      source.get('author') == caller and reply.get('author') == invocation['target'][1])
                check(f'chat-reply-links-original-source-{index}',
                      reply.get('reply') == source.get('ref') and reply.get('ref') != source.get('ref') and
                      reply.get('invocation') == invocation.get('ref') and source.get('invocation') == invocation.get('ref'))
                check(f'chat-completed-result-is-actual-uppercase-{index}',
                      invocation.get('phase') == 'completed' and text(invocation.get('result')) == text(reply['text']))

            before = executions()
            restart = run('chat', body='/history\n/recover\n/exit\n')
            check('chat-restart-reopens-retained-conversation',
                  'HELLO' in restart.stdout and 'SECOND THOUGHT' in restart.stdout)
            check('chat-recovery-does-not-repeat-action-or-reply',
                  executions() == before and history() == messages)
            for invocation in invocations:
                name = invocation['ref'].rsplit('/', 1)[-1]
                run('chat', body=f'/reconcile {name}\n/exit\n')
            check('completed-reconciliation-does-not-duplicate-reply', executions() == before and history() == messages)

            literal = '"Quotes stay literal" naïve 世界 🙂 $(touch SENTINEL); `touch SENTINEL`'
            upper = literal.translate(str.maketrans('abcdefghijklmnopqrstuvwxyz', 'ABCDEFGHIJKLMNOPQRSTUVWXYZ'))
            varied = run('chat', body=literal + '\n//a literal slash\n\n/unknown\n/coworker missing\n/coworker uppercase\n/history\n/exit\n')
            varied_history = history()
            check('chat-text-is-literal-and-ascii-uppercase-only',
                  text(varied_history[-4]['text']) == literal and text(varied_history[-3]['text']) == upper)
            check('double-slash-sends-a-literal-leading-slash',
                  text(varied_history[-2]['text']) == '/a literal slash' and text(varied_history[-1]['text']) == '/A LITERAL SLASH')
            check('chat-never-evaluates-shell-text', not (root / 'SENTINEL').exists())
            check('invalid-control-and-coworker-leave-chat-usable',
                  len(varied_history) == 8 and bool(varied.stderr) and 'HELLO' in varied.stdout)
            check('blank-input-does-not-create-an-action', executions() == before + 2)

            run('chat', body='/new other\nnew thread\n/use chat\n/history\n/new\ngenerated thread\n/exit\n')
            check('new-and-use-keep-conversations-separated',
                  [text(v['text']) for v in history('other')] == ['new thread', 'NEW THREAD'] and history() == varied_history)
            conversations = [record(v)['ref'] for v in machine('list', 'conversations')['value']]
            check('new-without-name-creates-another-conversation', len(conversations) == 3)
            generated = next(v.rsplit('/', 1)[-1] for v in conversations if not v.endswith(('/chat', '/other')))
            check('generated-conversation-retains-its-exchange',
                  [text(v['text']) for v in history(generated)] == ['generated thread', 'GENERATED THREAD'])

            current_count = executions()
            ended = run('chat', body='/exit\nshould never execute\n')
            check('exit-stops-reading-further-input', ended.returncode == 0 and executions() == current_count)
            eof = run('chat', body='/history\n')
            check('eof-exits-cleanly-and-releases-lock', eof.returncode == 0 and not (state / '.libcwr-lock').exists())
            oversized = run('chat', body='bad\x00input\n' + 'x' * (256 * 1024 + 4097) + '\n/history\n/exit\n')
            check('malformed-and-oversized-input-do-not-execute',
                  executions() == current_count and bool(oversized.stderr) and 'HELLO' in oversized.stdout)

            # Only the explicit fixture transport injects interrupted execution.
            # The user's ordinary chat controls must then settle the saved receipt.
            configured = record(json.loads(context_bytes))
            target = invocations[0]['target']
            binding = record(configured['binding'])
            binding['target'] = target
            binding['route'] = invocations[0]['route']
            configured['binding'] = list(map(list, binding.items()))
            configured['allow'] = [target]
            configured['available'] = list(dict.fromkeys([*configured['available'], target[1]]))
            context = configured['context']
            reference = context + '/invocations/interrupted'
            body = 'recover this reply once'
            characters = [{'char': c} for c in body]
            event = [['op', 'admit'], ['ref', reference], ['origin', context + '/requests/interrupted'],
                     ['conversation', context + '/conversations/chat'],
                     ['message', context + '/messages/interrupted'],
                     ['response-message', context + '/replies/interrupted'],
                     ['target', target], ['input', characters], ['text', characters]]
            controls = [
                {'operation': 'configure', 'arguments': list(map(list, configured.items()))},
                {'operation': 'dispatch', 'arguments': event},
                {'operation': 'fault', 'arguments': 'lose-completion'},
                {'operation': 'dispatch', 'arguments': [['op', 'run'], ['ref', reference]]},
            ]
            injected = subprocess.run([str(binary), '--test-host', '--directory', str(state)],
                                      input=''.join(json.dumps(v) + '\n' for v in controls), text=True,
                                      capture_output=True, env=env, timeout=30, check=True)
            report['transcript'].append({'fixture': 'lose-completion', 'stdout': injected.stdout, 'stderr': injected.stderr})
            check('fixture-runs-uppercase-before-losing-completion',
                  json.loads(injected.stdout.splitlines()[-1])['value'][0] == 'running' and executions() == current_count + 1)
            before_reconcile = history()
            check('interrupted-chat-has-source-without-published-reply',
                  text(before_reconcile[-1]['text']) == body)
            reconciled = run('chat', body='/status\n/reconcile interrupted\n/history\n/reconcile interrupted\n/recover\n/exit\n')
            after_reconcile = history()
            check('chat-reconciliation-publishes-the-bound-uppercase-reply',
                  text(after_reconcile[-1]['text']) == 'RECOVER THIS REPLY ONCE' and
                  after_reconcile[-1]['reply'] == before_reconcile[-1]['ref'] and
                  after_reconcile[-1]['author'] == target[1])
            check('repeated-reconciliation-keeps-one-reply-and-one-execution',
                  len(after_reconcile) == len(before_reconcile) + 1 and executions() == current_count + 1)

            # A missing receipt permits retry only because this exact local
            # action is declared repeat-safe. Its effect really can run again.
            retry_reference = context + '/invocations/missing-receipt'
            retry_body = 'repeat-safe missing receipt'
            retry_characters = [{'char': c} for c in retry_body]
            retry_event = [['op', 'admit'], ['ref', retry_reference],
                           ['origin', context + '/requests/missing-receipt'],
                           ['conversation', context + '/conversations/chat'],
                           ['message', context + '/messages/missing-receipt'],
                           ['response-message', context + '/replies/missing-receipt'],
                           ['target', target], ['input', retry_characters], ['text', retry_characters]]
            before_retry_count = executions()
            retry_controls = [
                {'operation': 'configure', 'arguments': list(map(list, configured.items()))},
                {'operation': 'dispatch', 'arguments': retry_event},
                {'operation': 'fault', 'arguments': 'lose-receipt'},
                {'operation': 'dispatch', 'arguments': [['op', 'run'], ['ref', retry_reference]]},
            ]
            retry_injected = subprocess.run([str(binary), '--test-host', '--directory', str(state)],
                                            input=''.join(json.dumps(v) + '\n' for v in retry_controls),
                                            text=True, capture_output=True, env=env, timeout=30, check=True)
            report['transcript'].append({'fixture': 'lose-receipt', 'stdout': retry_injected.stdout,
                                         'stderr': retry_injected.stderr})
            check('missing-receipt-fixture-executed-without-publishing-reply',
                  executions() == before_retry_count + 1 and text(history()[-1]['text']) == retry_body)
            before_retry_history = history()
            settled = run('chat', body='/status\n/reconcile missing-receipt\n/exit\n')
            settled_invocation = record(machine('show', 'missing-receipt')['value'])
            retried = run('chat', body='/run missing-receipt\n/run missing-receipt\n/history\n/exit\n')
            retry_history = history()
            check('missing-receipt-reconciliation-allows-declared-safe-retry',
                  'reconciliation-required' in settled.stdout and settled_invocation.get('checked') == 't' and
                  executions() == before_retry_count + 2)
            check('safe-retry-publishes-one-retained-reply-despite-repeat-effect',
                  len(retry_history) == len(before_retry_history) + 1 and
                  text(retry_history[-1]['text']) == 'REPEAT-SAFE MISSING RECEIPT' and
                  retry_history[-1]['reply'] == before_retry_history[-1]['ref'] and
                  retry_history[-1]['author'] == target[1])

            long_state = root / 'long-chat'
            long_body = 'abc xyz ' * 80 + 'naïve 世界 🙂'
            long_upper = long_body.translate(str.maketrans('abcdefghijklmnopqrstuvwxyz', 'ABCDEFGHIJKLMNOPQRSTUVWXYZ'))
            run('chat', workspace=long_state, body=long_body + '\n/exit\n')
            long_history = history(workspace=long_state)
            check('long-chat-message-crosses-action-chunks-and-preserves-nonascii',
                  [text(v['text']) for v in long_history] == [long_body, long_upper] and executions(long_state) == 1)

            unrelated = root / 'unrelated'
            unrelated.mkdir()
            (unrelated / 'notes.txt').write_text('keep unrelated work\n')
            refused = run('chat', workspace=unrelated, body='hello\n/exit\n', expected=None)
            check('chat-refuses-to-initialize-unrelated-nonempty-directory',
                  refused.returncode != 0 and (unrelated / 'notes.txt').read_text() == 'keep unrelated work\n' and not (unrelated / 'context.json').exists())

            old = root / 'existing-cli'
            machine('init', workspace=old)
            machine('conversation', 'create', 'journal', workspace=old)
            machine('submit', 'journal', 'first', '--text', 'old CLI artifact', workspace=old)
            machine('run', 'first', workspace=old)
            old_context = (old / 'context.json').read_bytes()
            old_history = history('journal', workspace=old)
            run('chat', workspace=old, body='hello from upgraded workspace\n/exit\n')
            check('chat-preserves-existing-workspace-context-and-history',
                  (old / 'context.json').read_bytes() == old_context and history('journal', workspace=old) == old_history)
            check('chat-preserves-existing-store-text-artifact',
                  run('artifact', 'first', workspace=old).stdout == 'old CLI artifact' and executions(old) == 2)

            # No-argument terminal startup is the primary experience. Use a
            # private working directory so its default .libcwr is isolated.
            terminal_dir = root / 'terminal'
            terminal_dir.mkdir()
            master, slave = pty.openpty()
            terminal = subprocess.Popen([str(binary)], stdin=slave, stdout=slave, stderr=slave,
                                        cwd=terminal_dir, env=env)
            os.close(slave)
            captured = bytearray()

            def until(marker):
                deadline = time.monotonic() + 10
                while marker not in captured and time.monotonic() < deadline:
                    ready, _, _ = select.select([master], [], [], 0.1)
                    if ready:
                        try:
                            chunk = os.read(master, 8192)
                        except OSError:
                            break
                        if not chunk:
                            break
                        captured.extend(chunk)
                return marker in captured

            try:
                check('terminal-no-arguments-opens-chat-prompt', until(b'you[chat]>'))
                os.write(master, b'hello terminal\n')
                check('terminal-plain-text-shows-coworker-reply', until(b'HELLO TERMINAL'))
                os.write(master, b'/exit\n')
                terminal.wait(timeout=10)
                check('terminal-chat-exits-cleanly', terminal.returncode == 0)
            finally:
                if terminal.poll() is None:
                    terminal.kill()
                    terminal.wait(timeout=5)
                os.close(master)
            report['terminal_transcript'] = captured.decode(errors='replace')
            report['passed'] = True
    except Exception as error:
        report['error'] = str(error)
        raise
    finally:
        output.write_text(json.dumps(report, indent=2) + '\n')
    print(f"Native chat acceptance: {len(report['groups'])} groups passed")


if __name__ == '__main__':
    main()
