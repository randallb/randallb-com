#!/usr/bin/env python3
"""Exercise source imports through the installed native command, in isolation."""
from pathlib import Path
import json
import os
import shutil
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[4]
BUNDLE = ROOT / 'target/rbel-command'
CHECKS = []


def sequence(value):
    result = []
    while value is not None:
        head, value = value['pair']
        result.append(head)
    return result


def definition_names(result):
    return [sequence(form)[1] for form in sequence(json.loads(result.stdout))]


def literate(name, definitions, expression=None, expected=None):
    source = f'```rbel definition={name}\n{definitions}\n```\n'
    if expression is not None:
        source += f'```rbel example={name}-example\n{expression}\n=>\n{expected}\n```\n'
    return source


def main():
    with tempfile.TemporaryDirectory(prefix='rbel imports ') as directory:
        work = Path(directory)
        bundle = work / 'moved bundle'
        shutil.copytree(BUNDLE, bundle)
        command = bundle / 'bin/rbel'
        guards = work / 'forbidden tools'
        guards.mkdir()
        marker = work / 'forbidden-execution'
        for name in ['node', 'python', 'python3', 'rustc', 'cargo', 'deno']:
            guard = guards / name
            guard.write_text(f'#!/bin/sh\n/bin/echo {name} >> "{marker}"\nexit 99\n')
            guard.chmod(0o755)
        env = {**os.environ, 'PATH': str(guards), 'LC_ALL': 'C'}

        def write(path, source):
            path = work / path
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(source)
            return path

        def invoke(name, mode, source, *args, success=True):
            result = subprocess.run([command, mode, source, *args], cwd=work,
                                    env=env, capture_output=True, timeout=90)
            assert result.returncode == (0 if success else 1), (name, result.returncode, result.stdout, result.stderr)
            if not success:
                assert not result.stdout and result.stderr, (name, result)
            CHECKS.append(name)
            return result

        dependency = write('modules/helper.rbel', '(def helper (xs) (join \\X xs))\n')
        source = write('main.rbel', '(import "modules/helper.rbel")\n(def main (xs) (helper xs))\n')
        result = invoke('raw-import-read-and-entry-order', 'read', source)
        assert definition_names(result) == ['main', 'helper'], result.stdout
        output = work / 'native program'
        invoke('native-import-build', 'build', source, '-o', output)
        assert subprocess.check_output([output], input=b'abc', env={'PATH': ''}) == b'Xabc'
        CHECKS.append('native-import-execution')

        write('modules/sub/leaf.rbel', '(def leaf (xs) xs)\n')
        write('modules/helper.rbel', '(import "sub/leaf.rbel")\n(def helper (xs) (leaf xs))\n')
        result = invoke('nested-relative-to-importer', 'read', source)
        assert definition_names(result) == ['main', 'helper', 'leaf'], result.stdout
        write('modules/second.rbel', '(import "sub/leaf.rbel")\n(def second (xs) (leaf xs))\n')
        source.write_text('(import "modules/helper.rbel")\n(import "modules/second.rbel")\n(def main (xs) (helper xs))\n')
        result = invoke('diamond-load-once', 'read', source)
        assert definition_names(result) == ['main', 'helper', 'leaf', 'second'], result.stdout
        source.write_text('(def main (xs) xs)\n(import "modules/sub/../sub/leaf.rbel")\n(import "modules/sub/leaf.rbel")\n')
        result = invoke('canonical-duplicate', 'read', source)
        assert definition_names(result) == ['main', 'leaf'], result.stdout
        leaf = work / 'modules/sub/leaf.rbel'
        (work / 'linked.rbel').symlink_to(leaf)
        os.link(leaf, work / 'hardlinked.rbel')
        source.write_text('(def main (xs) xs)\n(import "modules/sub/leaf.rbel")\n(import "linked.rbel")\n(import "hardlinked.rbel")\n')
        result = invoke('symlink-hardlink-deduplication', 'read', source)
        assert definition_names(result) == ['main', 'leaf'], result.stdout
        result = invoke('include-and-import-deduplication', 'read', source, '--include', leaf)
        assert definition_names(result) == ['main', 'leaf'], result.stdout

        odd = write('modules/helper $() `literal` file.rbel', '(def odd (xs) xs)\n')
        source.write_text(f'(def main (xs) (odd xs))\n(import "modules/{odd.name}")\n')
        assert definition_names(invoke('spaces-and-shell-metacharacters', 'read', source)) == ['main', 'odd']
        write('modules/parent.rbel', '(def parent (xs) xs)\n(import "../linked.rbel")\n')
        assert definition_names(invoke('parent-relative-import', 'read', work / 'modules/parent.rbel')) == ['parent', 'leaf']
        source.write_text('(def main (xs) xs)\r\n(import\r\n ; path comment\r\n "linked.rbel"\r\n)\r\n')
        assert definition_names(invoke('whitespace-comments-crlf', 'read', source)) == ['main', 'leaf']

        # Import-like text is ordinary language data unless it is an unquoted top-level form.
        untouched = ["'(import \"missing.rbel\")", '(def main (xs) (import "missing.rbel"))',
                     '"(import \\"missing.rbel\\")"', '; (import "missing.rbel")\n(def main (xs) xs)',
                     '(def main (xs) \\;)\n; (import "missing.rbel")\n']
        untouched.extend(["' (import \"missing.rbel\")", "'\n(import \"missing.rbel\")", "'; comment\n(import \"missing.rbel\")"])
        for index, text in enumerate(untouched):
            source.write_text(text)
            invoke(f'import-like-data-{index}', 'read', source)

        bad_forms = ['(import)', '(import missing)', '(import "linked.rbel" extra)',
                     '(import "")', '(import "/tmp/absolute.rbel")', '(import "cwp://example/source")',
                     '(import "https://example/source")', '(import . "linked.rbel")', '(import "missing.rbel")']
        for index, form in enumerate(bad_forms):
            source.write_text('(def main (xs) xs)\n' + form + '\n')
            invoke(f'invalid-import-{index}', 'read', source, success=False)
        source.write_text('(def main (xs) xs)\n(import "main.rbel")\n')
        invoke('self-cycle', 'read', source, success=False)
        write('cycle/a.rbel', '(import "b.rbel")\n(def a (xs) xs)')
        write('cycle/b.rbel', '(import "a.rbel")\n(def b (xs) xs)')
        invoke('nested-cycle', 'read', work / 'cycle/a.rbel', success=False)
        (work / 'cycle/link.rbel').symlink_to(work / 'cycle/a.rbel')
        write('cycle/b.rbel', '(import "link.rbel")\n(def b (xs) xs)')
        invoke('symlink-cycle', 'read', work / 'cycle/a.rbel', success=False)

        module = write('literate/helper.rbel.md', literate('helper-block', '(def helper (xs) xs)', '(helper "abc")', '"abc"'))
        md = write('literate/main.rbel.md', literate('main-block', '(import "helper.rbel.md")\n(def main (xs) (helper xs))', '(main "abc")', '"abc"'))
        result = invoke('literate-import-checks-all-examples', 'check', md)
        assert b'checked 2 examples' in result.stderr, result.stderr
        imports_only = write('literate/imports.rbel.md', literate('imports', '(import "helper.rbel.md")'))
        result = invoke('literate-imports-only', 'check', imports_only)
        assert b'checked 1 examples' in result.stderr, result.stderr
        module.write_text(literate('helper-block', '(def helper (xs) xs)', '(helper "abc")', '"wrong"'))
        result = invoke('imported-example-failure', 'check', md, success=False)
        assert b'helper-block-example' in result.stderr and str(module).encode() in result.stderr, result.stderr

        # Literate definition fences concatenate into one reader input. A quote
        # or enclosing list begun in one fence must protect the next fence too.
        for label, first, second in [
            ('quote', "'", '(import "missing.rbel")'),
            ('list', '(def main (xs)', '(import "missing.rbel"))'),
        ]:
            split = write(f'literate/split-{label}.rbel.md',
                          literate('first-block', first) +
                          literate('second-block', second, "(id 'a 'a)", 't'))
            invoke(f'cross-definition-block-{label}', 'read', split)

        # Transitive source protection must precede removal of an existing output.
        source.write_text('(def main (xs) xs)\n(import "dependency")\n')
        protected = write('dependency', '(def dependency (xs) xs)\n')
        original = protected.read_bytes()
        invoke('transitive-output-alias', 'build', source, '-o', protected, success=False)
        assert protected.read_bytes() == original
        alias = work / 'output-alias'
        os.link(protected, alias)
        invoke('transitive-output-hardlink', 'build', source, '-o', alias, success=False)
        assert alias.read_bytes() == original and protected.read_bytes() == original
        alias.unlink()
        alias.symlink_to(protected)
        invoke('transitive-output-symlink', 'build', source, '-o', alias, success=False)
        assert alias.is_symlink() and protected.read_bytes() == original

        unclosed = write('unclosed.md', '```rbel definition=unclosed\n(def main (xs) xs)\n(import "dependency")\n')
        invoke('unclosed-fence-dependency-output-alias', 'build', unclosed, '-o', protected, success=False)
        assert protected.read_bytes() == original
        alias.unlink()
        os.link(protected, alias)
        invoke('unclosed-fence-dependency-output-hardlink', 'build', unclosed, '-o', alias, success=False)
        assert alias.read_bytes() == original and protected.read_bytes() == original

        for index in range(33):
            write(f'chain/{index}.rbel', (f'(import "{index+1}.rbel")\n' if index < 32 else '') + f'(def item-{index} (xs) xs)\n')
        invoke('file-count-bound', 'read', work / 'chain/0.rbel', success=False)
        source.write_text('(def main (xs) xs)\n(import "oversized.rbel")\n')
        write('oversized.rbel', ';' + 'x' * 131072)
        invoke('dependency-size-bound', 'read', source, success=False)
        assert not marker.exists(), marker.read_text() if marker.exists() else ''
        CHECKS.append('no-interpreter-execution')
        evidence = ROOT / 'target/import-verification'
        evidence.mkdir(parents=True, exist_ok=True)
        (evidence / 'results.json').write_text(json.dumps({'checks': CHECKS, 'count': len(CHECKS)}, indent=2) + '\n')
        print(f'{len(CHECKS)} native import checks passed')


if __name__ == '__main__':
    main()
