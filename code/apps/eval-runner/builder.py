"""Separately owned, retained Linux Nix builder; no company services required."""
import hashlib
import json
from pathlib import Path
import shutil
import tarfile
import urllib.request

import lifecycle as vm

DEBIAN = 'docker.io/library/debian@sha256:0c8bbb8e987a035fe1d9704eb2e571b7e9a836e1caa46345290674b45b69e417'
NIX_URL = 'https://releases.nixos.org/nix/nix-2.34.1/nix-2.34.1-aarch64-linux.tar.xz'
NIX_SHA256 = '81aed0600605ee0494d2cd82c441b1ff6ecad7d3efcc6d4505a70aefc67b865c'
SOURCE_PATHS = ['flake.nix', 'flake.lock', 'code/rust-toolchain.toml'] + [
    'code/apps/eval-runner/' + name for name in
    ('default.nix', 'image.nix', 'guest.py', 'cli.py', 'lifecycle.py', 'builder.py', 'bootstrap-builder.sh')]


def package(root, state):
    hashes = {}
    with tarfile.open(state / 'source.tar', 'w') as archive:
        for name in SOURCE_PATHS:
            path = root / name
            if path.is_symlink() or not path.is_file():
                raise RuntimeError(f'invalid declared source input: {name}')
            hashes[name] = vm.digest(path)
            archive.add(path, arcname=name, recursive=False)
    vm.save(state / 'source-manifest.json', hashes)
    return hashes


def bootstrap(state):
    archive = state / 'nix.tar.xz'
    if not archive.exists():
        partial = state / 'nix-download.partial'
        with urllib.request.urlopen(NIX_URL, timeout=120) as response, partial.open('wb') as output:
            shutil.copyfileobj(response, output)
        if vm.digest(partial) != NIX_SHA256:
            raise RuntimeError('Nix download checksum mismatch')
        partial.replace(archive)
    if vm.digest(archive) != NIX_SHA256:
        raise RuntimeError('Nix bootstrap archive checksum mismatch')
    extracted = state / 'bootstrap-source'
    if not (state / 'bootstrap-extracted').exists():
        extracted.mkdir(exist_ok=True)
        with tarfile.open(archive) as source:
            # Official checksum-verified Nix archive contains absolute /nix/store links.
            source.extractall(extracted, filter='fully_trusted')
        (state / 'bootstrap-extracted').touch()
    directory, = extracted.iterdir()
    nix, = (directory / 'store').glob('*-nix-2.34.1')
    ca, = (directory / 'store').glob('*-nss-cacert-*/etc/ssl/certs/ca-bundle.crt')
    with tarfile.open(state / 'bootstrap.tar', 'w') as target:
        target.add(directory / 'store', arcname='store')
        target.add(directory / '.reginfo', arcname='reginfo')
    return '/nix/store/' + nix.name, '/nix/store/' + ca.relative_to(directory / 'store').as_posix()


def build(args):
    root, state = args.root.resolve(), args.state.resolve()
    state.mkdir(parents=True, exist_ok=True, mode=0o700)
    report = {'schema': 1, 'outcome': 'preparation_failure', 'bootstrap_url': NIX_URL, 'bootstrap_sha256': NIX_SHA256}
    machine = None
    with vm.lock(state):
        try:
            if shutil.disk_usage(state).free < 60 * vm.GIB:
                raise RuntimeError('builder needs at least 60 GiB free disk')
            backend = vm.backend_path(args.backend)
            code, version, _ = vm.command([backend, '--version'], 10)
            if code or version.strip() != b'smolvm 1.32.2':
                raise RuntimeError('builder requires pinned smolvm 1.32.2')
            report['backend_version'] = version.decode().strip()
            report['source'] = package(root, state)
            nix, ca = bootstrap(state)
            if (state / 'owner.json').exists():
                machine = vm.load_machine(state)
                if machine.record['kind'] != 'builder' or machine.backend != backend:
                    raise RuntimeError('builder ownership/backend mismatch')
                row = machine.inventory()
                if row is None:
                    raise RuntimeError('retained builder missing; choose a fresh state directory')
                machine.validate(row)
                machine.start()
            else:
                machine = vm.Machine(backend, state, vm.new_record(backend, DEBIAN, 'builder'))
                machine.persist()
                machine.create(DEBIAN, builder=True)
            def execute(argv, timeout=600):
                return machine.execute(argv, timeout, stream=True)
            _, capacity, _ = machine.execute(['/bin/sh', '-eu', '-c', "cat /sys/class/block/vda/size; stat -f -c '%S %b' / /workspace"])
            lines = capacity.decode().splitlines()
            if int(lines[0]) * 512 < 48 * vm.GIB or any(int(a) * int(b) < 45 * vm.GIB for a, b in (line.split() for line in lines[1:])):
                raise RuntimeError('actual builder disk/filesystem smaller than requested')
            report['capacity'] = lines
            name = machine.record['machine']
            machine.copy(state / 'bootstrap.tar', f'{name}:/workspace/bootstrap.tar')
            machine.copy(root / 'code/apps/eval-runner/bootstrap-builder.sh', f'{name}:/workspace/bootstrap-builder.sh')
            execute(['/bin/sh', '/workspace/bootstrap-builder.sh', nix, ca])
            machine.copy(state / 'source.tar', f'{name}:/workspace/source.tar')
            # This fixed path is private to this owned builder; never a host path.
            execute(['/bin/sh', '-eu', '-c', 'rm -rf /opt/rbdc-source; mkdir -p /opt/rbdc-source; tar -xf /workspace/source.tar -C /opt/rbdc-source'])
            _, lock_hash, _ = machine.execute(['/usr/bin/sha256sum', '/opt/rbdc-source/flake.lock'])
            if lock_hash.decode().split()[0] != report['source']['flake.lock']:
                raise RuntimeError('guest source lock mismatch')
            execute(['/opt/nix/bin/nix', 'build', '--store', 'local', '--no-update-lock-file', '--no-write-lock-file',
                     '--out-link', '/workspace/result', '-L', 'path:/opt/rbdc-source#packages.aarch64-linux.evals-image'], 5400)
            execute(['/bin/sh', '-eu', '-c', 'cp -L /workspace/result /workspace/image.tar.gz; sha256sum /workspace/image.tar.gz'])
            _, image_hash, _ = machine.execute(['/usr/bin/sha256sum', '/workspace/image.tar.gz'])
            _, size, _ = machine.execute(['/usr/bin/stat', '-c', '%s', '/workspace/image.tar.gz'])
            if not 0 < int(size) < 4 * vm.GIB:
                raise RuntimeError('image exceeds supported 4GiB transfer size')
            machine.copy(f'{name}:/workspace/image.tar.gz', state / 'image.tar.gz')
            image = state / 'image.tar.gz'
            if vm.digest(image) != image_hash.decode().split()[0] or image.stat().st_size != int(size):
                raise RuntimeError('retrieved image size/checksum mismatch')
            if vm.digest(root / 'flake.lock') != report['source']['flake.lock']:
                raise RuntimeError('root lock changed during image build')
            report.update(outcome='success', image=str(image), image_sha256=vm.digest(image))
            print(f'RBDC_EVALS_IMAGE={image}')
        except vm.Cancelled:
            report['outcome'] = 'cancelled'
        except Exception as error:
            report['error'] = str(error)
            print(f'rbdc: image build: {error}')
        finally:
            try:
                report['cleanup'] = machine.cleanup(retain=True) if machine else {'status': 'not_needed'}
            except Exception as error:
                report['cleanup'] = {'status': 'failed', 'errors': [str(error)]}
            report['execution_outcome'] = report['outcome']
            if report['cleanup']['status'] == 'failed':
                report['outcome'] = 'cleanup_failure'
            vm.save(state / 'build-result.json', report)
    return int(report['outcome'] != 'success')
