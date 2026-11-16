#!/usr/bin/env python3
"""Verify real CWR scenario execution, full export and two independent grades."""
import argparse
import hashlib
import json
from pathlib import Path
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / 'eval-runner'))
import workspace


def response(path):
    value = json.loads(path.read_text())
    assert value['status'] == 200, 'CWR invocation failed'
    return value, json.loads(value['result']['stdout'])


def removed(path):
    record = json.loads(path.read_text())
    owner = json.loads(path.with_name('owner.json').read_text())
    assert record['outcome'] == 'success' and record['cleanup']['status'] == 'removed'
    assert not (Path(owner['tart_home']) / 'vms' / owner['machine']).exists()
    return record


def verify(root, run_path, grade_paths):
    run, payload = response(run_path)
    assert payload['status'] == 'completed' and 'checks_passed' not in payload
    run_id = payload['run_id']
    evaluation = root / 'runs' / run_id / 'evaluation'
    result = json.loads((evaluation / 'eval-result.json').read_text())
    manifest_path = evaluation / 'export' / 'manifest.json'
    manifest_bytes = manifest_path.read_bytes()
    expected = json.loads(manifest_bytes)
    assert hashlib.sha256(manifest_bytes).hexdigest() == result['workspace_manifest_sha256']
    assert workspace.manifest(evaluation / 'export' / 'workspace') == expected
    assert 'invoice-total.sh' in expected['files']
    changes = json.loads((evaluation / 'export' / 'changes.json').read_text())
    assert any(x['path'] == 'invoice-total.sh' and x['status'] == 'added' for x in changes['files'])
    assert (evaluation / 'export' / 'diff.patch').is_file()
    model_record = removed(Path(result['run_record']))
    assert model_record['source'].get('revision') is None
    starting = model_record['inputs']['workspace']['manifest']
    assert not any(x.startswith('grading/') or x == 'contract.json' for x in starting['files'])
    assert workspace.manifest(root / 'scenarios' / result['scenario'] / 'workspace') == starting
    assert (evaluation / 'stdout').is_file() and (evaluation / 'stderr').is_file()
    grades = []
    for path in grade_paths:
        raw, grade = response(path)
        assert grade['status'] == 'passed' and grade['checks_passed'] == grade['checks_total'] == 5
        assert grade['source_run_id'] == run_id
        saved_grade = json.loads(Path(grade['evidence']['result']).read_text())
        assert saved_grade['source_manifest_sha256'] == result['workspace_manifest_sha256']
        record = removed(Path(saved_grade['run_record']))
        assert record['inputs']['workspace']['manifest'] == expected
        assert record['resources']['network'] == 'discard-only-socket'
        assert 'authentication' not in record and 'codex_installation' not in record
        assert 'codex' not in record['inputs']
        assert manifest_path.read_bytes() == manifest_bytes
        grades.append({'invocation_ref': raw['invocation_ref'], 'run_id': grade['run_id'],
                       'source_run_id': run_id, 'checks_passed': 5,
                       'evidence': grade['evidence'], 'offline': True, 'credentials': False})
    assert len({g['invocation_ref'] for g in grades}) == 2
    return {'verified': True, 'run_id': run_id, 'invocation_ref': run['invocation_ref'],
            'installed_root': str(root), 'original_scenario_unchanged': True,
            'source_manifest_sha256': result['workspace_manifest_sha256'],
            'workspace': payload['evidence']['workspace'], 'diff': payload['evidence']['diff'],
            'run_evidence': payload['evidence'], 'grades': grades, 'all_vms_removed': True}


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--installed', type=Path, required=True)
    parser.add_argument('--run', type=Path, required=True)
    parser.add_argument('--grade', type=Path, required=True)
    parser.add_argument('--regrade', type=Path, required=True)
    args = parser.parse_args()
    print(json.dumps(verify(args.installed.resolve(), args.run, [args.grade, args.regrade]), indent=2))
