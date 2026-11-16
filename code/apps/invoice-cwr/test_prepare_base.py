"""Force-killing the setup CLI still stops its controlled fake VM process."""
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time
import unittest


@unittest.skipUnless(Path('/usr/bin/expect').is_file(), 'macOS setup prerequisite')
class PreparationCancellationTests(unittest.TestCase):
    def test_cli_death_cancels_preparation_and_reaps_owned_vm(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            tools = root / 'bin'; tools.mkdir()
            tart = tools / 'tart'
            tart.write_text('#!' + sys.executable + '\n' + '''
import os, signal, sys, time
from pathlib import Path
h=Path(os.environ['TART_HOME']); args=sys.argv[1:]
if args[0]=='clone':
 p=h/'vms'/args[2];p.mkdir(parents=True)
 for name in ('config.json','disk.img','nvram.bin'): (p/name).write_text('fixture')
elif args[0]=='run':
 h.mkdir(exist_ok=True);(h/'started').write_text(str(os.getpid()))
 def stop(*_):
  (h/'stopped').write_text('yes');sys.exit(0)
 signal.signal(signal.SIGTERM,stop)
 while True: time.sleep(.05)
elif args[0]=='ip':
 time.sleep(120)
elif args[0]=='stop':
 os.kill(int((h/'started').read_text()),signal.SIGTERM)
''')
            tart.chmod(0o700)
            home = root / 'tart-state'
            output = root / 'base.json'
            env = dict(os.environ, PATH=str(tools) + os.pathsep + os.environ['PATH'])
            script = Path(__file__).with_name('prepare_base.py')
            with (root / 'log').open('w') as log:
                child = subprocess.Popen([sys.executable, str(script), '--tart-home', str(home),
                                          '--output', str(output)], env=env, stdout=log, stderr=log,
                                         start_new_session=True)
                deadline = time.monotonic() + 10
                while not (home / 'started').exists():
                    if time.monotonic() > deadline:
                        child.kill();child.wait();self.fail('fake VM did not start')
                    time.sleep(.02)
                os.kill(child.pid, signal.SIGKILL)
                child.wait(timeout=5)
            result = output.with_name(output.name + '.preparation') / 'result.json'
            deadline = time.monotonic() + 10
            while not result.exists():
                if time.monotonic() > deadline:
                    self.fail('setup worker did not finish cleanup')
                time.sleep(.02)
            self.assertFalse(json.loads(result.read_text())['prepared'])
            self.assertTrue((home / 'stopped').is_file())
            self.assertFalse(output.exists())
            pid = int((home / 'started').read_text())
            with self.assertRaises(ProcessLookupError):
                os.kill(pid, 0)


if __name__ == '__main__':
    unittest.main()
