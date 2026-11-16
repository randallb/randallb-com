#!/usr/bin/env python3
"""Prepare a credential-free local base whose RPC service does not require GUI login."""
import argparse
import hashlib
import ipaddress
import json
import os
from pathlib import Path
import plistlib
import re
import shutil
import subprocess
import sys
import time
import signal
import threading
import uuid

runner = Path(__file__).resolve().parent / 'runner'
if not runner.is_dir():
    runner = Path(__file__).resolve().parent.parent / 'eval-runner'
sys.path.insert(0, str(runner))
import lifecycle
from tart_backend import DEFAULT_IMAGE

EXPECT = r'''set timeout 25
log_user 0
set ip [lindex $argv 0]
set command [lindex $argv 1]
set known [lindex $argv 2]
spawn /usr/bin/ssh -F /dev/null -o UserKnownHostsFile=$known -o StrictHostKeyChecking=accept-new -o PubkeyAuthentication=no -o PreferredAuthentications=password -o ConnectTimeout=5 admin@$ip $command
expect {
    "*assword:*" {send "admin\r"; exp_continue}
    eof {}
    timeout {exit 124}
}
set result [wait]
puts $expect_out(buffer)
exit [lindex $result 3]
'''


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--tart-home', type=Path, required=True)
    p.add_argument('--output', type=Path, required=True)
    p.add_argument('--name', default='rbdc-base-rpc-v1')
    a = p.parse_args()
    if not re.fullmatch(r'rbdc-base-[a-z0-9-]+', a.name):
        p.error('name must start rbdc-base- and contain lowercase letters, digits or hyphens')
    home, output = a.tart_home.expanduser().resolve(), a.output.expanduser().absolute()
    if output.exists() or (home / 'vms' / a.name).exists():
        raise FileExistsError('base or descriptor already exists; choose a fresh name/output')
    output.parent.mkdir(parents=True, exist_ok=True)
    evidence = output.with_name(output.name + '.preparation')
    evidence.mkdir(mode=0o700)
    script = evidence / 'guest.expect'
    script.write_text(EXPECT)
    tart = shutil.which('tart')
    if not tart or not Path('/usr/bin/expect').is_file():
        raise RuntimeError('Tart and macOS expect are required')
    env = dict(os.environ, TART_HOME=str(home), TART_NO_AUTO_PRUNE='1')
    log = (evidence / 'commands.log').open('a')
    def check_cancelled():
        if lifecycle.interrupted:
            raise lifecycle.Cancelled('base preparation cancelled')
    def call(*args, timeout=60, cancelling=True):
        if cancelling:
            check_cancelled()
        code, stdout, stderr = lifecycle.command([tart, *args], timeout=timeout,
                                                environment=env, cancelling=cancelling)
        log.write(stdout.decode(errors='replace') + stderr.decode(errors='replace')); log.flush()
        if code:
            raise subprocess.CalledProcessError(code, [tart, *args])
    def ssh(ip, command):
        check_cancelled()
        argv = ['/usr/bin/expect', str(script), ip, command, str(evidence / 'known-hosts')]
        code, stdout, stderr = lifecycle.command(argv, timeout=30)
        log.write(stdout.decode(errors='replace') + stderr.decode(errors='replace')); log.flush()
        if code:
            raise subprocess.CalledProcessError(code, argv)
    def start(name):
        check_cancelled()
        return subprocess.Popen([tart, 'run', name, '--no-graphics', '--no-audio',
                                 '--no-usb-accessories', '--no-clipboard'], env=env,
                                stdin=subprocess.DEVNULL, stdout=log, stderr=log, start_new_session=True)
    def ready(name, limit):
        deadline = time.monotonic() + limit
        while time.monotonic() < deadline:
            try:
                call('exec', name, '/usr/bin/true', timeout=5)
                return
            except (subprocess.CalledProcessError, subprocess.TimeoutExpired, TimeoutError):
                time.sleep(.5)
        raise RuntimeError('prepared RPC service did not become ready')
    def stop(name, process):
        try:
            if process.poll() is None:
                try:
                    call('stop', name, '--timeout', '10', timeout=20, cancelling=False)
                    process.wait(timeout=20)
                except (subprocess.SubprocessError, OSError, TimeoutError):
                    try:
                        os.killpg(process.pid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass
                    process.wait(timeout=20)
        finally:
            log.flush()
    proof = 'rbdc-evals-tart-base-proof-' + uuid.uuid4().hex
    with lifecycle.lock(home / 'workload-lock'):
        call('clone', DEFAULT_IMAGE, a.name, timeout=3600)
        process = start(a.name)
        try:
            deadline = time.monotonic() + 120
            while True:
                check_cancelled()
                try:
                    ip = subprocess.check_output([tart, 'ip', a.name, '--resolver', 'dhcp'],
                                                 env=env, text=True, timeout=5).strip()
                    ipaddress.IPv4Address(ip)
                    ssh(ip, '/usr/bin/true')
                    break
                except (ValueError, subprocess.SubprocessError):
                    if time.monotonic() > deadline:
                        raise RuntimeError('clean base SSH preparation did not become ready')
                    time.sleep(1)
            print('Installing standalone RPC service in clean base (no Codex or host auth).', flush=True)
            service = plistlib.dumps({'Label': 'com.randallb.rbdc-rpc',
                'ProgramArguments': ['/opt/homebrew/bin/tart-guest-agent', '--run-rpc'],
                'UserName': 'admin', 'GroupName': 'staff', 'WorkingDirectory': '/Users/admin',
                'RunAtLoad': True, 'KeepAlive': True,
                'StandardOutPath': '/tmp/rbdc-rpc.log', 'StandardErrorPath': '/tmp/rbdc-rpc.log'}).decode()
            import base64
            encoded = base64.b64encode(service.encode()).decode()
            path = '/Library/LaunchDaemons/com.randallb.rbdc-rpc.plist'
            ssh(ip, "printf '%s' '" + encoded + "' | /usr/bin/base64 -D | sudo -n /usr/bin/tee " + path
                + ' >/dev/null; sudo -n /usr/sbin/chown root:wheel ' + path
                + '; sudo -n /bin/chmod 644 ' + path
                + '; sudo -n /bin/launchctl bootstrap system ' + path)
            ready(a.name, 60)
            ssh(ip, '/bin/sync')
        finally:
            stop(a.name, process)
        print('Checking a fresh clone starts RPC without SSH or GUI recovery.', flush=True)
        test = None
        try:
            call('clone', a.name, proof, timeout=120)
            test = start(proof)
            ready(proof, 180)
            call('exec', proof, '/bin/sh', '-c',
                 'set -eu; test ! -e /private/tmp/rbdc-codex-home; test ! -e /private/tmp/rbdc-tools; '
                 'test ! -e /Users/admin/.codex/auth.json')
        finally:
            if test is not None:
                stop(proof, test)
            if (home / 'vms' / proof).exists():
                call('delete', proof, cancelling=False)
        check_cancelled()
        files = {name: lifecycle.digest(home / 'vms' / a.name / name)
                 for name in ('config.json', 'disk.img', 'nvram.bin')}
        descriptor = {'format': 'rbdc-prepared-base-v1', 'name': a.name, 'tart_home': str(home),
                      'source_image': DEFAULT_IMAGE, 'files': files}
        check_cancelled()
        output.write_text(json.dumps(descriptor, indent=2) + '\n')
        (evidence / 'result.json').write_text(json.dumps({
            'prepared': True, 'fresh_clone_rpc_verified': True, 'host_credentials_copied': False,
            'codex_installed': False, 'source_image': DEFAULT_IMAGE, 'descriptor': str(output)}, indent=2) + '\n')
        print(output, flush=True)
    log.close()


def guarded():
    """A pipe watcher makes even a force-killed CLI trigger worker cleanup."""
    if len(sys.argv) > 2 and sys.argv[1] == '--worker-fd':
        fd = int(sys.argv[2])
        del sys.argv[1:3]
        def cancelled(number, _frame):
            lifecycle.on_signal(number, _frame)
        for number in (signal.SIGINT, signal.SIGTERM, signal.SIGHUP):
            signal.signal(number, cancelled)
        def watch():
            with os.fdopen(fd, 'rb') as pipe:
                pipe.read(1)
            lifecycle.on_signal(signal.SIGTERM, None)
        threading.Thread(target=watch, daemon=True).start()
        try:
            main()
        except BaseException as error:
            # No success descriptor is written before base stop and proof deletion.
            if '--output' in sys.argv:
                path = Path(sys.argv[sys.argv.index('--output') + 1]).expanduser().absolute()
                folder = path.with_name(path.name + '.preparation')
                if folder.is_dir():
                    (folder / 'result.json').write_text(json.dumps({'prepared': False, 'error': str(error)}, indent=2) + '\n')
            raise
        return
    read_fd, write_fd = os.pipe()
    child = subprocess.Popen([sys.executable, str(Path(__file__).resolve()), '--worker-fd', str(read_fd), *sys.argv[1:]],
                             pass_fds=(read_fd,), start_new_session=True)
    os.close(read_fd)
    alive = True
    def stop_worker(_number, _frame):
        nonlocal alive
        if alive:
            alive = False
            os.close(write_fd)
    for number in (signal.SIGINT, signal.SIGTERM, signal.SIGHUP):
        signal.signal(number, stop_worker)
    try:
        code = child.wait()
    finally:
        stop_worker(None, None)
    raise SystemExit(code)


if __name__ == '__main__':
    guarded()
