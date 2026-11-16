"""Disposable macOS guests. No shared host folders or SSH transport; shared NAT is opt-in."""
import contextlib
import fcntl
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import shutil
import shlex
import signal
import socket
import stat
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


@contextlib.contextmanager
def verified_prepared_base(descriptor_path, home):
    """Verify and read-lock a stopped local base without changing its contents.

    Tart 2.41.1 uses a POSIX whole-file write lock on config.json while running.
    Our shared POSIX lock prevents a source VM starting while it is verified and
    cloned. Hash through the existing descriptors: closing another descriptor to
    config.json would release this process's POSIX lock.
    """
    descriptor_path, home = Path(descriptor_path).expanduser(), Path(home).resolve()
    if not stat.S_ISREG(descriptor_path.lstat().st_mode):
        raise RuntimeError('prepared-base descriptor must be an ordinary file')
    with os.fdopen(os.open(descriptor_path, os.O_RDONLY | os.O_NOFOLLOW), 'rb') as source:
        raw = source.read(65537)
    if len(raw) > 65536:
        raise RuntimeError('prepared-base descriptor exceeds 64 KiB')
    descriptor = json.loads(raw)
    required = {'format', 'name', 'tart_home', 'source_image', 'files'}
    if not isinstance(descriptor, dict) or set(descriptor) != required or descriptor['format'] != 'rbdc-prepared-base-v1':
        raise RuntimeError('invalid prepared-base descriptor format')
    base_name = descriptor['name']
    if not isinstance(base_name, str) or not re.fullmatch(r'rbdc-base-[a-z0-9-]+', base_name):
        raise RuntimeError('invalid prepared-base local name')
    if not isinstance(descriptor['tart_home'], str) or Path(descriptor['tart_home']) != home:
        raise RuntimeError('prepared-base Tart home differs from this run')
    source_image = descriptor['source_image']
    if not isinstance(source_image, str):
        raise RuntimeError('prepared-base source image must be a pinned OCI reference')
    image_reference(source_image)
    files = descriptor['files']
    if (not isinstance(files, dict) or set(files) != {'config.json', 'disk.img', 'nvram.bin'}
            or any(not isinstance(value, str) or not re.fullmatch('[a-f0-9]{64}', value) for value in files.values())):
        raise RuntimeError('prepared-base requires exactly three SHA-256 file pins')
    base = home / 'vms' / base_name
    if (home.is_symlink() or (home / 'vms').is_symlink() or base.is_symlink()
            or not base.is_dir() or base.resolve() != base):
        raise RuntimeError('prepared-base must be an ordinary local VM directory')
    if (base / 'state.vzvmsave').exists() or (base / 'state.vzvmsave').is_symlink() or (base / 'overlay.asif').exists():
        raise RuntimeError('prepared-base must be stopped, unsuspended and standalone')
    with contextlib.ExitStack() as stack:
        handles = {}
        for filename in ('config.json', 'disk.img', 'nvram.bin'):
            path = base / filename
            info = path.lstat()
            if not stat.S_ISREG(info.st_mode) or info.st_nlink != 1:
                raise RuntimeError('prepared-base contains a link or nonregular required file')
            handle = stack.enter_context(os.fdopen(os.open(path, os.O_RDONLY | os.O_NOFOLLOW), 'rb'))
            opened = os.fstat(handle.fileno())
            if (opened.st_dev, opened.st_ino) != (info.st_dev, info.st_ino):
                raise RuntimeError('prepared-base file changed during verification')
            handles[filename] = handle
        try:
            fcntl.lockf(handles['config.json'].fileno(), fcntl.LOCK_SH | fcntl.LOCK_NB)
        except BlockingIOError:
            raise RuntimeError('prepared-base is running or its config lock is busy') from None
        if (base / 'state.vzvmsave').exists() or (base / 'state.vzvmsave').is_symlink():
            raise RuntimeError('prepared-base became suspended before its lock was acquired')
        observed = {}
        for filename, handle in handles.items():
            checksum = hashlib.sha256()
            while True:
                if vm.interrupted:
                    raise vm.Cancelled()
                block = handle.read(1024 * 1024)
                if not block:
                    break
                checksum.update(block)
            observed[filename] = checksum.hexdigest()
        if observed != files:
            raise RuntimeError('prepared-base file checksum mismatch')
        yield {**descriptor, 'descriptor': str(descriptor_path.resolve()),
               'descriptor_sha256': hashlib.sha256(raw).hexdigest()}


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

    def call(self, args, timeout=30, check=True, stream=False, cancelling=True, input_path=None):
        self.sequence += 1
        output = self.directory / f'backend-{self.sequence:03d}-{args[0]}'
        entry = {'argv': args, 'started': time.time()}
        if input_path:
            entry['stdin_sha256'] = vm.digest(input_path)
        self.record.setdefault('operations', []).append(entry)
        self.persist()
        try:
            status, stdout, stderr = vm.command([self.record['backend'], *args], timeout,
                                               output=output, stream=stream, cancelling=cancelling,
                                               environment=self.environment, input_path=input_path)
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
        args = [self.record['backend'], 'run', self.record['machine'], '--no-graphics',
                '--no-audio', '--no-usb-accessories', '--no-clipboard']
        guest = None
        if self.record['configuration']['network'] == 'shared-nat':
            # Tart's default network attachment is shared NAT. No helper needed.
            passed_fds = ()
        else:
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
            args += ['--net-fd', str(guest.fileno())]
            passed_fds = (guest.fileno(),)
        self.record['launch_argv'] = args
        self.persist()
        self.logs = [(self.directory / ('vm-' + suffix)).open('wb') for suffix in ('stdout', 'stderr')]
        env = {key: os.environ[key] for key in ('HOME', 'PATH', 'TMPDIR') if key in os.environ}
        env.update(self.environment)
        self.process = subprocess.Popen(args, stdin=subprocess.DEVNULL, stdout=self.logs[0], stderr=self.logs[1],
                                        env=env, pass_fds=passed_fds, start_new_session=True)
        self.record['launch_pid'] = self.process.pid
        self.persist()
        if guest is not None:
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
    interactive = getattr(args, "interactive", False)
    if interactive:
        from interactive import require_terminal
        require_terminal()
    argv = args.command[1:] if args.command[:1] == ['--'] else args.command
    if not argv:
        raise RuntimeError('supply a guest command after --')
    if not 1 <= args.boot_timeout <= 1800 or not 2 <= args.cpus <= 64 or not 4096 <= args.memory <= 65536:
        raise RuntimeError('require boot timeout 1..1800s, CPUs 2..64 and memory 4096..65536 MiB')
    state = Path(args.state).expanduser().resolve()
    state.mkdir(parents=True, exist_ok=True, mode=0o700)
    directory = state / ('run-' + uuid.uuid4().hex)
    directory.mkdir(mode=0o700)
    report = {'schema': 1, 'vm_type': 'tart', 'argv': argv, 'source': source, 'inputs': {},
              'guest_exit_status': None, 'outcome': 'preparation_failure',
              'interactive': interactive, 'output_capture': 'live_terminal' if interactive else 'separate_streams'}
    machine = None
    print(f'rbdc: run records: {directory}', file=sys.stderr, flush=True)
    with contextlib.ExitStack() as preparation, vm.lock(directory):
        try:
            snapshot_root = None
            if getattr(args, 'workspace_input', None):
                import workspace
                if getattr(args, 'workspace_output', None) and Path(args.workspace_output).expanduser().exists():
                    raise RuntimeError('workspace output must be a new directory')
                if getattr(args, 'workspace_output', None) and Path(args.workspace_output).expanduser().resolve().is_relative_to(Path(args.workspace_input).expanduser().resolve()):
                    raise RuntimeError('workspace output must be outside the original input folder')
                snapshot_root = directory / 'workspace-input'
                snapshot_manifest = workspace.snapshot(Path(args.workspace_input).expanduser(), snapshot_root)
                report['inputs']['workspace'] = {'path': str(snapshot_root), 'manifest': snapshot_manifest}
                if getattr(args, 'workspace_output', None):
                    report['workspace_export'] = {'status': 'not_attempted', 'path': str(Path(args.workspace_output).expanduser().resolve())}
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
            prepared = None
            if getattr(args, 'prepared_base', None):
                prepared = preparation.enter_context(verified_prepared_base(args.prepared_base, home))
                report['inputs']['prepared_base'] = prepared
                image = prepared['name']
            codex = None
            if getattr(args, 'with_codex', False):
                from codex_payload import verify
                codex = verify(args.codex_payload)
                report['inputs']['codex'] = codex['receipt']
            auth_path = None
            if getattr(args, "reuse_host_auth", False):
                from host_auth import source
                auth_path = source(args.codex_auth_home)
            identity = uuid.uuid4().hex
            owner = {'schema': 1, 'vm_type': 'tart', 'kind': 'run', 'id': identity,
                     'machine': 'rbdc-evals-tart-' + identity, 'backend': backend,
                     'backend_sha256': vm.digest(backend), 'backend_version': VERSION,
                     'tart_home': str(home), 'image': image, 'mutation_started': False,
                     'configuration': {'cpus': args.cpus, 'memory_mib': args.memory,
                                       'network': 'shared-nat' if getattr(args, 'network', 'offline') == 'shared' else 'discard-only-socket', 'mounts': 0,
                                       'clipboard': False, 'audio': False, 'usb': False}}
            machine = Machine(directory, owner)
            machine.persist()
            report.update(run_id=identity, machine=owner['machine'], backend_version=VERSION,
                          inputs={'image': prepared['source_image'] if prepared else image,
                                  'fixtures': {}, **report.get('inputs', {})}, resources=owner['configuration'])
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
                    if snapshot_root is not None:
                        import workspace
                        report['outcome'] = 'workspace_transfer_failure'
                        workspace.install(machine, snapshot_root)
                        report['outcome'] = 'transport_failure'
                    if codex:
                        from codex_payload import install
                        report['outcome'] = 'provisioning_failure'
                        report['codex_installation'] = install(machine, codex)
                        report['outcome'] = 'transport_failure'
                    auth_env = ''
                    if auth_path is not None:
                        from host_auth import install as install_auth, GUEST_HOME
                        report['outcome'] = 'authentication_failure'
                        report['authentication'] = install_auth(machine, auth_path)
                        auth_env = 'CODEX_HOME=' + GUEST_HOME + ' '
                        report['outcome'] = 'transport_failure'
                    # A retained guest completion marker distinguishes guest status from transport failure.
                    marker = '/private/tmp/rbdc-' + identity + '.exit'
                    term = ('TERM=' + shlex.quote(os.environ.get('TERM', 'xterm-256color')) + ' ') if interactive else ''
                    working_directory = workspace.GUEST_ROOT if snapshot_root is not None else '/private/tmp'
                    script = ('cd ' + shlex.quote(working_directory) + ' || exit 125; umask 077; '
                              '/usr/bin/env -i PATH=/private/tmp/rbdc-tools/bin:/usr/bin:/bin:/usr/sbin:/sbin:/opt/homebrew/bin '
                              'HOME=/private/tmp LANG=en_US.UTF-8 ' + term + auth_env + '"$@"; status=$?; '
                              'printf "%s\\n" "$status" > ' + marker + '; exit "$status"')
                    command = ['/bin/sh', '-c', script, 'rbdc', *argv]
                    if interactive:
                        from interactive import execute
                        print('rbdc: interactive session; exit the guest command to remove its VM.', file=sys.stderr, flush=True)
                        code = execute(machine, command, args.timeout)
                    else:
                        code, _, _, output = machine.call(['exec', owner['machine'], *command],
                                                           timeout=args.timeout, check=False, stream=True)
                        for suffix in ('stdout', 'stderr'):
                            shutil.copyfile(str(output) + '.' + suffix, directory / ('guest-' + suffix))
                    _, completed, _, _ = machine.call(['exec', owner['machine'], '/bin/cat', marker])
                    guest_status = int(completed.strip())
                    if not 0 <= guest_status <= 255 or guest_status != code:
                        raise RuntimeError('transport exit disagrees with retained guest status')
                    report['guest_exit_status'] = guest_status
                    report['outcome'] = 'success' if code == 0 else 'command_failure'
                    if getattr(args, 'workspace_output', None):
                        command_outcome = report['outcome']
                        report['command_outcome'] = command_outcome
                        report['outcome'] = 'workspace_export_failure'
                        report['workspace_export']['status'] = 'failed'
                        report['workspace_export'] = workspace.export(
                            machine, snapshot_root, Path(args.workspace_output).expanduser().resolve())
                        report['outcome'] = command_outcome
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
            if report.get('workspace_export', {}).get('status') == 'not_attempted':
                report['workspace_export'].update(status='incomplete', reason=report['outcome'])
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
