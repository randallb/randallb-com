#!/usr/bin/env python3
"""Invoke the installed pack through its independent local CWR runtime."""
import json
import fcntl
import os
from pathlib import Path
import subprocess
import sys


def invoke(root):
    config = json.loads((root / 'config.json').read_text())
    env = dict(os.environ)
    env.update(XDG_STATE_HOME=str(root / 'cwr-state'), WORKLOOP_STATE_HOME=str(root / 'cwr-state'),
               WORKLOOP_STORAGE_IDENTITY='invoice-cwr-local-v1')
    command = [config['cwr'], '--workspace', str(root / 'runtime')]
    try:
        result = subprocess.run([*command, 'actions', 'invoke', '@invoice_eval--run--invoice'],
                                cwd=root, env=env, text=True, capture_output=True)
        print(result.stdout, end='')
        print(result.stderr, end='', file=sys.stderr)
        if result.stdout:
            (root / 'last-cwr-response.json').write_text(result.stdout)
        return result.returncode
    finally:
        subprocess.run([*command, 'runtime', 'stop'], cwd=root, env=env,
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def main():
    root = Path(__file__).resolve().parent
    with (root / '.invoke.lock').open('a') as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            print('An invoice invocation is already running.', file=sys.stderr)
            return 1
        return invoke(root)


if __name__ == '__main__':
    raise SystemExit(main())
