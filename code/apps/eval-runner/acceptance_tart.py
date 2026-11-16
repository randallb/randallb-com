"""Actual macOS VM acceptance, including interruption and forced-kill recovery."""
import argparse
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import time


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--output', type=Path, required=True, help='new evidence directory')
    parser.add_argument('--tart-home', type=Path, default=Path('.rbdc-evals/tart'))
    args = parser.parse_args()
    root = args.output.resolve()
    root.mkdir(parents=True, exist_ok=False)
    cli = Path(__file__).with_name('cli.py')
    reports = []

    def case(name, argv, expected=0, outcome='success', timeout=60, interruption=None):
        target = root / name
        target.mkdir()
        state = target / 'state'
        command = [sys.executable, str(cli), 'evals', 'run', '--vm-type', 'tart',
                   '--tart-home', str(args.tart_home.resolve()), '--state', str(state),
                   '--timeout', str(timeout), '--', *argv]
        env = dict(os.environ, RBDC_HOST_SECRET='must-not-reach-guest')
        with (target / 'stdout').open('wb') as stdout, (target / 'stderr').open('wb') as stderr:
            process = subprocess.Popen(command, stdout=stdout, stderr=stderr, env=env)
            if interruption:
                deadline = time.monotonic() + 240
                while time.monotonic() < deadline:
                    if b'READY' in (target / 'stdout').read_bytes():
                        process.send_signal(interruption)
                        break
                    if process.poll() is not None:
                        raise AssertionError(f'{name}: process exited before READY')
                    time.sleep(.2)
                else:
                    process.kill()
                    raise AssertionError(f'{name}: no READY within deadline')
            code = process.wait(timeout=360)
        directories = list(state.glob('run-*'))
        assert len(directories) == 1, (name, directories)
        directory = directories[0]
        if interruption == signal.SIGKILL:
            assert code == -signal.SIGKILL, (name, code)
            recovery = subprocess.run([sys.executable, str(cli), 'evals', 'recover', str(directory)],
                                      capture_output=True, timeout=90)
            (target / 'recovery-stdout').write_bytes(recovery.stdout)
            (target / 'recovery-stderr').write_bytes(recovery.stderr)
            assert recovery.returncode == 0, recovery.stderr
            record = json.loads((directory / 'recovery.json').read_text())
            assert record['status'] == 'removed', record
        else:
            record = json.loads((directory / 'result.json').read_text())
            assert code == expected, (name, code, record)
            assert record['outcome'] == outcome, (name, record)
            assert record['cleanup']['status'] == 'removed', (name, record)
        reports.append({'case': name, 'passed': True, 'cli_status': code, 'record': str(directory), 'result': record})
        (root / 'report.json').write_text(json.dumps(reports, indent=2) + '\n')
        print(f'PASS {name}', flush=True)
        return target, record

    target, _ = case('success-and-boundaries', ['/bin/sh', '-c',
        'test "$(uname -sm)" = "Darwin arm64" || exit 41; '
        'test -z "$RBDC_HOST_SECRET" || exit 42; '
        'test ! -d "/Volumes/My Shared Files" || exit 43; '
        'if /usr/bin/curl --noproxy "*" --connect-timeout 3 --max-time 4 -s http://1.1.1.1 >/dev/null; then exit 44; fi; '
        'if /usr/bin/curl --noproxy "*" --connect-timeout 3 --max-time 4 -g -s "http://[2606:4700:4700::1111]" >/dev/null; then exit 45; fi; '
        'echo hello; echo diagnostic >&2; touch /private/tmp/rbdc-reset-sentinel'])
    assert (target / 'stdout').read_bytes() == b'hello\n'
    assert (target / 'state').glob('run-*')
    case('fresh-reset', ['/bin/sh', '-c', 'test ! -e /private/tmp/rbdc-reset-sentinel'])
    target, _ = case('argv', ['/bin/sh', '-c', 'for arg do printf "<%s>\\n" "$arg"; done',
                              'rbdc', 'with space', '"quoted"', '', '$(touch /tmp/no-host-execution);&|'])
    assert (target / 'stdout').read_bytes() == b'<with space>\n<"quoted">\n<>\n<$(touch /tmp/no-host-execution);&|>\n'
    case('exit-seven', ['/bin/sh', '-c', 'echo expected >&2; exit 7'], expected=7, outcome='command_failure')
    case('exit-124', ['/bin/sh', '-c', 'exit 124'], expected=124, outcome='command_failure')
    case('missing-command', ['/definitely/missing/rbdc'], expected=127, outcome='command_failure')
    case('timeout', ['/bin/sh', '-c', 'echo partial; sleep 60'], expected=1, outcome='timeout', timeout=2)
    case('cancel', ['/bin/sh', '-c', 'echo READY; sleep 60'], expected=130, outcome='cancelled', interruption=signal.SIGINT)
    case('forced-kill-recovery', ['/bin/sh', '-c', 'echo READY; sleep 60'], interruption=signal.SIGKILL)
    print(f'{len(reports)} actual macOS cases passed; evidence: {root}', flush=True)


if __name__ == '__main__':
    main()
