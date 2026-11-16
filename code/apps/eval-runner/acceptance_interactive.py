"""Actual Tart PTY input, resize, interruption, timeout, EOF and Codex UI checks."""
import argparse
import fcntl
import json
import os
from pathlib import Path
import pty
import select
import signal
import struct
import subprocess
import sys
import termios
import time


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--output', type=Path, required=True, help='new evidence directory')
    parser.add_argument('--payload', required=True, help='Nix-built Codex payload directory')
    parser.add_argument('--tart-home', type=Path, default=Path('.rbdc-evals/tart'))
    args = parser.parse_args()
    root = args.output.resolve()
    root.mkdir(parents=True, exist_ok=False)
    cli = Path(__file__).with_name('cli.py')
    results = []

    def case(name, command, action, expected, outcome, timeout=60, codex=False):
        directory = root / name
        directory.mkdir()
        argv = [sys.executable, str(cli), 'evals', 'run', '--vm-type', 'tart', '--interactive',
                '--timeout', str(timeout), '--tart-home', str(args.tart_home.resolve()),
                '--state', str(directory / 'state')]
        if codex:
            argv += ['--with-codex', '--codex-payload', args.payload]
        master, slave = pty.openpty()
        original = termios.tcgetattr(slave)
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 24, 80, 0, 0))
        controller = directory / 'terminal-controller.py'
        controller.write_text('import fcntl,json,os,signal,subprocess,sys,termios\n'
            'fcntl.ioctl(0,termios.TIOCSCTTY,0)\n'
            'before=termios.tcgetattr(0)\n'
            f'child=subprocess.Popen({[*argv, "--", *command]!r})\n'
            'signal.signal(signal.SIGTERM,lambda *unused: child.send_signal(signal.SIGTERM))\n'
            'signal.signal(signal.SIGWINCH,lambda *unused: child.send_signal(signal.SIGWINCH))\n'
            'status=child.wait()\n'
            'restored=termios.tcgetattr(0)==before\n'
            f'open({str(directory / "terminal-restoration.json")!r},"w").write(json.dumps({{"restored":restored}}))\n'
            'sys.exit(status)\n')
        process = subprocess.Popen([sys.executable, str(controller)], stdin=slave, stdout=slave, stderr=slave,
                                   env=dict(os.environ, TERM='xterm-256color'), start_new_session=True)
        output = bytearray()

        def drain(seconds=.1):
            if select.select([master], [], [], seconds)[0]:
                try:
                    data = os.read(master, 65536)
                    output.extend(data)
                    return bool(data)
                except OSError:
                    return False
            return False

        def until(marker, deadline=180):
            end = time.monotonic() + deadline
            while marker not in output and time.monotonic() < end:
                drain()
                if process.poll() is not None:
                    break
            assert marker in output, (name, marker, bytes(output)[-2000:])

        try:
            if action == 'shell':
                until(b'sh-3.2$ ')
                os.write(master, b"printf 'INPUT_OK\\n'; test -t 0 && test -t 1; stty size\n")
                until(b'24 80')
                fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 43, 119, 0, 0))
                process.send_signal(signal.SIGWINCH)
                time.sleep(.5)
                os.write(master, b'stty size\n')
                until(b'43 119', 10)
                os.write(master, b'sleep 60\n')
                until(b'sleep 60', 10)
                time.sleep(.3)
                os.write(master, b'\x03')
                time.sleep(.3)
                os.write(master, b"printf 'AFTER_INTERRUPT\\n'; exit 7\n")
            elif action == 'eof':
                until(b'sh-3.2$ ')
                os.write(master, b'\x04')
            elif action == 'terminate':
                until(b'READY')
                process.send_signal(signal.SIGTERM)
            elif action == 'timeout':
                until(b'READY')
            elif action == 'codex':
                until(b'Sign in with ChatGPT')
                os.write(master, b'\x1b[B')
                time.sleep(.2)
                os.write(master, b'\x03')
            end = time.monotonic() + 100
            while process.poll() is None and time.monotonic() < end:
                drain()
            assert process.poll() is not None, (name, 'runner did not finish')
            while select.select([master], [], [], .1)[0]:
                if not drain(0):
                    break
            assert json.loads((directory / 'terminal-restoration.json').read_text())['restored'], (name, 'terminal settings not restored')
            assert process.returncode == expected, (name, process.returncode, bytes(output)[-2000:])
            record_path, = (directory / 'state').glob('run-*/result.json')
            record = json.loads(record_path.read_text())
            assert record['outcome'] == outcome and record['cleanup']['status'] == 'removed', record
            assert record['output_capture'] == 'live_terminal'
            if action == 'shell':
                assert b'AFTER_INTERRUPT\r\n' in output
            results.append({'case': name, 'passed': True, 'terminal_restored': True, 'result': record})
            (root / 'report.json').write_text(json.dumps(results, indent=2) + '\n')
            print('PASS ' + name, flush=True)
        finally:
            (directory / 'terminal.cast').write_bytes(output)
            if process.poll() is None:
                process.send_signal(signal.SIGTERM)
                try:
                    process.wait(timeout=90)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()
            os.close(master)
            os.close(slave)

    case('shell-input-resize-ctrl-c-exit', ['/bin/sh', '-i'], 'shell', 7, 'command_failure')
    case('shell-eof', ['/bin/sh', '-i'], 'eof', 0, 'success')
    case('interactive-timeout', ['/bin/sh', '-c', 'echo READY; sleep 60'], 'timeout', 1, 'timeout', timeout=2)
    case('host-cancellation', ['/bin/sh', '-c', 'echo READY; sleep 60'], 'terminate', 143, 'cancelled')
    case('codex-welcome-exit', ['codex', '--no-daemon'], 'codex', 0, 'success', codex=True)
    print(f'{len(results)} actual interactive VM cases passed', flush=True)


if __name__ == '__main__':
    main()
