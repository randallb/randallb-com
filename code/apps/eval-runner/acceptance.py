"""Live public CLI acceptance. Requires a built image and pinned SmolVM.

Never substitutes a fake VM. Writes selected raw transcripts and a JSON report.
"""
import argparse
import json
import os
from pathlib import Path
import signal
import subprocess
import time


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    cases = []
    def invoke(name, argv, expected, *, options=(), interrupt=None):
        command = ['rbdc', 'evals', 'run', *options, '--', *argv]
        stdout_path, stderr_path = output / (name + '.stdout'), output / (name + '.stderr')
        with stdout_path.open('wb') as stdout, stderr_path.open('wb') as stderr:
            child = subprocess.Popen(command, stdout=stdout, stderr=stderr, start_new_session=True,
                                     env={**os.environ, 'RBDC_HOST_SENTINEL': 'must-not-cross'})
            if interrupt:
                deadline = time.monotonic() + 120
                while b'ready' not in stdout_path.read_bytes():
                    if child.poll() is not None or time.monotonic() > deadline:
                        raise RuntimeError(f'{name}: guest never reached ready')
                    time.sleep(.1)
                child.send_signal(interrupt)
                if interrupt == signal.SIGKILL:
                    child.wait(timeout=10)
                    record = record_path(stderr_path)
                    recovery = subprocess.run(['rbdc', 'evals', 'recover', str(record)], capture_output=True, timeout=90)
                    (output / (name + '.recovery')).write_bytes(recovery.stdout + recovery.stderr)
                    assert recovery.returncode == 0
            code = child.wait(timeout=180)
        assert code == expected, (name, code, stderr_path.read_text())
        directory = record_path(stderr_path)
        if interrupt == signal.SIGKILL:
            result = json.loads((directory / 'recovery.json').read_text())
            assert result['status'] == 'removed'
        else:
            result = json.loads((directory / 'result.json').read_text())
            assert result['cleanup']['status'] in ('removed', 'not_needed'), result
        case = {'case': name, 'argv': command, 'exit_status': code, 'record': str(directory), 'result': result}
        cases.append(case)
        print(f'{name}: passed', flush=True)
        return result, stdout_path.read_bytes(), stderr_path.read_bytes()
    def record_path(stderr_path):
        line = next(line for line in stderr_path.read_text().splitlines() if line.startswith('rbdc: run records: '))
        return Path(line.removeprefix('rbdc: run records: '))
    result, stdout, _ = invoke('hello', ['sh', '-c', 'echo hello; uname -sm; echo guest-stderr >&2'], 0)
    assert b'hello\nLinux aarch64\n' == stdout
    first, _, _ = invoke('write', ['sh', '-c', 'echo fixture > /tmp/rbdc-reset-fixture; cat /tmp/rbdc-reset-fixture'], 0)
    second, _, _ = invoke('reset', ['sh', '-c', 'test ! -e /tmp/rbdc-reset-fixture; echo fresh'], 0)
    assert first['inputs']['image_sha256'] == second['inputs']['image_sha256']
    assert first['machine'] != second['machine']
    arguments = ['with spaces', 'single\'quote', 'double"quote', '', '$(touch /tmp/rbdc-host-expanded); & | < > *']
    _, stdout, _ = invoke('argv', ['python3', '-c', 'import json,sys; print(json.dumps(sys.argv[1:]))', *arguments], 0)
    assert json.loads(stdout) == arguments
    failure, _, _ = invoke('exit7', ['sh', '-c', 'echo expected-error >&2; exit 7'], 7)
    assert failure['guest_exit_status'] == 7 and failure['outcome'] == 'command_failure'
    invoke('invalid-command', ['rbdc-executable-does-not-exist'], 127)
    invoke('missing-backend', ['true'], 1, options=['--backend', '/rbdc-missing-backend'])
    image = output / 'invalid-image.tar'
    image.write_bytes(b'invalid image fixture')
    invoke('invalid-image', ['true'], 1, options=['--image', str(image)])
    invoke('interrupt', ['sh', '-c', 'echo ready; sleep 60'], 130, interrupt=signal.SIGINT)
    invoke('forced-kill', ['sh', '-c', 'echo ready; sleep 60'], -signal.SIGKILL, interrupt=signal.SIGKILL)
    network = '''import os,socket
assert 'RBDC_HOST_SENTINEL' not in os.environ
s=socket.socket(); s.settimeout(1)
try: s.connect(('1.1.1.1',80))
except OSError: print('network unavailable; host environment absent')
else: raise RuntimeError('unexpected workload egress')
'''
    invoke('boundaries', ['python3', '-c', network], 0)
    (output / 'report.json').write_text(json.dumps({'cases': cases, 'status': 'passed'}, indent=2) + '\n')


if __name__ == '__main__':
    main()
