"""One live coding eval with saved solution, independent grading and VM cleanup."""
import argparse
import base64
import hashlib
import json
import os
from pathlib import Path
import shlex
import subprocess
import sys

TASK = '''Implement invoice-total.sh, a POSIX shell CLI taking one CSV file path.
The CSV header is sku,quantity,unit_cents. Each subsequent nonblank row has
an unquoted SKU, a nonnegative integer quantity, and a nonnegative integer
price in cents. Print only the sum of quantity * unit_cents, as integer cents
and a newline. A header-only file must print 0. Ignore blank lines. Use the
system shell and awk, with no downloads or dependencies. Create the file and
run your own checks. The fixture example.csv should total 1350.
'''
CASES = [
    ('example', 'sku,quantity,unit_cents\npen,3,150\nbook,2,450\n', '1350'),
    ('empty', 'sku,quantity,unit_cents\n', '0'),
    ('blank-lines', 'sku,quantity,unit_cents\n\na,2,107\n\nb,3,201\n\n', '817'),
    ('zero-values', 'sku,quantity,unit_cents\na,0,999\nb,4,0\nc,1,1\n', '1'),
    ('large-total', 'sku,quantity,unit_cents\na,125,1999\na,30,350\n', '260375'),
]


def guest_script():
    lines = ['set -eu', 'mkdir /private/tmp/invoice-eval', 'cd /private/tmp/invoice-eval',
             "cat > TASK.txt <<'TASK_END'", TASK, 'TASK_END',
             "cat > example.csv <<'EXAMPLE_END'", CASES[0][1], 'EXAMPLE_END',
             'codex exec --skip-git-repo-check --sandbox danger-full-access ' + shlex.quote(
                 'Read TASK.txt and implement the requested CLI. Use your tools to create and test it.'),
             'test -f invoice-total.sh']
    # Grading is supplied after the coding turn; expected answers are runner-owned.
    for name, data, expected in CASES:
        lines += ["cat > grade.csv <<'CASE_END'", data, 'CASE_END',
                  'actual=$(/bin/sh invoice-total.sh grade.csv)',
                  'test "$actual" = ' + shlex.quote(expected),
                  "printf '%s\\n' " + shlex.quote('EVAL_CASE_PASS ' + name)]
    lines += ["printf '%s\\n' EVAL_ARTIFACT_BEGIN", '/usr/bin/base64 < invoice-total.sh',
              "printf '\\n%s\\n' EVAL_ARTIFACT_END", "printf '%s\\n' EVAL_ALL_PASS"]
    return '\n'.join(lines)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--payload', default=os.environ.get('RBDC_CODEX_PAYLOAD'), required=False)
    parser.add_argument('--output', required=True, type=Path)
    args = parser.parse_args()
    if not args.payload:
        parser.error('supply --payload with the Nix evals-codex-payload output')
    root = args.output.resolve()
    root.mkdir(parents=True, exist_ok=False)
    (root / 'task.txt').write_text(TASK)
    (root / 'grading-cases.json').write_text(json.dumps(CASES, indent=2) + '\n')
    cli = Path(__file__).with_name('cli.py')
    argv = [sys.executable, str(cli), 'evals', 'run', '--vm-type', 'tart',
            '--reuse-host-auth', '--codex-payload', str(Path(args.payload).resolve()),
            '--state', str(root / 'state'), '--timeout', '300', '--', '/bin/sh', '-c', guest_script()]
    # Lifecycle output is persisted by the runner before owned VM cleanup.
    with (root / 'stdout').open('wb') as stdout, (root / 'stderr').open('wb') as stderr:
        result = subprocess.run(argv, stdout=stdout, stderr=stderr)
    directory, = (root / 'state').glob('run-*')
    record = json.loads((directory / 'result.json').read_text())
    output = (directory / 'guest-stdout').read_text() if (directory / 'guest-stdout').exists() else ''
    passed = [name for name, _, _ in CASES if 'EVAL_CASE_PASS ' + name in output.splitlines()]
    artifact_hash = None
    if '\nEVAL_ARTIFACT_BEGIN\n' in output and '\nEVAL_ARTIFACT_END' in output:
        encoded = output.split('\nEVAL_ARTIFACT_BEGIN\n', 1)[1].split('\nEVAL_ARTIFACT_END', 1)[0]
        artifact = base64.b64decode(''.join(encoded.split()), validate=True)
        (root / 'invoice-total.sh').write_bytes(artifact)
        artifact_hash = hashlib.sha256(artifact).hexdigest()
    success = (result.returncode == 0 and record['outcome'] == 'success'
               and record['cleanup']['status'] == 'removed'
               and len(passed) == len(CASES) and artifact_hash is not None
               and 'EVAL_ALL_PASS' in output.splitlines())
    report = {'task': 'invoice-total', 'passed': success, 'cases_passed': passed,
              'cases_total': len(CASES), 'artifact_sha256': artifact_hash,
              'run_record': str(directory / 'result.json'), 'runner_result': record}
    (root / 'eval-result.json').write_text(json.dumps(report, indent=2) + '\n')
    print(f"{'PASS' if success else 'FAIL'} invoice-total: {len(passed)}/{len(CASES)} checks; cleanup={record['cleanup']['status']}")
    print(root / 'eval-result.json')
    return 0 if success else 1


if __name__ == '__main__':
    raise SystemExit(main())
