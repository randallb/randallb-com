#!/usr/bin/env python3
"""Isolated regressions for stale-source acceptance and destructive output aliases."""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
from types import SimpleNamespace

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--package', type=Path, required=True)
    args = parser.parse_args()
    package = args.package.resolve()
    manifest = json.loads((package / 'manifest.json').read_text())
    with tempfile.TemporaryDirectory(prefix='libcwr-regressions-') as directory:
        work = Path(directory)
        tree = work / 'source'
        for name in manifest['inputs']:
            destination = tree / name
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(ROOT / name, destination)
        adapter = tree / 'libcwr/embedding/core.mjs'
        adapter.write_text("throw Error('unbuilt maintained adapter');\n" + adapter.read_text())
        def rejected(selected):
            run = subprocess.run([sys.executable, str(tree / 'libcwr/embedding/verify.py'),
                                  '--package', str(selected)], capture_output=True, text=True, timeout=30)
            report = json.loads((tree / 'target/libcwr-embedding-checks/verification.json').read_text())
            assert run.returncode != 0 and not report['passed'], run.stdout + run.stderr
            assert report['error'] == 'maintained-input-correspondence', report
        rejected(package)
        shutil.copy2(ROOT / 'libcwr/embedding/core.mjs', adapter)
        incomplete = work / 'incomplete'
        incomplete.mkdir()
        metadata = json.loads(json.dumps(manifest))
        del metadata['inputs']['libcwr/embedding/core.mjs']
        (incomplete / 'manifest.json').write_text(json.dumps(metadata))
        rejected(incomplete)
        print('PASS stale adapter and omitted input cannot receive acceptance')

        spec = importlib.util.spec_from_file_location('native_builder', HERE / 'build-native.py')
        builder = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(builder)
        # Load the copied builder so all maintained-source collision tests stay isolated.
        spec = importlib.util.spec_from_file_location('isolated_native_builder', tree / 'libcwr/embedding/build-native.py')
        builder = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(builder)
        copied = work / 'package'
        copied.mkdir()
        for name in [*manifest['files'], 'manifest.json']:
            shutil.copy2(package / name, copied / name)
        # Corrupt Wasm: a faulty validator would delete the output before noticing.
        (copied / 'core.wasm').write_bytes(b'not wasm')
        original = {p: p.read_bytes() for p in copied.iterdir()}
        for source in [*[copied / name for name in [*manifest['files'], 'manifest.json']],
                       *[tree / name for name in manifest['inputs']]]:
            original[source] = source.read_bytes()
            for kind in ['direct', 'symlink', 'hardlink']:
                destination = source
                if kind != 'direct':
                    destination = work / ('alias-' + kind)
                    destination.unlink(missing_ok=True)
                    if kind == 'symlink': destination.symlink_to(source)
                    else: os.link(source, destination)
                for suffix in ['', '.json', '.runtime-license.txt']:
                    # Exercise collisions by each of the builder's three outputs.
                    output = destination
                    if suffix:
                        # Alias a sidecar of a separate executable to the input.
                        output = work / 'custom-native'
                        alias = output.with_name(output.name + suffix)
                        alias.unlink(missing_ok=True)
                        os.link(source, alias)
                    options = SimpleNamespace(wasm=copied / 'core.wasm', tools=work / 'tools', runtime=work / 'runtime',
                                              cc=Path(sys.executable), runtime_license=ROOT / 'LICENSE', output=output)
                    try:
                        builder.build(options)
                    except ValueError as error:
                        assert 'aliases an input' in str(error) or 'outside source' in str(error), str(error)
                    else:
                        raise AssertionError('accepted protected output: ' + str(output))
                    assert all(p.read_bytes() == data for p, data in original.items())
                    if suffix: alias.unlink()
                if kind != 'direct': destination.unlink()
        print('PASS package and maintained inputs survive direct, symlink, hardlink and sidecar collisions')


if __name__ == '__main__':
    main()
