#!/usr/bin/env python3
"""rbdc task dispatch and repeatable synthetic VM runs."""
import argparse
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys

import lifecycle as vm


def source_record():
    try:
        root = subprocess.check_output(['git', 'rev-parse', '--show-toplevel'], stderr=subprocess.DEVNULL).decode().strip()
        revision = subprocess.check_output(['git', '-C', root, 'rev-parse', 'HEAD']).decode().strip()
        dirty = subprocess.check_output(['git', '-C', root, 'status', '--porcelain=v1']).decode()
        return {'root': root, 'revision': revision, 'dirty': bool(dirty), 'status': dirty}
    except (subprocess.CalledProcessError, FileNotFoundError):
        return {'revision': None, 'dirty': None}


def run(args):
    if getattr(args, "interactive", False) and getattr(args, "vm_type", "smolvm") != "tart":
        raise RuntimeError("--interactive currently supports macOS Tart guests only")
    if getattr(args, "with_codex", False) and getattr(args, "vm_type", "smolvm") != "tart":
        raise RuntimeError("--with-codex currently supports macOS Tart guests only")
    if getattr(args, "vm_type", "smolvm") == "tart":
        from tart_backend import run as run_tart
        return run_tart(args, source_record())
    argv = args.command
    if argv and argv[0] == '--':
        argv = argv[1:]
    if not argv:
        raise RuntimeError('supply a command after --; example: rbdc evals run -- sh -c "echo hello"')
    state = Path(args.state).expanduser().resolve()
    state.mkdir(parents=True, exist_ok=True, mode=0o700)
    directory = state / ('run-' + vm.uuid.uuid4().hex)
    directory.mkdir(mode=0o700)
    report = {'schema': 1, 'argv': argv, 'source': source_record(), 'inputs': {},
              'guest_exit_status': None, 'outcome': 'preparation_failure'}
    machine = None
    print(f'rbdc: run records: {directory}', file=sys.stderr, flush=True)
    with vm.lock(directory):
        try:
            backend = vm.backend_path(args.backend)
            code, version, _ = vm.command([backend, '--version'], 10)
            if code or version.strip() != b'smolvm 1.32.2':
                raise RuntimeError(f'unsupported backend: {version.decode(errors="replace").strip()}; require smolvm 1.32.2')
            report['backend_version'] = version.decode().strip()
            if not args.image:
                raise RuntimeError('image missing; build with rbdc evals build-image, then set RBDC_EVALS_IMAGE or --image')
            image = Path(args.image).expanduser().resolve()
            if not image.is_file():
                raise RuntimeError('image missing or invalid; run rbdc evals build-image or supply --image')
            if not image.is_file() or image.stat().st_size == 0:
                raise RuntimeError('invalid image: require a nonempty Linux image archive')
            if shutil.disk_usage(directory).free < 8 * vm.GIB:
                raise RuntimeError('at least 8 GiB free disk required before workload creation')
            image_hash = vm.digest(image)
            report['inputs'] = {'image': str(image), 'image_sha256': image_hash, 'fixtures': {}}
            owner = vm.new_record(backend, 'local:' + image_hash)
            owner['backend_version'] = report['backend_version']
            machine = vm.Machine(backend, directory, owner)
            machine.persist()
            report['run_id'] = owner['id']
            report['machine'] = owner['machine']
            report['resources'] = owner['configuration']
            machine.create(image)
            report['outcome'] = 'transport_failure'
            backend_status, _, _ = machine.execute(
                ['/bin/env', '-i', 'PATH=/bin', 'HOME=/tmp', '/bin/python3', '/share/rbdc/guest.py', owner['id'], *argv],
                args.timeout, check=False, stream=True)
            report['backend_exec_status'] = backend_status
            guest_dir = f'{owner["machine"]}:/tmp/rbdc-{owner["id"]}'
            try:
                machine.copy(f'{guest_dir}/result.json', directory / 'guest-result.json')
            except RuntimeError as error:
                # SmolVM's timeout and ordinary guest exit 124 share a code.
                # A completed guest result distinguishes the latter.
                if backend_status == 124:
                    report['timeout_seconds'] = args.timeout
                    raise TimeoutError(f'guest command exceeded {args.timeout}s; partial output retained') from error
                raise
            for filename in ('stdout', 'stderr'):
                machine.copy(f'{guest_dir}/{filename}', directory / ('guest-' + filename))
            result = json.loads((directory / 'guest-result.json').read_text())
            if result.get('token') != owner['id'] or result.get('argv') != argv or result.get('system', [''])[0] != 'Linux':
                raise RuntimeError('guest result identity mismatch')
            status = result['exit_status']
            if not isinstance(status, int) or isinstance(status, bool) or not -255 <= status <= 255:
                raise RuntimeError('invalid guest exit status')
            expected = status if status >= 0 else 128 - status
            if backend_status != expected:
                raise RuntimeError('transport exit disagrees with retained guest status')
            report['guest_exit_status'] = status
            report['guest'] = result
            report['outcome'] = 'invalid_executable' if result.get('kind') == 'invalid_executable' else ('success' if status == 0 else 'command_failure')
        except TimeoutError as error:
            report.update(outcome='timeout', error=str(error))
        except vm.Cancelled:
            report['outcome'] = 'cancelled'
            report['signal'] = vm.interrupted
        except Exception as error:
            report['error'] = str(error)
            print(f'rbdc: {error}', file=sys.stderr)
        finally:
            try:
                report['cleanup'] = machine.cleanup() if machine else {'status': 'not_needed'}
            except Exception as error:
                report['cleanup'] = {'status': 'failed', 'errors': [str(error)]}
            report['execution_outcome'] = report['outcome']
            if report['cleanup']['status'] == 'failed':
                report['outcome'] = 'cleanup_failure'
            vm.save(directory / 'result.json', report)
            print(f'rbdc: {report["outcome"]}; guest={report["guest_exit_status"]}; cleanup={report["cleanup"]["status"]}', file=sys.stderr)
    if report['outcome'] == 'success':
        return 0
    if report['outcome'] == 'command_failure':
        status = report['guest_exit_status']
        return status if status > 0 else 128 - status
    if report['outcome'] == 'invalid_executable':
        return 127
    if report['outcome'] == 'cancelled':
        return 128 + report['signal']
    return 1


def recover(args):
    directory = Path(args.record).expanduser().resolve()
    with vm.lock(directory):
        owner = json.loads((directory / 'owner.json').read_text())
        if owner.get('vm_type') == 'tart':
            from tart_backend import recover
            return recover(directory)
        machine = vm.load_machine(directory)
        result = machine.cleanup(retain=machine.record['kind'] == 'builder' and not args.delete_builder)
        vm.save(directory / 'recovery.json', result)
        print(json.dumps(result))
        return int(result['status'] == 'failed')


def main():
    parser = argparse.ArgumentParser(prog='rbdc', description='randallb.com repository tasks')
    tasks = parser.add_subparsers(dest='task', required=True)
    evals = tasks.add_parser('evals', help='fresh Linux or macOS VM execution')
    commands = evals.add_subparsers(dest='action', required=True)
    runner = commands.add_parser('run', help='execute argv in a fresh owned VM')
    runner.add_argument('--vm-type', choices=['smolvm', 'tart'], default='smolvm', help='VM implementation; --backend remains the SmolVM executable')
    runner.add_argument('-it', '--interactive', action='store_true', help='attach terminal input/output to a macOS guest PTY')
    runner.add_argument('--with-codex', action='store_true', help='install pinned Codex CLI in the fresh macOS guest; no authentication')
    runner.add_argument('--codex-payload', default=os.environ.get('RBDC_CODEX_PAYLOAD'), help='verified Nix-built Codex payload directory')
    runner.add_argument('--tart', default=os.environ.get('RBDC_TART', 'tart'), help='Tart executable (requires 2.41.1)')
    runner.add_argument('--tart-home', default=os.environ.get('RBDC_TART_HOME', '.rbdc-evals/tart'), help='dedicated Tart state/cache directory')
    runner.add_argument('--macos-image', help='digest-pinned OCI macOS image; uses the documented default when omitted')
    runner.add_argument('--boot-timeout', type=int, default=180)
    runner.add_argument('--cpus', type=int, default=4)
    runner.add_argument('--memory', type=int, default=8192, help='macOS guest RAM in MiB')
    runner.add_argument('--image', default=os.environ.get('RBDC_EVALS_IMAGE', '.rbdc-evals/builder/image.tar.gz'))
    runner.add_argument('--backend', default=os.environ.get('RBDC_SMOLVM', 'smolvm'))
    runner.add_argument('--state', default=os.environ.get('RBDC_EVALS_STATE', '.rbdc-evals'))
    runner.add_argument('--timeout', type=int, default=None, help='guest deadline in seconds (1..86400; default 300, or 3600 interactive)')
    runner.add_argument('command', nargs=argparse.REMAINDER)
    runner.set_defaults(function=run)
    recovery = commands.add_parser('recover', help='clean the exact VM in an inactive run/builder record')
    recovery.add_argument('record', help='directory containing owner.json')
    recovery.add_argument('--delete-builder', action='store_true', help='delete the owned builder instead of retaining stopped state')
    recovery.set_defaults(function=recover)
    builder = commands.add_parser('build-image', help='build root Linux image in a separately owned local builder')
    builder.add_argument('--root', type=Path, default=Path.cwd())
    builder.add_argument('--state', type=Path, default=Path('.rbdc-evals/builder'))
    builder.add_argument('--backend', default=os.environ.get('RBDC_SMOLVM', 'smolvm'))
    builder.set_defaults(function=build_image)
    args = parser.parse_args()
    if hasattr(args, 'timeout') and args.timeout is None:
        args.timeout = 3600 if args.interactive else 300
    if hasattr(args, 'timeout') and not 1 <= args.timeout <= 86400:
        parser.error('--timeout must be between 1 and 86400 seconds')
    signal.signal(signal.SIGINT, vm.on_signal)
    signal.signal(signal.SIGTERM, vm.on_signal)
    signal.signal(signal.SIGHUP, vm.on_signal)
    try:
        return args.function(args)
    except Exception as error:
        print(f'rbdc: {error}', file=sys.stderr)
        return 1


def build_image(args):
    from builder import build
    return build(args)


if __name__ == '__main__':
    sys.exit(main())
