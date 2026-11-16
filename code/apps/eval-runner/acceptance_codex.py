"""Actual offline macOS Codex installation/reset/failure checks. No model calls."""
import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys

from codex_payload import PIN


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--output', required=True, type=Path, help='new evidence directory')
    parser.add_argument('--payload', default=os.environ.get('RBDC_CODEX_PAYLOAD'))
    parser.add_argument('--tart-home', type=Path, default=Path('.rbdc-evals/tart'))
    args = parser.parse_args()
    if not args.payload:
        parser.error('supply --payload with the output of nix build .#evals-codex-payload --no-link --print-out-paths')
    root = args.output.resolve()
    root.mkdir(parents=True, exist_ok=False)
    cli = Path(__file__).with_name('cli.py')
    reports = []

    def case(name, command, install=True, backend=None, expected=0, outcome='success'):
        target = root / name
        target.mkdir()
        argv = [sys.executable, str(cli), 'evals', 'run', '--vm-type', 'tart',
                '--tart-home', str(args.tart_home.resolve()), '--state', str(target / 'state')]
        if install:
            argv += ['--with-codex', '--codex-payload', str(Path(args.payload).resolve())]
        if backend:
            argv += ['--tart', str(backend)]
        with (target / 'stdout').open('wb') as stdout, (target / 'stderr').open('wb') as stderr:
            result = subprocess.run([*argv, '--', *command], stdout=stdout, stderr=stderr,
                                    env=dict(os.environ, OPENAI_API_KEY='synthetic-host-sentinel'), timeout=360)
        directory, = (target / 'state').glob('run-*')
        record = json.loads((directory / 'result.json').read_text())
        assert result.returncode == expected, (name, record)
        assert record['outcome'] == outcome, (name, record)
        assert record['cleanup']['status'] == 'removed', (name, record)
        reports.append({'case': name, 'passed': True, 'record': str(directory), 'result': record})
        (root / 'report.json').write_text(json.dumps(reports, indent=2) + '\n')
        print('PASS ' + name, flush=True)
        return record

    check = ('set -eu; test ! -e /private/tmp/rbdc-codex-reset; '
             'test -z "${OPENAI_API_KEY:-}"; test ! -e "$HOME/.codex/auth.json"; '
             'test ! -d "/Volumes/My Shared Files"; codex --version; codex --help >/dev/null; '
             'if /usr/bin/curl --noproxy "*" --connect-timeout 3 --max-time 4 -s http://1.1.1.1 >/dev/null; then exit 44; fi; '
             'touch /private/tmp/rbdc-codex-reset')
    first = case('first-install', ['/bin/sh', '-c', check])
    second = case('fresh-install', ['/bin/sh', '-c', check])
    for result in (first, second):
        assert result['codex_installation']['observed_version'] == PIN['expected_version']
        assert result['codex_installation']['sha256'] == PIN['sha256']
    assert first['machine'] != second['machine']
    assert first['inputs'] == second['inputs']
    assert first['codex_installation']['binary_sha256'] == second['codex_installation']['binary_sha256']
    case('unprovisioned-reset', ['/bin/sh', '-c',
         'test ! -e /private/tmp/rbdc-tools && test ! -e /private/tmp/rbdc-codex-reset'], install=False)
    # Inject only a transport failure; clone, boot and cleanup still use the real backend/VM.
    backend = str(Path(shutil.which('tart')).resolve())
    wrapper = root / 'fail-transfer'
    wrapper.write_text('#!' + sys.executable + '\nimport os,sys\n'
                       'if len(sys.argv)>2 and sys.argv[1]=="exec" and sys.argv[2]=="-i":\n'
                       '    print("injected transfer failure",file=sys.stderr);sys.exit(42)\n'
                       f'os.execv({backend!r}, [{backend!r}, *sys.argv[1:]])\n')
    wrapper.chmod(0o755)
    failure = case('transfer-failure', ['codex', '--version'], backend=wrapper,
                   expected=1, outcome='provisioning_failure')
    assert 'injected transfer failure' in failure['error']
    print(f'{len(reports)} actual macOS cases passed; no authentication/model calls', flush=True)


if __name__ == '__main__':
    main()
