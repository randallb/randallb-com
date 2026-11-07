#!/usr/bin/env python3
"""Exercise native structured values independently of resource semantics."""
import argparse
import json
from pathlib import Path
import subprocess
import tempfile


def normalize(value):
    if isinstance(value, list):
        return [normalize(item) for item in value] if value else None
    if isinstance(value, dict):
        return {key: normalize(item) for key, item in value.items()}
    return value


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--rbel', type=Path, required=True)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='rbel-value-check-') as directory:
        work = Path(directory)
        source, program = work / 'echo.rbel', work / 'echo'
        source.write_text('(def echo (value) value)\n')
        subprocess.run([str(args.rbel.resolve()), 'build', str(source), '--value-interface',
                        '-o', str(program), '--quiet'], check=True)
        values = ['hello', None, [], ['invoke', 'draft--brief',
                  [['topic', [{'char': 'é'}]], ['notes', []]]],
                  {'pair': ['a', 'tail']}, [{'char': '😀'}, {'char': '\\'}]]
        for value in values:
            result = subprocess.run([str(program)], input=json.dumps(value), text=True,
                                    capture_output=True, timeout=10, check=True)
            assert json.loads(result.stdout) == normalize(value)
        for invalid in ['{bad', '1', '[' * 130 + ']' * 130, json.dumps({'char': 'ab'}),
                        ' ' * 262145]:
            result = subprocess.run([str(program)], input=invalid, text=True,
                                    capture_output=True, timeout=10)
            assert result.returncode and not result.stdout
    print('11 structured transport checks passed')


if __name__ == '__main__':
    main()
