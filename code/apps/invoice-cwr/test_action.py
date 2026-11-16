"""Process-boundary tests: stopped Shell wrappers still permit runner cleanup."""
import fcntl
import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import unittest


class ActionProcessTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        source = Path(__file__).resolve().parent
        for name in ('action.py', 'install.py', 'invoke.py'):
            shutil.copyfile(source / name, self.root / name)
        (self.root / 'runner').mkdir()
        (self.root / 'runs').mkdir()
        (self.root / 'config.json').write_text(json.dumps({
            'python': sys.executable, 'tart': '/bin/true', 'payload': 'unused'}))

    def fixture(self, long=False):
        (self.root / 'runner' / 'acceptance_eval.py').write_text('''
import argparse, json, os, signal, time
from pathlib import Path
p=argparse.ArgumentParser(); p.add_argument('--output',type=Path); p.add_argument('--payload')
a=p.parse_args(); a.output.mkdir(); stopped=False
def stop(*args):
 global stopped
 stopped=True
signal.signal(signal.SIGTERM,stop)
(a.output/'started').write_text(str(os.getpid()))
''' + ('''
while not stopped: time.sleep(0.01)
time.sleep(0.3)
''' if long else '') + '''
(a.output/'invoice-total.sh').write_text('echo 0\\n')
(a.output/'eval-result.json').write_text(json.dumps({'status':'error' if stopped else 'failed',
 'cases_passed':['empty'],'error':'cancelled' if stopped else None,'cases_total':5,
 'runner_result':{'cleanup':{'status':'removed'}}}))
(a.output/'cleaned').write_text('yes')
''')
        inventory = {str(p.relative_to(self.root)): hashlib.sha256(p.read_bytes()).hexdigest()
                     for p in self.root.rglob('*.py')}
        (self.root / 'bundle.json').write_text(json.dumps({'format': 'invoice-cwr-local-v1', 'files': inventory}))

    def wait_for(self, pattern):
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            found = list(self.root.glob(pattern))
            if found:
                return found[0]
            time.sleep(0.01)
        self.fail('timed out waiting for ' + pattern)

    def test_failed_grade_is_a_completed_action_with_retained_evidence(self):
        self.fixture()
        run = subprocess.run([sys.executable, str(self.root / 'action.py')], input='{}',
                             text=True, capture_output=True, timeout=15)
        self.assertEqual(run.returncode, 0, run.stderr)
        report = json.loads(run.stdout)
        self.assertEqual(report['status'], 'failed')
        self.assertEqual(report['checks_passed'], 1)
        self.assertTrue(Path(report['evidence']['solution']).is_file())

    def test_sigkill_to_cwr_wrapper_still_cleans_and_retains_lock(self):
        self.fixture(long=True)
        with (self.root / 'wrapper.stdout').open('w') as log:
            run = subprocess.Popen([sys.executable, str(self.root / 'action.py')], stdin=subprocess.PIPE,
                                   stdout=log, stderr=log, text=True, start_new_session=True)
            run.stdin.write('{}'); run.stdin.close()
            self.wait_for('runs/*/evaluation/started')
            os.killpg(run.pid, signal.SIGKILL)
            run.wait(timeout=5)
        with (self.root / '.invoice.lock').open('a') as lock:
            with self.assertRaises(BlockingIOError):
                fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        report = self.wait_for('runs/*/action-report.json')
        self.assertEqual(json.loads(report.read_text())['status'], 'error')
        self.assertTrue(list(self.root.glob('runs/*/evaluation/cleaned')))
        with (self.root / '.invoice.lock').open('a') as lock:
            deadline = time.monotonic() + 5
            while True:
                try:
                    fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
                    break
                except BlockingIOError:
                    if time.monotonic() > deadline:
                        raise
                    time.sleep(0.01)

    def test_overlapping_launcher_never_attempts_runtime_shutdown(self):
        self.fixture()
        with (self.root / '.invoke.lock').open('a') as lock:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            run = subprocess.run([sys.executable, str(self.root / 'invoke.py')],
                                 text=True, capture_output=True, timeout=5)
        self.assertEqual(run.returncode, 1)
        self.assertIn('already running', run.stderr)
        self.assertNotIn('Traceback', run.stderr)


if __name__ == '__main__':
    unittest.main()
