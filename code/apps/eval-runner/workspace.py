"""Bounded snapshots and guest workspace transfer; never execute exported code on host."""
import difflib
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import shutil
import stat
import tarfile

GUEST_ROOT = '/private/tmp/rbdc-workspace'
MAX_BYTES = 64 * 1024 * 1024
MAX_FILES = 4096
MAX_ARCHIVE = MAX_BYTES + 8 * 1024 * 1024
MAX_DIFF_BYTES = 256 * 1024
MAX_DIFF_LINES = 4000
MAX_PATCH_BYTES = 1024 * 1024
MAX_DIFF_TOTAL_BYTES = 4 * 1024 * 1024
MAX_DIFF_TOTAL_LINES = 16000


def name(value):
    """Accept portable relative member names, stripping tar's leading ./ only."""
    if value.startswith('./'):
        value = value[2:]
    parts = value.split('/')
    if not value or any(p in ('', '.', '..') for p in parts) or value.startswith('/') or '\\' in value or any(ord(c) < 32 or ord(c) == 127 for c in value):
        raise ValueError('unsafe workspace path')
    return str(PurePosixPath(value))


def manifest(folder):
    folder = Path(folder)
    if folder.is_symlink() or not folder.is_dir():
        raise ValueError('workspace must be an ordinary directory')
    files, directories, total = {}, [], 0
    entries = []
    for path in folder.rglob('*'):
        entries.append(path)
        if len(entries) > MAX_FILES:
            raise ValueError('workspace exceeds 4096 entries')
    entries.sort()
    for path in entries:
        relative = name(path.relative_to(folder).as_posix())
        info = path.lstat()
        if stat.S_ISDIR(info.st_mode):
            directories.append(relative)
        elif stat.S_ISREG(info.st_mode) and info.st_nlink == 1:
            total += info.st_size
            if total > MAX_BYTES:
                raise ValueError('workspace exceeds 64 MiB')
            with os.fdopen(os.open(path, os.O_RDONLY | os.O_NOFOLLOW), 'rb') as handle:
                data = handle.read(info.st_size + 1)
                if len(data) != info.st_size:
                    raise ValueError('workspace changed while hashing')
                checksum = hashlib.sha256(data).hexdigest()
            files[relative] = {'sha256': checksum, 'size': info.st_size,
                               'executable': bool(info.st_mode & 0o111)}
        else:
            raise ValueError('workspace contains a link or special file: ' + relative)
    content = {'files': files, 'directories': directories, 'bytes': total}
    content['tree_sha256'] = hashlib.sha256(json.dumps(content, sort_keys=True, separators=(',', ':')).encode()).hexdigest()
    return content


def validate_manifest(folder, expected):
    observed = manifest(folder)
    if observed != expected:
        raise ValueError('workspace differs from its saved manifest')
    return observed


def snapshot(folder, target):
    """Copy a folder into target/workspace, hash it, and archive the frozen copy."""
    folder, target = Path(folder), Path(target)
    if folder.is_symlink() or target.resolve().is_relative_to(folder.resolve()):
        raise ValueError('snapshot must be outside its ordinary source directory')
    original = manifest(folder)
    target.mkdir(parents=True, exist_ok=False, mode=0o700)
    frozen = target / 'workspace'
    frozen.mkdir(mode=0o700)
    for relative in original['directories']:
        (frozen / relative).mkdir(parents=True, exist_ok=True, mode=0o700)
    for relative, info in original['files'].items():
        path = frozen / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        with os.fdopen(os.open(folder / relative, os.O_RDONLY | os.O_NOFOLLOW), 'rb') as source, path.open('xb') as destination:
            remaining = info['size']
            while remaining:
                block = source.read(min(remaining, 1024 * 1024))
                if not block:
                    raise ValueError('workspace changed during snapshot')
                destination.write(block)
                remaining -= len(block)
            if source.read(1):
                raise ValueError('workspace changed during snapshot')
        path.chmod(0o700 if info['executable'] else 0o600)
    validate_manifest(frozen, original)
    (target / 'manifest.json').write_text(json.dumps(original, indent=2) + '\n')
    with tarfile.open(target / 'archive.tar', 'w', format=tarfile.PAX_FORMAT) as archive:
        for relative in original['directories']:
            archive.add(frozen / relative, arcname=relative, recursive=False)
        for relative in original['files']:
            archive.add(frozen / relative, arcname=relative, recursive=False)
    if (target / 'archive.tar').stat().st_size > MAX_ARCHIVE:
        raise ValueError('workspace archive exceeds transfer limit')
    return original


def install(machine, snapshot_root):
    archive = Path(snapshot_root) / 'archive.tar'
    script = ('set -eu; umask 077; test ! -e "$1"; /bin/mkdir "$1"; '
              '/usr/bin/tar -xf - -C "$1"')
    machine.call(['exec', '-i', machine.record['machine'], '/bin/sh', '-c', script,
                  'rbdc-workspace', GUEST_ROOT], timeout=120, input_path=archive)


def extract(archive_path, destination):
    """Extract only bounded regular files and directories into a fresh host folder."""
    archive_path, destination = Path(archive_path), Path(destination)
    if archive_path.stat().st_size > MAX_ARCHIVE:
        raise ValueError('workspace archive exceeds transfer limit')
    destination.mkdir(parents=True, exist_ok=False, mode=0o700)
    seen, total, count = set(), 0, 0
    with tarfile.open(archive_path, 'r:') as archive:
        for member in archive:
            count += 1
            if count > MAX_FILES + 1:
                raise ValueError('workspace archive has too many entries')
            if member.name in ('.', './') and member.isdir():
                continue
            relative = name(member.name.rstrip('/') if member.isdir() else member.name)
            if len(seen) >= MAX_FILES or relative in seen:
                raise ValueError('workspace archive has too many or duplicate entries')
            seen.add(relative)
            if not member.isdir() and not member.isreg():
                raise ValueError('workspace archive contains a link or special file')
            total += member.size
            if member.size < 0 or total > MAX_BYTES:
                raise ValueError('workspace archive exceeds 64 MiB')
            path = destination / relative
            if member.isdir():
                path.mkdir(parents=True, exist_ok=True, mode=0o700)
            else:
                path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
                with archive.extractfile(member) as source, path.open('xb') as target:
                    shutil.copyfileobj(source, target, 1024 * 1024)
                path.chmod(0o700 if member.mode & 0o111 else 0o600)
    return manifest(destination)


def diff(before_root, after_root, output):
    # lifecycle does not import workspace. Check its existing cancellation flag
    # between files so host-side inspection never delays normal VM cleanup.
    import lifecycle as vm
    if vm.interrupted:
        raise vm.Cancelled()
    before_root, after_root, output = Path(before_root), Path(after_root), Path(output)
    before, after = manifest(before_root), manifest(after_root)
    changes, patch = [], []
    patch_bytes, text_bytes, text_lines = 0, 0, 0
    omitted = False
    def append(text):
        nonlocal patch_bytes, omitted
        encoded_size = len(text.encode())
        if patch_bytes + encoded_size <= MAX_PATCH_BYTES - 256:
            patch.append(text)
            patch_bytes += encoded_size
        else:
            omitted = True
    for relative in sorted(before['files'].keys() | after['files'].keys()):
        if vm.interrupted:
            raise vm.Cancelled()
        old, new = before['files'].get(relative), after['files'].get(relative)
        if old == new:
            continue
        status = 'added' if old is None else 'deleted' if new is None else 'modified'
        changes.append({'path': relative, 'status': status, 'before': old, 'after': new})
        try:
            if (omitted or text_bytes >= MAX_DIFF_TOTAL_BYTES or text_lines >= MAX_DIFF_TOTAL_LINES
                    or any(info and info['size'] > MAX_DIFF_BYTES for info in (old, new))):
                raise OverflowError()
            data = []
            for root, info in ((before_root, old), (after_root, new)):
                if info:
                    with (root / relative).open('rb') as source:
                        part = source.read(MAX_DIFF_BYTES + 1)
                    if len(part) > MAX_DIFF_BYTES:
                        raise OverflowError()
                else:
                    part = b''
                data.append(part)
            a, b = (part.decode().splitlines(keepends=True) for part in data)
            if max(len(a), len(b)) > MAX_DIFF_LINES:
                raise OverflowError()
            if any('\x00' in line for line in a + b):
                raise UnicodeError()
            text_bytes += sum(len(part) for part in data)
            text_lines += len(a) + len(b)
            candidate = []
            for line in difflib.unified_diff(a, b, fromfile='before/' + relative, tofile='after/' + relative):
                candidate.append(line if line.endswith('\n') else line + '\n\\ No newline at end of file\n')
            if old and new and old['executable'] != new['executable']:
                candidate.append(f'Executable mode changed: {relative}\n')
            if patch_bytes + len(''.join(candidate).encode()) > MAX_PATCH_BYTES - 256:
                raise OverflowError()
            append(''.join(candidate))
        except UnicodeError:
            append(f'Binary file {status}: {relative}; full file and hashes retained.\n')
        except OverflowError:
            append(f'Text diff omitted by inspection limits: {relative} ({status}); full file and hashes retained.\n')
    if omitted:
        patch.append('Further diff summaries omitted by patch-size limit; inspect changes.json and the full workspace.\n')
    result = {'files': changes, 'directories_added': sorted(set(after['directories']) - set(before['directories'])),
              'directories_deleted': sorted(set(before['directories']) - set(after['directories']))}
    (output / 'changes.json').write_text(json.dumps(result, indent=2) + '\n')
    (output / 'diff.patch').write_text(''.join(patch))
    return result


def export(machine, snapshot_root, output):
    output = Path(output)
    output.mkdir(parents=True, exist_ok=False, mode=0o700)
    # Limit the temporary tar file in the guest, then bound its actual byte count
    # before allowing any archive bytes onto the host transport stream.
    script = ('set -eu; umask 077; ulimit -f 147456; '
              '/usr/bin/tar -cf "$2" -C "$1" .; '
              'test "$(/usr/bin/stat -f %z "$2")" -le ' + str(MAX_ARCHIVE) + '; '
              '/bin/cat "$2"; /bin/rm "$2"')
    _, _, _, stream = machine.call(['exec', machine.record['machine'], '/bin/sh', '-c', script,
                                    'rbdc-export', GUEST_ROOT, '/private/tmp/rbdc-workspace-export.tar'],
                                   timeout=120)
    archive = output / 'workspace.tar'
    shutil.copyfile(str(stream) + '.stdout', archive)
    result = extract(archive, output / 'workspace')
    (output / 'manifest.json').write_text(json.dumps(result, indent=2) + '\n')
    diff(Path(snapshot_root) / 'workspace', output / 'workspace', output)
    return {'status': 'complete', 'path': str(output.resolve()), 'manifest': result}
