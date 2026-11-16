#!/usr/bin/env python3
"""Check retained evidence from a real installed CWR invoice invocation."""
import argparse
import hashlib
import json
from pathlib import Path


def verify(root, response_path):
    response = json.loads(response_path.read_text())
    assert response['status'] == 200, 'CWR action did not complete successfully'
    assert response['invocation_ref'], 'missing actual CWR invocation identity'
    grade = json.loads(response['result']['stdout'])
    assert grade['status'] == 'passed' and grade['checks_passed'] == grade['checks_total'] == 5
    evidence = grade['evidence']
    paths = {name: Path(evidence[name]) for name in ('result', 'solution', 'run_record')}
    for path in paths.values():
        assert path.is_file() and path.resolve().is_relative_to(root.resolve()), 'missing or external evidence'
    result = json.loads(paths['result'].read_text())
    record = json.loads(paths['run_record'].read_text())
    owner = json.loads(paths['run_record'].with_name('owner.json').read_text())
    checksum = hashlib.sha256(paths['solution'].read_bytes()).hexdigest()
    assert result['status'] == 'passed' and result['artifact_sha256'] == checksum
    assert len(result['cases']) == 5 and all(case['passed'] for case in result['cases'])
    assert record['outcome'] == 'success' and record['guest_exit_status'] == 0
    assert record['cleanup']['status'] == 'removed'
    assert not (Path(owner['tart_home']) / 'vms' / owner['machine']).exists(), 'owned VM still exists'
    # The installed invocation runs in the pack rather than discovering a checkout.
    assert record['source'].get('revision') is None, 'acceptance ran within a source checkout'
    return {'verified': True, 'invocation_ref': response['invocation_ref'],
            'checks_passed': 5, 'checks_total': 5, 'solution_sha256': checksum,
            'vm_removed': True, 'installed_root': str(root),
            'evidence': {name: str(path) for name, path in paths.items()}}


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--installed', type=Path, required=True)
    parser.add_argument('--response', type=Path, required=True)
    args = parser.parse_args()
    print(json.dumps(verify(args.installed.resolve(), args.response), indent=2))
