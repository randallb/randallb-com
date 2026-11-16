"""Attach a terminal to Tart's guest PTY, restoring it before VM cleanup."""
import os
import signal
import subprocess
import sys
import termios
import time

import lifecycle as vm


def require_terminal():
    if not sys.stdin.isatty() or not sys.stdout.isatty():
        raise RuntimeError('--interactive requires terminal stdin and stdout; run it from Terminal')
    try:
        foreground = os.tcgetpgrp(sys.stdin.fileno())
    except OSError as error:
        raise RuntimeError('--interactive requires a controlling terminal; run it from Terminal') from error
    if foreground != os.getpgrp():
        raise RuntimeError('--interactive must run in the foreground terminal process group')


def execute(machine, argv, timeout):
    require_terminal()
    terminal = sys.stdin.fileno()
    original = termios.tcgetattr(terminal)
    previous_resize = signal.getsignal(signal.SIGWINCH)
    entry = {'argv': ['exec', '-i', '-t', machine.record['machine'], *argv],
             'started': time.time(), 'output_capture': 'live_terminal'}
    machine.record.setdefault('operations', []).append(entry)
    machine.persist()
    env = {key: os.environ[key] for key in ('HOME', 'PATH', 'TMPDIR', 'TERM') if key in os.environ}
    env.update(machine.environment)
    child = None
    failed = False
    try:
        # Tart handles raw input, guest PTY setup and window dimensions. Keep its
        # process group independent so timeout/cancellation cannot signal the host shell.
        child = subprocess.Popen([machine.record['backend'], *entry['argv']],
                                 env=env, start_new_session=True)

        def resize(_number, _frame):
            try:
                os.killpg(child.pid, signal.SIGWINCH)
            except ProcessLookupError:
                pass

        signal.signal(signal.SIGWINCH, resize)
        deadline = time.monotonic() + timeout
        while child.poll() is None:
            if vm.interrupted:
                raise vm.Cancelled()
            if time.monotonic() >= deadline:
                raise TimeoutError(f'interactive command exceeded {timeout}s')
            time.sleep(.05)
        entry['exit_status'] = child.returncode
        return child.returncode
    except Exception as error:
        failed = True
        entry['error'] = str(error) or type(error).__name__
        raise
    finally:
        try:
            if child:
                # Also reap descendants holding the terminal after the client exits.
                try:
                    os.killpg(child.pid, signal.SIGTERM)
                except ProcessLookupError:
                    pass
                try:
                    child.wait(timeout=2)
                except subprocess.TimeoutExpired:
                    pass
                try:
                    os.killpg(child.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                child.wait()
        finally:
            restoration_error = None
            try:
                signal.signal(signal.SIGWINCH, previous_resize)
                termios.tcsetattr(terminal, termios.TCSANOW, original)
                termios.tcflush(terminal, termios.TCIFLUSH)
                if failed or (child and child.returncode is not None and child.returncode < 0):
                    # A killed client cannot relay the TUI's normal teardown sequences.
                    sys.stdout.write('\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1006l\x1b[?1004l'
                                     '\x1b[?2004l\x1b[<u\x1b[>4;0m\x1b[?1049l\x1b[?25h\x1b[0m')
                    sys.stdout.flush()
                entry['terminal_restoration'] = {'status': 'restored'}
            except (OSError, termios.error) as error:
                restoration_error = str(error)
                entry['terminal_restoration'] = {'status': 'failed', 'error': restoration_error}
            finally:
                entry['elapsed_seconds'] = time.time() - entry['started']
                machine.persist()
            if restoration_error and not failed:
                raise RuntimeError('terminal restoration failed: ' + restoration_error)
