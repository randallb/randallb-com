import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

import lifecycle as vm
import tart_backend as tart


class TartTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name).resolve()
        self.identity = 'a' * 32
        self.owner = {'schema': 1, 'vm_type': 'tart', 'id': self.identity,
                      'machine': 'rbdc-evals-tart-' + self.identity, 'tart_home': str(self.root / 'home'),
                      'backend': sys.executable, 'backend_sha256': vm.digest(sys.executable),
                      'image': tart.DEFAULT_IMAGE, 'mutation_started': False}
        self.machine = tart.Machine(self.root, self.owner)

    def tearDown(self):
        self.temp.cleanup()

    def test_mutable_or_malformed_images_are_rejected(self):
        for image in ['base:latest', 'base@sha256:1234', '--overwrite', '../foo']:
            with self.assertRaises(RuntimeError):
                tart.image_reference(image)
        self.assertEqual(tart.image_reference(tart.DEFAULT_IMAGE), tart.DEFAULT_IMAGE)

    def test_collision_never_adopts_or_deletes_existing_vm(self):
        self.machine.machine_path.mkdir(parents=True)
        with patch.object(self.machine, 'call') as call:
            with self.assertRaisesRegex(RuntimeError, 'collision'):
                self.machine.create(4, 8192)
            self.assertEqual(self.machine.cleanup()['status'], 'not_needed')
            call.assert_not_called()

    def test_failed_clone_has_durable_ownership_and_cleanup(self):
        def fail(args, **kwargs):
            owner = json.loads((self.root / 'owner.json').read_text())
            self.assertTrue(owner['mutation_started'])
            self.assertEqual(owner['machine'], self.owner['machine'])
            raise RuntimeError('clone interrupted')
        with patch.object(self.machine, 'call', side_effect=fail):
            with self.assertRaisesRegex(RuntimeError, 'clone interrupted'):
                self.machine.create(4, 8192)
        self.assertEqual(self.machine.cleanup()['status'], 'removed')

    def test_cleanup_refuses_changed_configuration(self):
        self.owner['mutation_started'] = True
        self.machine.machine_path.mkdir(parents=True)
        config = self.machine.machine_path / 'config.json'
        config.write_text('original')
        self.owner['config_sha256'] = vm.digest(config)
        config.write_text('changed')
        with patch.object(self.machine, 'call') as call:
            self.assertEqual(self.machine.cleanup()['status'], 'failed')
            call.assert_not_called()

    def test_cleanup_failure_is_visible_and_retains_owner(self):
        self.owner['mutation_started'] = True
        self.machine.machine_path.mkdir(parents=True)
        self.machine.persist()
        with patch.object(self.machine, 'call', side_effect=RuntimeError('stop failed')):
            result = self.machine.cleanup()
        self.assertEqual(result['status'], 'failed')
        self.assertTrue((self.root / 'owner.json').exists())
        self.assertTrue(self.machine.machine_path.exists())

    def test_recovery_refuses_tampered_machine_name_and_backend(self):
        self.owner['machine'] = 'unrelated-vm'
        self.machine.persist()
        with self.assertRaisesRegex(RuntimeError, 'invalid Tart ownership'):
            tart.recover(self.root)
        self.owner['machine'] = 'rbdc-evals-tart-' + self.identity
        self.owner['backend_sha256'] = '0' * 64
        self.machine.persist()
        with self.assertRaisesRegex(RuntimeError, 'backend identity changed'):
            tart.recover(self.root)

    def test_shared_network_uses_builtin_attachment_and_offline_keeps_sink(self):
        for mode in ('shared-nat', 'discard-only-socket'):
            with self.subTest(mode=mode):
                self.owner['configuration'] = {'network': mode}
                machine = tart.Machine(self.root, self.owner)
                try:
                    with patch.object(machine, 'validate'), patch.object(machine, 'call', return_value=(0, b'', b'', None)), patch.object(tart.subprocess, 'Popen') as launch:
                        launch.return_value.pid = 12345
                        launch.return_value.poll.return_value = None
                        machine.start(1)
                    argv = launch.call_args.args[0]
                    self.assertEqual('--net-fd' in argv, mode == 'discard-only-socket')
                    self.assertEqual(bool(launch.call_args.kwargs['pass_fds']), mode == 'discard-only-socket')
                    self.assertNotIn('--net-softnet', argv)
                    self.assertNotIn('--dir', argv)
                    self.assertIn('--no-clipboard', argv)
                finally:
                    for sock in machine.sockets:
                        sock.close()
                    for handle in machine.logs:
                        handle.close()

    def test_tart_state_is_explicit_and_auto_pruning_disabled(self):
        with patch.object(vm, 'command', return_value=(0, b'', b'')) as command:
            self.machine.call(['list', '--format', 'json'])
        self.assertEqual(command.call_args.kwargs['environment'],
                         {'TART_HOME': self.owner['tart_home'], 'TART_NO_AUTO_PRUNE': '1'})


if __name__ == '__main__':
    unittest.main()
