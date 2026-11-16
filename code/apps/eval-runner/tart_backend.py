"""Disposable macOS guests. No shared host folders, NAT or SSH transport."""
import json
import os
from pathlib import Path
import platform
import re
import shutil
import signal
import socket
import subprocess
import sys
import threading
import time
import uuid

import lifecycle as vm

DEFAULT_IMAGE = 'ghcr.io/cirruslabs/macos-tahoe-base@sha256:65c3133a633fda8753cf1f71f2d55727792d534a2125b0032466563dcc8e58fd'
VERSION = '2.41.1'


def image_reference(value):
    if not re.fullmatch(r'[a-zA-Z0-9./_:-]+@sha256:[a-f0-9]{64}', value):
        raise RuntimeError('macOS image must be an OCI reference pinned with @sha256:<64 hex digits>')
    return value


class Machine:
    def __init__(self, directory, record):
        self.directory = Path(directory)
        self.record = record
        self.process = None
        self.sockets = []
        self.logs = []
        self.sequence = len(record.get('operations', []))

    def persist(self):
        vm.save(self.directory / 'owner.json', self.record)

    @property
    def environment(self):
        return {'TART_HOME': self.record['tart_home'], 'TART_NO_AUTO_PRUNE': '1'}

    @property
    def machine_path(self):
        return Path(self.record['tart_home']) / 'vms' / self.record['machine']

    def call(self, args, timeout=30, check=True, stream=False, cancelling=True):
        self.sequence += 1
        output = self.directory / f'backend-{self.sequence:03d}-{args[0]}'
        entry = {'argv': args, 'started': time.time()}
        self.record.setdefault('operations', []).append(entry)
        self.persist()
        try:
            status, stdout, stderr = vm.command([self.record['backend'], *args], timeout,
                                               output=output, stream=stream, cancelling=cancelling,
                                               environment=self.environment)
            entry['exit_status'] = status
            if status and check:
                raise RuntimeError(f'Tart {args[0]} failed ({status}): {stderr.decode(errors="replace")[-2000:]}')
            return status, stdout, stderr, output
        except Exception as error:
            entry['error'] = str(error)
            raise
        finally:
            entry['elapsed_seconds'] = time.time() - entry['started']
            self.persist()

    def exists(self):
        path = self.machine_path
        if path.is_symlink():
            raise RuntimeError('owned VM directory is a symlink; refusing mutation')
        return path.exists()

    def validate(self):
        if self.record.get('config_sha256'):
            if vm.digest(self.machine_path / 'config.json') != self.record['config_sha256']:
                raise RuntimeError('owned VM configuration changed; refusing mutation')

    def create(self, cpus, memory):
        if self.exists():
            raise RuntimeError('owned VM name collision; refusing adoption')
        self.record['mutation_started'] = True
        self.persist()
        self.call(['clone', self.record['image'], self.record['machine']], timeout=3600)
        self.call(['set', self.record['machine'], '--cpu', str(cpus), '--memory', str(memory)])
        self.record['config_sha256'] = vm.digest(self.machine_path / 'config.json')
        self.record['effective_configuration'] = json.loads((self.machine_path / 'config.json').read_text())
        self.persist()

    def start(self, boot_timeout):
        self.validate()
        guest, sink = socket.socketpair(socket.AF_UNIX, socket.SOCK_DGRAM)
        self.sockets = [guest, sink]
        sink.settimeout(.5)

        def discard():
            while True:
                try:
                    sink.recv(65536)
                except socket.timeout:
                    continue
                except OSError:
                    return

        threading.Thread(target=discard, daemon=True).start()
        args = [self.record['backend'], 'run', self.record['machine'], '--no-graphics',
                '--no-audio', '--no-usb-accessories', '--no-clipboard', '--net-fd', str(guest.fileno())]
        self.record['launch_argv'] = args
        self.persist()
        self.logs = [(self.directory / ('vm-' + suffix)).open('wb') for suffix in ('stdout', 'stderr')]
        env = {key: os.environ[key] for key in ('HOME', 'PATH', 'TMPDIR') if key in os.environ}
        env.update(self.environment)
        self.process = subprocess.Popen(args, stdin=subprocess.DEVNULL, stdout=self.logs[0], stderr=self.logs[1],
                                        env=env, pass_fds=(guest.fileno(),), start_new_session=True)
        self.record['launch_pid'] = self.process.pid
        self.persist()
        guest.close()
        deadline = time.monotonic() + boot_timeout
        while time.monotonic() < deadline:
            if vm.interrupted:
                raise vm.Cancelled()
            if self.process.poll() is not None:
                detail = (self.directory / 'vm-stderr').read_text(errors='replace')[-2000:]
                raise RuntimeError(f'Tart VM exited during boot: {detail}')
            try:
                status, _, _, _ = self.call(['exec', self.record['machine'], '/usr/bin/true'], timeout=5, check=False)
                if status == 0:
                    return
            except TimeoutError:
                pass
            time.sleep(.5)
        raise TimeoutError(f'macOS guest agent not ready within {boot_timeout}s')

    def cleanup(self):
        errors = []
        if not self.record.get('mutation_started'):
            return {'status': 'not_needed', 'errors': []}
        try:
            if self.exists():
                self.validate()
                self.call(['stop', self.record['machine'], '--timeout', '10'], timeout=20,
                          check=False, cancelling=False)
                if self.process:
                    self.process.wait(timeout=10)
                self.call(['delete', self.record['machine']], timeout=30, cancelling=False)
            if self.exists():
                raise RuntimeError('owned VM remains after cleanup')
        except Exception as error:
            errors.append(str(error))
        finally:
            if self.process and self.process.poll() is None:
                try:
                    os.killpg(self.process.pid, signal.SIGKILL)
                    self.process.wait(timeout=10)
                except Exception as error:
                    errors.append(str(error))
            for sock in self.sockets:
                sock.close()
            for handle in self.logs:
                handle.close()
        return {'status': 'failed' if errors else 'removed', 'errors': errors}


def recover(directory):
    owner = json.loads((directory / 'owner.json').read_text())
    identity = owner.get('id', '')
    if (owner.get('schema') != 1 or owner.get('vm_type') != 'tart'
            or not re.fullmatch('[a-f0-9]{32}', identity)
            or owner.get('machine') != 'rbdc-evals-tart-' + identity
            or not Path(owner['tart_home']).is_absolute()
            or str(Path(owner['tart_home']).resolve()) != owner['tart_home']):
        raise RuntimeError('invalid Tart ownership record; refusing recovery')
    if vm.digest(owner['backend']) != owner['backend_sha256']:
        raise RuntimeError('backend identity changed; refusing recovery')
    with vm.lock(Path(owner['tart_home']) / 'workload-lock'):
        result = Machine(directory, owner).cleanup()
    vm.save(directory / 'recovery.json', result)
    print(json.dumps(result))
    return int(result['status'] == 'failed')


def run(args, source):
    argv = args.command[1:] if args.command[:1] == ['--'] else args.command
    if not argv:
        raise RuntimeError('supply a guest command after --')
    if not 1 <= args.boot_timeout <= 1800 or not 2 <= args.cpus <= 64 or not 4096 <= args.memory <= 65536:
        raise RuntimeError('require boot timeout 1..1800s, CPUs 2..64 and memory 4096..65536 MiB')
    state = Path(args.state).expanduser().resolve()
    state.mkdir(parents=True, exist_ok=True, mode=0o700)
    directory = state / ('run-' + uuid.uuid4().hex)
    directory.mkdir(mode=0o700)
    report = {'schema': 1, 'vm_type': 'tart', 'argv': argv, 'source': source,
              'guest_exit_status': None, 'outcome': 'preparation_failure'}
    machine = None
    print(f'rbdc: run records: {directory}', file=sys.stderr, flush=True)
    with vm.lock(directory):
        try:
            if sys.platform != 'darwin' or platform.machine() != 'arm64' or int(platform.mac_ver()[0].split('.')[0]) < 14:
                raise RuntimeError('Tart execution requires Apple Silicon macOS 14 or newer')
            physical_memory = int(subprocess.check_output(['/usr/sbin/sysctl', '-n', 'hw.memsize']))
            if args.cpus > (os.cpu_count() or 1) or args.memory * 1024 ** 2 + 4 * vm.GIB > physical_memory:
                raise RuntimeError('requested resources exceed host capacity with 4 GiB reserved for the host')
            report['host'] = {'architecture': platform.machine(), 'macos': platform.mac_ver()[0],
                              'physical_memory': physical_memory, 'cpu_count': os.cpu_count()}
            backend = shutil.which(args.tart)
            if not backend:
                raise RuntimeError('Tart missing; install with brew install openai/tools/tart')
            backend = str(Path(backend).resolve())
            status, version, _ = vm.command([backend, '--version'], 10)
            if status or version.decode().strip() != VERSION:
                raise RuntimeError(f'require Tart {VERSION}')
            image = image_reference(args.macos_image or DEFAULT_IMAGE)
            home = Path(args.tart_home).expanduser().resolve()
            home.mkdir(parents=True, exist_ok=True, mode=0o700)
            if shutil.disk_usage(home).free < 40 * vm.GIB:
                raise RuntimeError('at least 40 GiB free disk required for macOS image and writable state')
            identity = uuid.uuid4().hex
            owner = {'schema': 1, 'vm_type': 'tart', 'kind': 'run', 'id': identity,
                     'machine': 'rbdc-evals-tart-' + identity, 'backend': backend,
                     'backend_sha256': vm.digest(backend), 'backend_version': VERSION,
                     'tart_home': str(home), 'image': image, 'mutation_started': False,
                     'configuration': {'cpus': args.cpus, 'memory_mib': args.memory,
                                       'network': 'discard-only-socket', 'mounts': 0,
                                       'clipboard': False, 'audio': False, 'usb': False}}
            machine = Machine(directory, owner)
            machine.persist()
            report.update(run_id=identity, machine=owner['machine'], backend_version=VERSION,
                          inputs={'image': image, 'fixtures': {}}, resources=owner['configuration'])
            # One workload per dedicated Tart home; no persistent local base VM.
            with vm.lock(home / 'workload-lock'):
                try:
                    stale = list((home / 'vms').glob('rbdc-evals-tart-*'))
                    if stale:
                        raise RuntimeError('previous owned Tart VM remains; recover its run record before starting another')
                    machine.create(args.cpus, args.memory)
                    report['outcome'] = 'boot_failure'
                    boot_start = time.monotonic()
                    machine.start(args.boot_timeout)
                    report['boot_seconds'] = time.monotonic() - boot_start
                    report['outcome'] = 'transport_failure'
                    _, system, _, _ = machine.call(['exec', owner['machine'], '/usr/bin/uname', '-sm'])
                    if system.strip() != b'Darwin arm64':
                        raise RuntimeError('expected macOS ARM64 guest identity')
                    report['guest_system'] = system.decode().strip()
                    # A retained guest completion marker distinguishes guest status from transport failure.
                    marker = '/private/tmp/rbdc-' + identity + '.exit'
                    script = ('cd /private/tmp || exit 125; umask 077; '
                              '/usr/bin/env -i PATH=/usr/bin:/bin:/usr/sbin:/sbin:/opt/homebrew/bin '
                              'HOME=/private/tmp LANG=en_US.UTF-8 "$@"; status=$?; '
                              'printf "%s\\n" "$status" > ' + marker + '; exit "$status"')
                    code, _, _, output = machine.call(['exec', owner['machine'], '/bin/sh', '-c', script, 'rbdc', *argv],
                                                       timeout=args.timeout, check=False, stream=True)
                    for suffix in ('stdout', 'stderr'):
                        shutil.copyfile(str(output) + '.' + suffix, directory / ('guest-' + suffix))
                    _, completed, _, _ = machine.call(['exec', owner['machine'], '/bin/cat', marker])
                    guest_status = int(completed.strip())
                    if not 0 <= guest_status <= 255 or guest_status != code:
                        raise RuntimeError('transport exit disagrees with retained guest status')
                    report['guest_exit_status'] = guest_status
                    report['outcome'] = 'success' if code == 0 else 'command_failure'
                finally:
                    report['cleanup'] = machine.cleanup()
        except TimeoutError as error:
            report.update(outcome='timeout', error=str(error))
        except vm.Cancelled:
            report.update(outcome='cancelled', signal=vm.interrupted)
        except Exception as error:
            report['error'] = str(error)
            print(f'rbdc: {error}', file=sys.stderr)
        finally:
            report.setdefault('cleanup', {'status': 'not_needed', 'errors': []})
            report['execution_outcome'] = report['outcome']
            if report['cleanup']['status'] == 'failed':
                report['outcome'] = 'cleanup_failure'
            vm.save(directory / 'result.json', report)
            print(f'rbdc: {report["outcome"]}; guest={report["guest_exit_status"]}; cleanup={report["cleanup"]["status"]}', file=sys.stderr)
    if report['outcome'] == 'success':
        return 0
    if report['outcome'] == 'command_failure':
        return report['guest_exit_status']
    if report['outcome'] == 'cancelled':
        return 128 + report['signal']
    return 1
