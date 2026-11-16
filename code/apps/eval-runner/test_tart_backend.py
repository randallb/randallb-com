import json
import hashlib
import os
from pathlib import Path
import sys
import subprocess
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


class PreparedBaseTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()
        self.home = self.root / 'home'
        self.base = self.home / 'vms/rbdc-base-test-v1'
        self.base.mkdir(parents=True)
        for name in ('config.json', 'disk.img', 'nvram.bin'):
            (self.base / name).write_bytes(('tiny-' + name).encode())
        self.data = {'format': 'rbdc-prepared-base-v1', 'name': self.base.name,
                     'tart_home': str(self.home), 'source_image': tart.DEFAULT_IMAGE,
                     'files': {name: vm.digest(self.base / name) for name in ('config.json', 'disk.img', 'nvram.bin')}}
        self.descriptor = self.root / 'base.json'
        self.write_descriptor()

    def write_descriptor(self):
        self.descriptor.write_text(json.dumps(self.data))

    def test_verified_base_is_unchanged_and_locked_against_tart_start(self):
        before = {name: (self.base / name).read_bytes() for name in self.data['files']}
        probe = '''import os,fcntl,sys
f=os.open(sys.argv[1],os.O_RDWR)
try: fcntl.lockf(f,fcntl.LOCK_EX|fcntl.LOCK_NB)
except BlockingIOError: sys.exit(73)
'''
        with tart.verified_prepared_base(self.descriptor, self.home) as receipt:
            self.assertEqual(receipt['files'], self.data['files'])
            self.assertEqual(receipt['descriptor_sha256'], hashlib.sha256(self.descriptor.read_bytes()).hexdigest())
            result = subprocess.run([sys.executable, '-c', probe, str(self.base / 'config.json')])
            self.assertEqual(result.returncode, 73)
        self.assertEqual(subprocess.run([sys.executable, '-c', probe, str(self.base / 'config.json')]).returncode, 0)
        self.assertEqual({name: (self.base / name).read_bytes() for name in self.data['files']}, before)

    def test_running_source_config_lock_is_rejected(self):
        script = '''import fcntl,sys
with open(sys.argv[1],'r+') as f:
 fcntl.lockf(f,fcntl.LOCK_EX)
 print('locked',flush=True)
 sys.stdin.read()
'''
        child = subprocess.Popen([sys.executable, '-c', script, str(self.base / 'config.json')],
                                 stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
        try:
            self.assertEqual(child.stdout.readline().strip(), 'locked')
            with self.assertRaisesRegex(RuntimeError, 'running'):
                with tart.verified_prepared_base(self.descriptor, self.home):
                    self.fail('running base accepted')
        finally:
            child.communicate('', timeout=5)

    def test_changed_base_file_is_rejected(self):
        (self.base / 'disk.img').write_bytes(b'changed')
        with self.assertRaisesRegex(RuntimeError, 'checksum'):
            with tart.verified_prepared_base(self.descriptor, self.home):
                self.fail('changed base accepted')

    def test_links_and_suspended_sources_are_rejected(self):
        original = (self.base / 'disk.img').read_bytes()
        (self.base / 'disk.img').unlink()
        (self.root / 'outside').write_bytes(original)
        (self.base / 'disk.img').symlink_to(self.root / 'outside')
        with self.assertRaises(RuntimeError):
            with tart.verified_prepared_base(self.descriptor, self.home):
                self.fail('linked source accepted')
        (self.base / 'disk.img').unlink()
        (self.base / 'disk.img').write_bytes(original)
        (self.base / 'state.vzvmsave').write_bytes(b'suspended')
        with self.assertRaisesRegex(RuntimeError, 'unsuspended'):
            with tart.verified_prepared_base(self.descriptor, self.home):
                self.fail('suspended source accepted')

    def test_wrong_home_name_source_and_pin_set_are_rejected(self):
        original = json.loads(json.dumps(self.data))
        for key, value in [('tart_home', str(self.root / 'other')), ('name', '../other'),
                           ('source_image', 'base:latest'), ('files', {'disk.img': 'a' * 64}),
                           ('format', 'other')]:
            with self.subTest(key=key):
                self.data = {**original, key: value}; self.write_descriptor()
                with self.assertRaises(RuntimeError):
                    with tart.verified_prepared_base(self.descriptor, self.home):
                        self.fail('invalid descriptor accepted')

    def test_runner_records_local_clone_and_original_oci_provenance(self):
        from types import SimpleNamespace
        args = SimpleNamespace(command=['/bin/true'], interactive=False, boot_timeout=180,
                               cpus=4, memory=8192, state=str(self.root / 'state'),
                               tart=sys.executable, macos_image=None, tart_home=str(self.home),
                               prepared_base=str(self.descriptor), with_codex=False, reuse_host_auth=False)
        # Spoof only this backend, leaving lifecycle.save on the real host platform.
        with patch.object(tart, 'sys', SimpleNamespace(platform='darwin', stderr=sys.stderr)), \
                patch.object(tart.platform, 'machine', return_value='arm64'), \
                patch.object(tart.platform, 'mac_ver', return_value=('26.5.2', (), '')), \
                patch.object(tart.subprocess, 'check_output', return_value=str(64 * vm.GIB).encode()), \
                patch.object(tart.shutil, 'disk_usage', return_value=SimpleNamespace(free=100 * vm.GIB)), \
                patch.object(vm, 'command', return_value=(0, b'2.41.1', b'')), \
                patch.object(tart, 'Machine') as machine:
            machine.return_value.create.side_effect = RuntimeError('intentional stop before VM creation')
            machine.return_value.cleanup.return_value = {'status': 'not_needed'}
            self.assertEqual(tart.run(args, {'revision': None}), 1)
        self.assertEqual(machine.call_args.args[1]['image'], self.base.name)
        record_path, = (self.root / 'state').glob('run-*/result.json')
        record = json.loads(record_path.read_text())
        self.assertEqual(record['inputs']['image'], tart.DEFAULT_IMAGE)
        self.assertEqual(record['inputs']['prepared_base']['name'], self.base.name)
        self.assertEqual(record['inputs']['prepared_base']['files'], self.data['files'])


if __name__ == '__main__':
    unittest.main()
