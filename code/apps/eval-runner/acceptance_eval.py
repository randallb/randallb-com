"""One live coding eval with saved solution, independent grading and VM cleanup."""
import argparse
import base64
import hashlib
import json
import os
from pathlib import Path
import shlex
import signal
import subprocess
import sys
import uuid

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


def grade_script(token):
    """Run all cases after coding, retaining the solution even when checks fail."""
    lines = ["printf '\\n%s\\n' " + shlex.quote('EVAL_BEGIN ' + token),
             "printf '%s\\n' EVAL_ARTIFACT_BEGIN", '/usr/bin/base64 < invoice-total.sh',
             "printf '\\n%s\\n' EVAL_ARTIFACT_END"]
    for name, data, expected in CASES:
        lines += ["cat > grade.csv <<'CASE_END'", data, 'CASE_END',
                  'if actual=$(/bin/sh invoice-total.sh grade.csv) && test "$actual" = ' + shlex.quote(expected) + '; then',
                  "printf '%s\\n' " + shlex.quote('EVAL_CASE_PASS ' + name),
                  'else', "printf '%s\\n' " + shlex.quote('EVAL_CASE_FAIL ' + name), 'fi']
    lines += ["printf '%s\\n' " + shlex.quote('EVAL_COMPLETE ' + token)]
    return '\n'.join(lines)


def guest_script(token):
    lines = ['set -eu', 'mkdir /private/tmp/invoice-eval', 'cd /private/tmp/invoice-eval',
             "cat > TASK.txt <<'TASK_END'", TASK, 'TASK_END',
             "cat > example.csv <<'EXAMPLE_END'", CASES[0][1], 'EXAMPLE_END',
             'codex exec --skip-git-repo-check --sandbox danger-full-access ' + shlex.quote(
                 'Read TASK.txt and implement the requested CLI. Use your tools to create and test it.'),
             'test -f invoice-total.sh']
    # Only the final grading section counts; model turn stdout is not grading evidence.
    lines += [grade_script(token)]
    return '\n'.join(lines)


def collect(root, record, output, token, returncode):
    report = {'task': 'invoice-total', 'status': 'error', 'passed': False,
              'cases_passed': [], 'cases_total': len(CASES), 'cases': [],
              'artifact_sha256': None, 'runner_result': record}
    try:
        section = output.rsplit('\nEVAL_BEGIN ' + token + '\n', 1)[1]
        if section.count('EVAL_COMPLETE ' + token) != 1 or not section.endswith('EVAL_COMPLETE ' + token + '\n'):
            raise ValueError('grading did not complete')
        encoded, results = section.split('EVAL_ARTIFACT_END\n', 1)
        encoded = encoded.removeprefix('EVAL_ARTIFACT_BEGIN\n')
        artifact = base64.b64decode(''.join(encoded.split()), validate=True)
        if not artifact:
            raise ValueError('solution is empty')
        (root / 'invoice-total.sh').write_bytes(artifact)
        report['artifact_sha256'] = hashlib.sha256(artifact).hexdigest()
        rows = results.splitlines()
        expected_names = [name for name, _, _ in CASES]
        if len(rows) != len(CASES) + 1:
            raise ValueError('unexpected grading output')
        for row, name in zip(rows, expected_names):
            if row not in ('EVAL_CASE_PASS ' + name, 'EVAL_CASE_FAIL ' + name):
                raise ValueError('invalid grading case output')
            passed = row == 'EVAL_CASE_PASS ' + name
            report['cases'].append({'name': name, 'passed': passed})
            if passed:
                report['cases_passed'].append(name)
        if returncode != 0 or record.get('outcome') != 'success' or record.get('cleanup', {}).get('status') != 'removed':
            raise ValueError('runner execution or VM cleanup failed')
        report['passed'] = len(report['cases_passed']) == len(CASES)
        report['status'] = 'passed' if report['passed'] else 'failed'
    except (IndexError, ValueError, KeyError) as error:
        report['error'] = str(error) or 'grading evidence missing'
    return report


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
    token = uuid.uuid4().hex
    cli = Path(__file__).with_name('cli.py')
    argv = [sys.executable, str(cli), 'evals', 'run', '--vm-type', 'tart',
            '--reuse-host-auth', '--codex-payload', str(Path(args.payload).resolve()),
            '--state', str(root / 'state'), '--timeout', '300', '--', '/bin/sh', '-c', guest_script(token)]
    # Lifecycle output is persisted by the runner before owned VM cleanup.
    with (root / 'stdout').open('wb') as stdout, (root / 'stderr').open('wb') as stderr:
        child = subprocess.Popen(argv, stdout=stdout, stderr=stderr)
        def stop(number, _frame):
            if child.poll() is None:
                child.send_signal(number)
        previous = {s: signal.signal(s, stop) for s in (signal.SIGINT, signal.SIGTERM, signal.SIGHUP)}
        try:
            child.wait()
        finally:
            for number, handler in previous.items():
                signal.signal(number, handler)
    try:
        directory, = (root / 'state').glob('run-*')
        record = json.loads((directory / 'result.json').read_text())
        output = (directory / 'guest-stdout').read_text() if (directory / 'guest-stdout').exists() else ''
        report = collect(root, record, output, token, child.returncode)
        report['run_record'] = str(directory / 'result.json')
    except (OSError, ValueError) as error:
        report = {'task': 'invoice-total', 'status': 'error', 'passed': False,
                  'cases_passed': [], 'cases_total': len(CASES), 'error': str(error)}
    (root / 'eval-result.json').write_text(json.dumps(report, indent=2) + '\n')
    print(f"{report['status'].upper()} invoice-total: {len(report['cases_passed'])}/{len(CASES)} checks")
    print(root / 'eval-result.json')
    return 0 if report['status'] == 'passed' else 1


if __name__ == '__main__':
    raise SystemExit(main())
