#!/usr/bin/env python3
"""Invoke the local CWR runtime; retain receipts and supervise caller cancellation."""
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import threading
import uuid


def run(parent_fd):
    project = Path(__file__).resolve().parent
    runtime = Path.home() / "code/evals-local-runtime/scenario-cwr/invoke"
    if not runtime.is_file():
        print("Local runtime is missing. Follow SETUP.md in this folder.", file=sys.stderr)
        return 1
    child = subprocess.Popen([str(runtime), *sys.argv[1:]], stdout=subprocess.PIPE,
                             stderr=subprocess.PIPE, text=True, start_new_session=True)
    cancelled = threading.Event()
    def cancel(_number=None, _frame=None):
        cancelled.set()
        if child.poll() is None:
            try:
                os.killpg(child.pid, signal.SIGINT)
            except ProcessLookupError:
                pass
    for number in (signal.SIGINT, signal.SIGTERM, signal.SIGHUP):
        signal.signal(number, cancel)
    def watch_parent():
        with os.fdopen(parent_fd, "rb") as parent:
            parent.read(1)
        cancel()
    threading.Thread(target=watch_parent, daemon=True).start()
    stdout, stderr = child.communicate()
    try:
        response = json.loads(stdout)
    except ValueError:
        response = None
    receipt = {"arguments": sys.argv[1:], "caller_exit": child.returncode,
               "cancelled": cancelled.is_set(), "response": response,
               "stdout": stdout, "stderr": stderr}
    folder = project / "invocations"
    folder.mkdir(exist_ok=True)
    name = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ-") + uuid.uuid4().hex + ".json"
    (folder / name).write_text(json.dumps(receipt, indent=2) + "\n")
    print(stdout, end="")
    print(stderr, end="", file=sys.stderr)
    return child.returncode if child.returncode >= 0 else 128 - child.returncode


def main():
    if len(sys.argv) > 2 and sys.argv[1] == "--supervise-fd":
        fd = int(sys.argv[2])
        del sys.argv[1:3]
        return run(fd)
    read_fd, write_fd = os.pipe()
    child = subprocess.Popen([sys.executable, str(Path(__file__).resolve()),
                              "--supervise-fd", str(read_fd), *sys.argv[1:]],
                             pass_fds=(read_fd,), start_new_session=True)
    os.close(read_fd)
    alive = True
    def cancel(_number=None, _frame=None):
        nonlocal alive
        if alive:
            alive = False
            os.close(write_fd)
    for number in (signal.SIGINT, signal.SIGTERM, signal.SIGHUP):
        signal.signal(number, cancel)
    try:
        return child.wait()
    finally:
        cancel()


if __name__ == "__main__":
    raise SystemExit(main())
