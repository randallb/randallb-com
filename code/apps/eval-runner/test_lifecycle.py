import contextlib
import io
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time
import unittest
from unittest.mock import patch

import cli
import lifecycle as vm


class LifecycleTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.directory = Path(self.temporary.name)
        vm.interrupted = 0
        self.record = vm.new_record(sys.executable, 'local:' + 'a' * 64)
        self.machine = vm.Machine(sys.executable, self.directory, self.record)

    def tearDown(self):
        vm.interrupted = 0
        self.temporary.cleanup()

    def test_ownership_durable_before_create_even_when_create_fails(self):
        self.machine.inventory = lambda **kwargs: None
        def failed_create(args, *unused, **kwargs):
            owner = json.loads((self.directory / 'owner.json').read_text())
            self.assertTrue(owner['mutation_started'])
            self.assertEqual(owner['machine'], self.record['machine'])
            raise RuntimeError('injected create failure')
        self.machine.call = failed_create
        with self.assertRaisesRegex(RuntimeError, 'injected create'):
            self.machine.create('/image')
        self.assertTrue((self.directory / 'owner.json').exists())

    def test_owner_sync_failure_prevents_mutation(self):
        self.machine.inventory = lambda **kwargs: None
        with patch.object(self.machine, 'persist', side_effect=OSError('sync failed')), patch.object(self.machine, 'call') as call:
            with self.assertRaises(OSError):
                self.machine.create('/image')
            call.assert_not_called()

    def test_cleanup_inventory_failure_still_stops_exact_owner_and_reports_error(self):
        self.record['mutation_started'] = True
        calls = []
        self.machine.inventory = lambda **kwargs: (_ for _ in ()).throw(RuntimeError('inventory unavailable'))
        self.machine.call = lambda args, *unused, **kwargs: calls.append(args)
        result = self.machine.cleanup()
        self.assertEqual(result['status'], 'failed')
        self.assertEqual(calls, [['stop', '--name', self.record['machine']]])

    def test_cleanup_configuration_mismatch_refuses_mutation(self):
        self.record['mutation_started'] = True
        self.machine.inventory = lambda **kwargs: {'network': True}
        with patch.object(self.machine, 'call') as call:
            result = self.machine.cleanup()
            self.assertEqual(result['status'], 'failed')
            call.assert_not_called()

    def test_cleanup_success_requires_verified_absence(self):
        self.record['mutation_started'] = True
        row = {**self.record['configuration'], 'name': self.record['machine']}
        rows = iter([row, row, row])
        self.machine.inventory = lambda **kwargs: next(rows)
        calls = []
        self.machine.call = lambda args, *unused, **kwargs: calls.append(args)
        result = self.machine.cleanup()
        self.assertEqual(result['status'], 'failed')
        self.assertEqual(calls[-1], ['delete', '--name', self.record['machine'], '--force'])

    def test_recovery_refuses_tampered_machine_or_backend(self):
        self.machine.persist()
        loaded = vm.load_machine(self.directory)
        self.assertEqual(loaded.record['id'], self.record['id'])
        self.record['machine'] = 'somebody-elses-vm'
        self.machine.persist()
        with self.assertRaisesRegex(RuntimeError, 'invalid ownership'):
            vm.load_machine(self.directory)

    def test_active_run_lock_prevents_recovery(self):
        with vm.lock(self.directory):
            with self.assertRaisesRegex(RuntimeError, 'active'):
                with vm.lock(self.directory):
                    pass

    def test_timeout_retains_partial_output_and_reaps_descendant_pipe_holder(self):
        output = self.directory / 'output'
        script = 'import os,time; print("partial",flush=True); pid=os.fork(); time.sleep(30)'
        started = time.monotonic()
        with self.assertRaises(TimeoutError):
            vm.command([sys.executable, '-c', script], .3, output)
        self.assertLess(time.monotonic() - started, 3)
        self.assertEqual(output.with_suffix('.stdout').read_bytes(), b'partial\n')

    def test_cancelled_flag_does_not_block_cleanup_command(self):
        vm.interrupted = signal.SIGINT
        code, stdout, _ = vm.command([sys.executable, '-c', 'print("cleaned")'], 3, cancelling=False)
        self.assertEqual((code, stdout), (0, b'cleaned\n'))

    def test_execution_and_cleanup_failures_both_persist(self):
        image = self.directory / 'image'
        image.write_bytes(b'test fixture')
        args = type('Args', (), dict(command=['--', 'false'], state=str(self.directory), backend=sys.executable,
                                     image=str(image), timeout=5))()
        with patch.object(vm, 'command', return_value=(0, b'smolvm 1.32.2\n', b'')), \
             patch.object(vm.Machine, 'create', side_effect=RuntimeError('create failed')), \
             patch.object(vm.Machine, 'cleanup', return_value={'status': 'failed', 'errors': ['stop failed']}), \
             contextlib.redirect_stderr(io.StringIO()):
            self.assertEqual(cli.run(args), 1)
        result = json.loads(next(self.directory.glob('run-*/result.json')).read_text())
        self.assertEqual(result['outcome'], 'cleanup_failure')
        self.assertEqual(result['execution_outcome'], 'preparation_failure')
        self.assertIn('create failed', result['error'])
        self.assertEqual(result['cleanup']['errors'], ['stop failed'])


if __name__ == '__main__':
    unittest.main()
