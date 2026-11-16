"""Owned SmolVM operations; subprocess argv never passes through a host shell."""
import contextlib
import fcntl
import hashlib
import json
import os
from pathlib import Path
import selectors
import shutil
import signal
import subprocess
import sys
import time
import uuid

GIB = 1024 ** 3
interrupted = 0


class Cancelled(Exception):
    pass


def on_signal(number, _frame):
    global interrupted
    interrupted = number


def digest(path):
    value = hashlib.sha256()
    with open(path, 'rb') as handle:
        for block in iter(lambda: handle.read(1024 * 1024), b''):
            value.update(block)
    return value.hexdigest()


def save(path, value):
    """Atomic durable replacement; directory sync precedes external mutation."""
    path = Path(path)
    temporary = path.with_name(path.name + '.pending')
    with temporary.open('w') as handle:
        json.dump(value, handle, indent=2)
        handle.write('\n')
        handle.flush()
        os.fsync(handle.fileno())
        if sys.platform == 'darwin':
            fcntl.fcntl(handle.fileno(), 51)  # F_FULLFSYNC
    os.replace(temporary, path)
    fd = os.open(path.parent, os.O_RDONLY)
    try:
        os.fsync(fd)
    finally:
        os.close(fd)


@contextlib.contextmanager
def lock(directory):
    directory.mkdir(parents=True, exist_ok=True, mode=0o700)
    with (directory / 'lock').open('a') as handle:
        try:
            fcntl.flock(handle, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError as error:
            raise RuntimeError('state is active in another process') from error
        yield


def backend_path(value):
    path = shutil.which(value)
    if not path:
        raise RuntimeError('SmolVM backend missing; enter the root development shell')
    return str(Path(path).resolve())


def command(argv, timeout=90, output=None, stream=False, cancelling=True, cwd=None):
    """Drain byte streams, bound process groups, capture partial output on cancel."""
    env = {key: os.environ[key] for key in ('HOME', 'PATH', 'TMPDIR') if key in os.environ}
    child = subprocess.Popen(argv, env=env, cwd=cwd, stdin=subprocess.DEVNULL,
                             stdout=subprocess.PIPE, stderr=subprocess.PIPE, start_new_session=True)
    readers = selectors.DefaultSelector()
    readers.register(child.stdout, selectors.EVENT_READ, 0)
    readers.register(child.stderr, selectors.EVENT_READ, 1)
    buffers = [bytearray(), bytearray()]
    files = []
    if output:
        files = [Path(str(output) + '.' + suffix).open('wb') for suffix in ('stdout', 'stderr')]
    deadline = time.monotonic() + timeout
    failure = None
    try:
        while readers.get_map() or child.poll() is None:
            if (cancelling and interrupted) or time.monotonic() >= deadline:
                failure = Cancelled('interrupted') if cancelling and interrupted else TimeoutError(f'operation exceeded {timeout}s')
                break
            for key, _ in readers.select(0.1):
                data = os.read(key.fd, 65536)
                if not data:
                    readers.unregister(key.fileobj)
                    key.fileobj.close()
                else:
                    index = key.data
                    # Control output is limited; raw execution streams remain on disk.
                    if len(buffers[index]) < 1024 * 1024:
                        buffers[index].extend(data[:1024 * 1024 - len(buffers[index])])
                    if files:
                        files[index].write(data)
                        files[index].flush()
                    if stream:
                        destination = sys.stdout.buffer if index == 0 else sys.stderr.buffer
                        destination.write(data)
                        destination.flush()
        if failure:
            raise failure
        return child.wait(), bytes(buffers[0]), bytes(buffers[1])
    finally:
        # Reap the entire owned subprocess group, including orphan pipe holders.
        try:
            os.killpg(child.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        child.wait()
        readers.close()
        for pipe in (child.stdout, child.stderr):
            pipe.close()
        for handle in files:
            handle.close()


class Machine:
    def __init__(self, backend, directory, record):
        self.backend = backend_path(backend)
        self.directory = Path(directory)
        self.path = self.directory / 'owner.json'
        self.record = record
        self.sequence = 0

    def persist(self):
        save(self.path, self.record)

    def call(self, args, timeout=90, check=True, stream=False, cancelling=True):
        self.sequence += 1
        output = self.directory / f'backend-{self.sequence:03d}-{args[0]}'
        started = time.time()
        entry = {'argv': args, 'started': started}
        self.record.setdefault('operations', []).append(entry)
        self.persist()
        try:
            code, stdout, stderr = command([self.backend, 'machine', *args], timeout, output, stream, cancelling)
            if args == ['ls', '--json']:
                rows = json.loads(stdout)
                stdout = json.dumps([row for row in rows if row.get('name') == self.record['machine']]).encode()
                Path(str(output) + '.stdout').write_bytes(stdout)
            entry['exit_status'] = code
            if check and code:
                raise RuntimeError(f'SmolVM {args[0]} failed ({code}): {stderr.decode(errors="replace")[-2000:]}')
            return code, stdout, stderr
        except Exception as error:
            entry['error'] = str(error)
            raise
        finally:
            entry['elapsed_seconds'] = time.time() - started
            self.persist()

    def inventory(self, cancelling=True):
        _, data, _ = self.call(['ls', '--json'], 15, cancelling=cancelling)
        matches = [row for row in json.loads(data) if row['name'] == self.record['machine']]
        if len(matches) > 1:
            raise RuntimeError('duplicate owned machine name')
        return matches[0] if matches else None

    def validate(self, row):
        for key, value in self.record['configuration'].items():
            if row.get(key) != value:
                raise RuntimeError(f'owned VM configuration mismatch: {key}; refusing mutation')

    def create(self, image, builder=False):
        if self.inventory() is not None:
            raise RuntimeError('machine name collision; refusing adoption')
        self.record['mutation_started'] = True
        self.persist()
        config = self.record['configuration']
        args = ['create', '--name', self.record['machine'], '--image', str(image),
                '--cpus', str(config['cpus']), '--mem', str(config['memory_mib']),
                '--storage', str(config['storage_gb']), '--overlay', str(config['overlay_gb'])]
        if builder:
            args.append('--net')
        self.call(args, 180)
        self.start()

    def start(self):
        self.call(['start', '--name', self.record['machine']], 60)
        row = self.inventory()
        if row is None:
            raise RuntimeError('created VM missing from inventory')
        self.validate(row)
        self.record['effective_configuration'] = row
        self.persist()

    def execute(self, argv, timeout=300, check=True, stream=False):
        return self.call(['exec', '--name', self.record['machine'], '--stream', '--timeout', f'{timeout}s', '--', *argv],
                         timeout + 15, check, stream)

    def copy(self, source, target):
        return self.call(['cp', str(source), str(target)], 120)

    def cleanup(self, retain=False):
        if not self.record.get('mutation_started'):
            return {'status': 'not_needed'}
        errors = []
        name = self.record['machine']
        try:
            row = self.inventory(cancelling=False)
            if row is not None:
                self.validate(row)
        except RuntimeError as error:
            if 'configuration mismatch' in str(error):
                return {'status': 'failed', 'errors': [str(error)]}
            errors.append(str(error))
        except Exception as error:
            errors.append(str(error))
        # Inventory failure must not prevent the exact owned stop attempt.
        try:
            self.call(['stop', '--name', name], 20, check=False, cancelling=False)
        except Exception as error:
            errors.append(str(error))
        try:
            row = self.inventory(cancelling=False)
            if row is not None:
                self.validate(row)
                if retain:
                    if row.get('state') not in ('created', 'stopped'):
                        raise RuntimeError('builder remains running after stop')
                else:
                    self.call(['delete', '--name', name, '--force'], 20, cancelling=False)
            final = self.inventory(cancelling=False)
            if retain:
                if final is None or final.get('state') not in ('created', 'stopped'):
                    raise RuntimeError('builder stop not verified')
            elif final is not None:
                raise RuntimeError('owned VM remains after deletion')
        except Exception as error:
            errors.append(str(error))
        return {'status': 'failed' if errors else ('stopped' if retain else 'removed'), 'errors': errors}


def new_record(backend, image, kind='run'):
    identity = uuid.uuid4().hex
    return {
        'schema': 1, 'kind': kind, 'id': identity, 'machine': f'rbdc-evals-{kind}-{identity}',
        'backend': backend, 'backend_sha256': digest(backend),
        'configuration': {'image': image, 'network': kind == 'builder', 'mounts': 0, 'ports': 0,
                          'cpus': 4 if kind == 'builder' else 1,
                          'memory_mib': 8192 if kind == 'builder' else 512,
                          'storage_gb': 48 if kind == 'builder' else 4, 'overlay_gb': 2},
        'created_at': time.time(), 'mutation_started': False,
    }


def load_machine(directory):
    directory = Path(directory).resolve()
    record = json.loads((directory / 'owner.json').read_text())
    kind, identity = record.get('kind'), record.get('id', '')
    if (record.get('schema') != 1 or kind not in ('run', 'builder') or len(identity) != 32
            or any(c not in '0123456789abcdef' for c in identity)
            or record.get('machine') != f'rbdc-evals-{kind}-{identity}'):
        raise RuntimeError('invalid ownership record; refusing mutation')
    backend = record['backend']
    if digest(backend) != record['backend_sha256']:
        raise RuntimeError('backend identity changed; refusing recovery')
    machine = Machine(backend, directory, record)
    machine.sequence = len(record.get('operations', []))
    return machine
