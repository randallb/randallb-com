"""Pinned standalone Codex provisioning over the guest-agent stdin channel."""
import hashlib
import json
from pathlib import Path
import tarfile

import lifecycle as vm

PIN = json.loads(Path(__file__).with_name('codex-pin.json').read_text())
GUEST_BIN = '/private/tmp/rbdc-tools/bin/codex'


def verify(directory):
    if not directory:
        raise RuntimeError('Codex payload missing; enter the root Nix shell, or build .#evals-codex-payload and pass --codex-payload')
    directory = Path(directory).expanduser().resolve()
    manifest = json.loads((directory / 'manifest.json').read_text())
    if manifest != PIN:
        raise RuntimeError('Codex payload manifest differs from the repository pin')
    archive = directory / 'codex.tar.gz'
    if vm.digest(archive) != PIN['sha256']:
        raise RuntimeError('Codex archive checksum mismatch')
    with tarfile.open(archive, 'r:gz') as source:
        members = source.getmembers()
        if len(members) != 1 or members[0].name != PIN['archive_member'] or not members[0].isfile():
            raise RuntimeError('unexpected Codex archive layout; require one regular executable')
        with source.extractfile(members[0]) as binary:
            checksum = hashlib.file_digest(binary, 'sha256').hexdigest()
    return {'archive': archive, 'receipt': {**PIN, 'binary_sha256': checksum}}


def install(machine, payload):
    receipt = payload['receipt']
    archive = '/private/tmp/rbdc-' + machine.record['id'] + '-codex.tar.gz'
    # Fixed paths are absent in the pinned starting image. Do not silently reuse an installation.
    machine.call(['exec', machine.record['machine'], '/bin/sh', '-c',
                  'test ! -e /private/tmp/rbdc-tools && test ! -e "$1"', 'rbdc', archive])
    machine.call(['exec', '-i', machine.record['machine'], '/bin/sh', '-c',
                  'umask 077; /bin/cat > "$1"', 'rbdc', archive], timeout=120,
                 input_path=payload['archive'])
    _, checksum, _, _ = machine.call(['exec', machine.record['machine'], '/usr/bin/shasum', '-a', '256', archive])
    if checksum.decode().split()[0] != receipt['sha256']:
        raise RuntimeError('guest Codex archive checksum mismatch')
    script = ('set -eu; umask 077; /bin/mkdir -p /private/tmp/rbdc-tools/bin; '
              '/usr/bin/tar -xzf "$1" -C /private/tmp/rbdc-tools; '
              '/bin/mv "/private/tmp/rbdc-tools/$2" /private/tmp/rbdc-tools/bin/codex; '
              '/bin/chmod 755 /private/tmp/rbdc-tools/bin/codex; /bin/rm "$1"')
    machine.call(['exec', machine.record['machine'], '/bin/sh', '-c', script,
                  'rbdc', archive, receipt['archive_member']])
    _, checksum, _, _ = machine.call(['exec', machine.record['machine'], '/usr/bin/shasum', '-a', '256', GUEST_BIN])
    if checksum.decode().split()[0] != receipt['binary_sha256']:
        raise RuntimeError('installed Codex executable checksum mismatch')
    _, version, _, _ = machine.call(['exec', machine.record['machine'], '/usr/bin/env', '-i',
                                    'HOME=/private/tmp', 'PATH=/usr/bin:/bin', GUEST_BIN, '--version'])
    if version.decode().strip() != receipt['expected_version']:
        raise RuntimeError('installed Codex version differs from the repository pin')
    result = {**receipt, 'guest_binary': GUEST_BIN, 'observed_version': version.decode().strip(),
              'transfer': 'guest-agent-stdin', 'network': 'discard-only-socket',
              'host_mounts': 0, 'authentication': 'not_provisioned'}
    vm.save(machine.directory / 'codex-installation.json', result)
    return result
