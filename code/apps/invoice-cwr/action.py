#!/usr/bin/env python3
"""Fixed CWR action: run the bundled invoice harness and report saved evidence."""
import fcntl
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import threading
import uuid

from install import verify


def supervise(root, output, parent_fd, lock_fd):
    """Survive Shell SIGKILL only long enough to cancel, record and clean the run."""
    # The inherited flock stays held until this process finishes, even if CWR kills
    # its action wrapper. EOF on the pipe detects SIGKILL and closes the spawn race.
    os.fstat(lock_fd)
    config = json.loads((root / 'config.json').read_text())
    evaluation = output / 'evaluation'
    with (output / 'harness.log').open('a') as log:
        env = dict(os.environ)
        env['PATH'] = str(Path(config['tart']).parent) + os.pathsep + env.get('PATH', '')
        command = [config['python'], str(root / 'runner' / 'acceptance_eval.py'),
                   '--payload', config['payload'], '--output', str(evaluation)]
        child = subprocess.Popen(command, env=env, cwd=root, stdout=log, stderr=log,
                                 start_new_session=True)
        cancelled = threading.Event()
        def watch_parent():
            with os.fdopen(parent_fd, 'rb') as parent:
                parent.read(1)
            cancelled.set()
            try:
                os.killpg(child.pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
        threading.Thread(target=watch_parent, daemon=True).start()
        child.wait()
        result_path = evaluation / 'eval-result.json'
        if not result_path.is_file():
            result = {'task': 'invoice-total', 'status': 'error', 'passed': False,
                      'cases_passed': [], 'cases_total': 5,
                      'error': 'harness stopped without a complete evaluation record'}
            evaluation.mkdir(exist_ok=True)
            result_path.write_text(json.dumps(result, indent=2) + '\n')
        else:
            result = json.loads(result_path.read_text())
        solution = evaluation / 'invoice-total.sh'
        status = result['status']
        if status not in ('passed', 'failed', 'error'):
            raise ValueError('harness returned an unknown evaluation status')
        if cancelled.is_set() or (status == 'passed' and child.returncode != 0):
            status = 'error'
            result.update(status='error', passed=False, error='CWR action or harness stopped; inspect runner cleanup evidence')
            result_path.write_text(json.dumps(result, indent=2) + '\n')
        count = len(result['cases_passed'])
        report = {'status': status, 'checks_passed': count, 'checks_total': 5,
                  'summary': f'{status.upper()}: invoice-total, {count}/5 checks',
                  'evidence': {'result': str(result_path), 'result_uri': result_path.as_uri(),
                               'solution': str(solution) if solution.is_file() else None,
                               'solution_uri': solution.as_uri() if solution.is_file() else None,
                               'run_record': result.get('run_record')}}
        if status == 'error':
            report['error'] = result.get('error', 'runner error')
        (output / 'action-report.json').write_text(json.dumps(report, indent=2) + '\n')
        return 1 if status == 'error' else 0


def execute(root):
    os.umask(0o077)
    verify(root)
    if json.load(sys.stdin) != {}:
        raise ValueError('run--invoice takes an empty object')
    config = json.loads((root / 'config.json').read_text())
    with (root / '.invoice.lock').open('a') as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            raise RuntimeError('another invoice evaluation is running') from None
        output = root / 'runs' / uuid.uuid4().hex
        output.mkdir(mode=0o700)
        parent_fd, alive_fd = os.pipe()
        try:
            with (output / 'supervisor.log').open('a') as log:
                child = subprocess.Popen([config['python'], str(root / 'action.py'), '--supervise',
                                          str(output), str(parent_fd), str(lock.fileno())],
                                         pass_fds=(parent_fd, lock.fileno()), start_new_session=True,
                                         stdin=subprocess.DEVNULL, stdout=log, stderr=log, cwd=root)
            os.close(parent_fd)
            parent_fd = None
            child.wait()
        finally:
            os.close(alive_fd)
            if parent_fd is not None:
                os.close(parent_fd)
        report_path = output / 'action-report.json'
        if not report_path.is_file():
            raise RuntimeError('supervisor stopped; inspect ' + str(output))
        print(report_path.read_text(), end='', flush=True)
        return child.returncode


if __name__ == '__main__':
    try:
        root = Path(__file__).resolve().parent
        if len(sys.argv) == 5 and sys.argv[1] == '--supervise':
            raise SystemExit(supervise(root, Path(sys.argv[2]), int(sys.argv[3]), int(sys.argv[4])))
        raise SystemExit(execute(root))
    except (OSError, ValueError, RuntimeError) as error:
        print(json.dumps({'status': 'error', 'summary': 'Invoice evaluation could not run',
                          'error': str(error)}), flush=True)
        raise SystemExit(1)
