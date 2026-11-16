#!/usr/bin/env python3
"""Verify the relocated portable CLI checks examples before publishing emit output."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
OUTPUT = ROOT / 'target/bootstrap-command-verification'


def chapter(definitions, label, expression, expected):
    return (f'```rbel definition=module-source\n{definitions}\n```\n'
            f'```rbel example={label}\n{expression}\n=>\n{expected}\n```\n')


def unpairs(value):
    if isinstance(value, dict) and set(value) == {'pair'}:
        items, tail = [], value
        while isinstance(tail, dict) and set(tail) == {'pair'}:
            items.append(unpairs(tail['pair'][0]))
            tail = tail['pair'][1]
        return items if tail is None else {'items': items, 'tail': unpairs(tail)}
    return value


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--bundle', type=Path, default=ROOT / 'target/rbel-wasm-bootstrap')
    args = parser.parse_args()
    OUTPUT.mkdir(parents=True, exist_ok=True)
    report_path = OUTPUT / 'results.json'
    report = {'passed': False, 'checks': [],
              'core_sha256': hashlib.sha256((args.bundle / 'libexec/rbel/core.wasm').read_bytes()).hexdigest()}
    report_path.write_text(json.dumps(report, indent=2) + '\n')
    try:
        with tempfile.TemporaryDirectory(prefix='portable CLI with spaces ', dir=OUTPUT) as directory:
            work = Path(directory)
            bundle = work / 'moved portable bundle'
            shutil.copytree(args.bundle, bundle)
            command = bundle / 'bin/rbel'
            root = work / 'main.rbel.md'
            dependency = work / 'words.rbel.md'
            output = work / 'result.wat'
            definitions = '(import "words.rbel.md" as words)\n(def main (x) (words/finish x))'
            library = '(module words)\n(export finish)\n(def helper (x) (join \'welcome x))\n(def finish (x) (helper x))'
            good_root = chapter(definitions, 'greet', "(main '(friend))", '(welcome friend)')
            good_dependency = chapter(library, 'private-greet', "(helper '(friend))", '(welcome friend)')
            root.write_text(good_root)
            dependency.write_text(good_dependency)

            def invoke(label, mode='emit', success=True, source=root, destination=output):
                arguments = [str(command), mode, str(source)]
                if mode == 'emit':
                    arguments += ['-o', str(destination)]
                result = subprocess.run(arguments, text=True, capture_output=True, timeout=300, cwd=work)
                if (result.returncode == 0) != success:
                    raise AssertionError((label, result.returncode, result.stdout[-2000:], result.stderr[-5000:]))
                if not success and (result.stdout or not result.stderr):
                    raise AssertionError((label, 'failure must have diagnostics and no stdout'))
                report['checks'].append(label)
                print('PASS', label, flush=True)
                return result

            invoke('relocated-module-emit-checks-root-and-imported-private-examples')
            if not output.read_bytes().startswith(b'(module'):
                raise AssertionError('emit did not publish a Wasm module')
            read = invoke('relocated-module-read', mode='read')
            forms = unpairs(json.loads(read.stdout))
            if not forms or forms[0][:2] != ['def', 'main'] or len(forms) != 3:
                raise AssertionError(('read-module-forms', forms))
            plain = work / 'data.rbel'
            plain.write_text('nil t 1/3 "é世界🦉" \\u{0}')
            read = invoke('ordinary-data-unicode-and-scalar-read', mode='read', source=plain)
            forms = unpairs(json.loads(read.stdout))
            if len(forms) != 5 or forms[0] is not None or forms[-1] != {'char': '\0'}:
                raise AssertionError(('read-data', forms))

            ascii_source = work / 'ascii.rbel.md'
            ascii_output = work / 'ascii.wat'
            ascii_source.write_text(chapter('(def identity (x) x)', 'ascii-equality', "(identity '(a))", '(a)'))
            invoke('simple-ascii-example-emits', source=ascii_source, destination=ascii_output)
            ascii_source.write_text(chapter('(def identity (x) x)', 'ascii-equality', "(identity '(a))", '(b)'))
            ascii_output.write_bytes(b'stale-success')
            failed = invoke('simple-ascii-false-example-clears-output', success=False,
                            source=ascii_source, destination=ascii_output)
            if ascii_output.exists() or 'example ascii-equality failed' not in failed.stderr:
                raise AssertionError(('ascii-false-example', failed.stderr))

            root.write_text(chapter(definitions, 'greet', "(main '(friend))", '(wrong)'))
            output.write_bytes(b'stale-success')
            failed = invoke('false-root-example-clears-old-output', success=False)
            if output.exists() or 'example greet failed' not in failed.stderr:
                raise AssertionError(('false-root-example', failed.stderr))
            root.write_text(good_root)
            dependency.write_text(chapter(library, 'private-greet', "(helper '(friend))", '(wrong)'))
            output.write_bytes(b'stale-success')
            failed = invoke('false-imported-example-clears-old-output', success=False)
            if output.exists() or 'words.rbel.md:' not in failed.stderr or 'example private-greet failed' not in failed.stderr:
                raise AssertionError(('false-imported-example', failed.stderr))
            dependency.write_text(good_dependency)
            root.write_text(chapter(definitions, 'raises', "(err 'deliberate-example-error)", 'nil'))
            output.write_bytes(b'stale-success')
            failed = invoke('runtime-example-error-clears-old-output', success=False)
            if output.exists() or 'deliberate-example-error' not in failed.stderr:
                raise AssertionError(('runtime-example-error', failed.stderr))

            # Discovery fails before old output is removed; semantic checks fail after.
            root.write_text('```rbel definition=module-source\n(import "absent.rbel")\n(def main (x) x)\n```\n')
            output.write_bytes(b'keep-existing')
            missing = invoke('missing-import-preserves-old-output', success=False)
            if 'absent.rbel' not in missing.stderr:
                raise AssertionError(('missing-import', missing.stderr))
            if output.read_bytes() != b'keep-existing':
                raise AssertionError('discovery failure removed output')
            root.write_bytes(b'\xff')
            failed = invoke('invalid-utf8-preserves-old-output', success=False)
            if output.read_bytes() != b'keep-existing' or 'UTF-8' not in failed.stderr:
                raise AssertionError(('invalid-utf8', failed.stderr))
            root.write_text('```rbel definition=module-source\n(import "words.rbel.md" as words)\n(def main (x) (words/helper x))\n```\n')
            output.write_bytes(b'stale-success')
            invoke('private-reference-clears-old-output', success=False)
            if output.exists():
                raise AssertionError('semantic failure retained stale output')

            root.write_text(good_root)
            before = dependency.read_bytes()
            alias = work / 'source hardlink.wat'
            os.link(dependency, alias)
            invoke('transitive-source-hardlink-protected', success=False, destination=alias)
            if dependency.read_bytes() != before or alias.read_bytes() != before:
                raise AssertionError('source alias damaged')
            resource = bundle / 'libexec/rbel/core.wasm'
            resource_before = hashlib.sha256(resource.read_bytes()).hexdigest()
            resource_alias = work / 'bundle hardlink.wat'
            os.link(resource, resource_alias)
            invoke('bundle-hardlink-protected', success=False, destination=resource_alias)
            if hashlib.sha256(resource.read_bytes()).hexdigest() != resource_before:
                raise AssertionError('bundle alias damaged')
            protected = work / 'unrelated-source.rbel'
            protected.write_text('(def retained (x) x)')
            invoke('protected-source-extension', success=False, destination=protected)
            if protected.read_text() != '(def retained (x) x)':
                raise AssertionError('protected extension overwritten')
        report['passed'] = True
    finally:
        report_path.write_text(json.dumps(report, indent=2) + '\n')
    print(f"{len(report['checks'])} portable CLI verification groups passed")


if __name__ == '__main__':
    main()
