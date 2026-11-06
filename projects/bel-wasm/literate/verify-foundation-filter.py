#!/usr/bin/env python3
"""Check the built foundation-based native filter without build tools at runtime."""
import hashlib
import json
from pathlib import Path
import platform
import shutil
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]


def main():
    binary = ROOT / 'target/bel-native/rbel-foundation-uppercase'
    cases = [b'', b'hello, rbel!\n', bytes(range(256)), b'a' * 255,
             b'b' * 256, b'c' * 257, 'naïve 世界\n'.encode(), bytes(range(256)) * 4097]
    with tempfile.TemporaryDirectory(prefix='rbel-foundation-filter-') as directory:
        executable = Path(directory) / 'filter'
        shutil.copy2(binary, executable)
        for index, data in enumerate(cases):
            result = subprocess.run([str(executable)], input=data, capture_output=True,
                                    cwd=directory, env={'PATH': ''}, timeout=30)
            assert result.returncode == 0 and result.stdout == data.upper() and not result.stderr, (index, result)
            print(f'PASS foundation filter fixture {index}', flush=True)
        for args, status in [(['--help'], 0), (['unknown'], 2)]:
            result = subprocess.run([str(executable), *args], capture_output=True,
                                    cwd=directory, env={'PATH': ''}, timeout=30)
            assert result.returncode == status, result
    libraries = []
    if sys.platform == 'darwin':
        kind = subprocess.check_output(['/usr/bin/file', binary], text=True)
        if platform.machine() == 'arm64':
            assert 'arm64' in kind, kind
        link = subprocess.check_output(['/usr/bin/otool', '-L', binary], text=True)
        libraries = [line.strip().split(' (')[0] for line in link.splitlines()[1:]]
        assert libraries and all(x.startswith(('/usr/lib/', '/System/Library/')) for x in libraries), link
    receipt = {'platform': platform.platform(), 'architecture': platform.machine(),
               'sha256': hashlib.sha256(binary.read_bytes()).hexdigest(), 'bytes': binary.stat().st_size,
               'empty_path_execution': True, 'byte_fixtures': len(cases), 'dynamic_libraries': libraries,
               'source_sha256': hashlib.sha256((ROOT / 'target/literate/foundation-filter.rbel').read_bytes()).hexdigest()}
    (ROOT / 'target/bel-native/foundation-results.json').write_text(json.dumps(receipt, indent=2) + '\n')
    print('PASS: foundation filter byte streams, help, usage, architecture, and system libraries')


if __name__ == '__main__':
    main()
