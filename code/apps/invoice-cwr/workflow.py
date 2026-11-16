#!/usr/bin/env python3
"""Run a documented scenario or grade an immutable saved workspace in a fresh VM."""
import hashlib
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import uuid

sys.path.insert(0, str(Path(__file__).resolve().parent / 'runner'))
import acceptance_eval as invoice
import workspace


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def run_cli(root, output, args, command):
    cli = root / 'runner' / 'cli.py'
    argv = [sys.executable, str(cli), 'evals', 'run', '--vm-type', 'tart',
            '--state', str(output / 'state'), '--boot-timeout', '600', '--timeout', '300', *args, '--', *command]
    with (output / 'stdout').open('wb') as out, (output / 'stderr').open('wb') as err:
        child = subprocess.Popen(argv, cwd=root, stdout=out, stderr=err)
        def stop(number, _frame):
            if child.poll() is None:
                child.send_signal(number)
        previous = {s: signal.signal(s, stop) for s in (signal.SIGINT, signal.SIGTERM, signal.SIGHUP)}
        try:
            child.wait()
        finally:
            for number, handler in previous.items():
                signal.signal(number, handler)
    directory, = (output / 'state').glob('run-*')
    record_path = directory / 'result.json'
    record = json.loads(record_path.read_text())
    return child.returncode, record_path, record


def run_scenario(root, output, request):
    scenario = root / 'scenarios' / request['scenario']
    if scenario.is_symlink() or not scenario.resolve().is_relative_to((root / 'scenarios').resolve()):
        raise ValueError('scenario must be a local folder in this installation')
    task = scenario / 'task.md'
    rubric = scenario / 'grading' / 'contract.json'
    for path in (task, rubric):
        if path.is_symlink() or not path.resolve().is_relative_to(scenario.resolve()):
            raise ValueError('scenario metadata must not link outside its folder')
    prompt = task.read_text()
    if not prompt.strip() or len(prompt.encode()) > 65536:
        raise ValueError('task.md must contain 1..65536 bytes')
    contract = json.loads(rubric.read_text())
    if contract != {'grader': 'invoice-total-v1', 'version': 1}:
        raise ValueError('this release supports the invoice-total-v1 grading contract')
    (output / 'task.md').write_text(prompt)
    (output / 'grading-contract.json').write_text(json.dumps(contract, indent=2) + '\n')
    config = json.loads((root / 'config.json').read_text())
    code, record_path, record = run_cli(root, output, [
        '--reuse-host-auth', '--codex-payload', config['payload'],
        '--workspace-input', str(scenario / 'workspace'),
        '--workspace-output', str(output / 'export')],
        ['codex', 'exec', '--skip-git-repo-check', '--sandbox', 'danger-full-access', prompt])
    export = output / 'export'
    manifest = export / 'manifest.json'
    success = code == 0 and record['outcome'] == 'success' and record['cleanup']['status'] == 'removed' and record.get('workspace_export', {}).get('status') == 'complete'
    result = {'status': 'completed' if success else 'error', 'scenario': request['scenario'],
            'task_sha256': digest(output / 'task.md'),
            'contract_sha256': digest(output / 'grading-contract.json'),
            'run_record': str(record_path), 'runner_result': record}
    if record.get('workspace_export', {}).get('status') == 'complete' and manifest.is_file():
        result.update(workspace=str(export / 'workspace'), workspace_manifest=str(manifest),
                      workspace_manifest_sha256=digest(manifest),
                      diff=str(export / 'diff.patch'), changes=str(export / 'changes.json'))
    if not success:
        result['error'] = 'scenario execution or workspace export/cleanup failed; inspect ' + str(record_path)
    return result


def grade_result(root, output, request):
    saved = root / 'runs' / request['run_id'] / 'evaluation'
    if saved.is_symlink() or not saved.resolve().is_relative_to((root / 'runs').resolve()):
        raise ValueError('saved run must be inside this installation')
    source = json.loads((saved / 'eval-result.json').read_text())
    if source['status'] != 'completed':
        raise ValueError('select a completed scenario run, not an invoice or grading run')
    # Paths are derived from the selected run, never taken from model-authored files.
    exported = saved / 'export'
    if digest(exported / 'manifest.json') != source['workspace_manifest_sha256']:
        raise ValueError('saved workspace manifest changed')
    if digest(saved / 'grading-contract.json') != source['contract_sha256']:
        raise ValueError('saved grading contract changed')
    if json.loads((saved / 'grading-contract.json').read_text()) != {'grader': 'invoice-total-v1', 'version': 1}:
        raise ValueError('unsupported saved grading contract')
    expected = json.loads((exported / 'manifest.json').read_text())
    if workspace.manifest(exported / 'workspace') != expected:
        raise ValueError('saved workspace changed; original artifacts cannot be regraded after editing')
    provenance = {'source_run_id': request['run_id'],
                  'source_manifest_sha256': source['workspace_manifest_sha256'],
                  'grader': 'invoice-total-v1', 'grader_source_sha256': digest(root / 'runner' / 'acceptance_eval.py'),
                  'contract_sha256': source['contract_sha256'], 'grading_network': 'offline',
                  'grading_authentication': False}
    solution = exported / 'workspace' / 'invoice-total.sh'
    if not solution.is_file() or solution.stat().st_size == 0:
        reason = 'invoice-total.sh is empty' if solution.is_file() else 'invoice-total.sh missing'
        if solution.is_file():
            (output / 'invoice-total.sh').write_bytes(solution.read_bytes())
        return {**provenance, 'status': 'failed', 'passed': False, 'cases_passed': [],
                'cases_total': len(invoice.CASES),
                'artifact_sha256': digest(solution) if solution.is_file() else None,
                'cases': [{'name': name, 'passed': False, 'reason': reason}
                          for name, _, _ in invoice.CASES]}
    token = uuid.uuid4().hex
    code, record_path, record = run_cli(root, output, [
        '--network', 'offline', '--workspace-input', str(exported / 'workspace')],
        ['/bin/sh', '-c', 'set -eu\ntest -f invoice-total.sh\n' + invoice.grade_script(token)])
    if record.get('inputs', {}).get('workspace', {}).get('manifest') != expected:
        raise ValueError('grading input changed before its frozen snapshot')
    guest_output = record_path.with_name('guest-stdout').read_text() if record_path.with_name('guest-stdout').is_file() else ''
    result = invoice.collect(output, record, guest_output, token, code)
    result.update(**provenance, run_record=str(record_path))
    return result


def main():
    root, job = Path(sys.argv[1]), Path(sys.argv[2])
    request = json.loads((job / 'request.json').read_text())
    output = job / 'evaluation'
    output.mkdir(mode=0o700)
    try:
        result = (run_scenario if request['action'] == 'run-scenario' else grade_result)(root, output, request)
    except (OSError, ValueError, RuntimeError, KeyError) as error:
        result = {'status': 'error', 'error': str(error), 'cases_passed': [], 'cases_total': 5}
        records = list((output / 'state').glob('run-*/result.json'))
        if len(records) == 1:
            result['run_record'] = str(records[0])
            result['runner_result'] = json.loads(records[0].read_text())
    (output / 'eval-result.json').write_text(json.dumps(result, indent=2) + '\n')
    return 1 if result['status'] == 'error' else 0


if __name__ == '__main__':
    raise SystemExit(main())
