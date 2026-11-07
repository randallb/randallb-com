#!/usr/bin/env python3
"""Exercise the native command's actual rbel lint rules and file transport."""
from pathlib import Path
import json
import os
import shutil
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[4]
COMMAND = ROOT / 'target/rbel-command/bin/rbel'
checks = []


def main():
    with tempfile.TemporaryDirectory(prefix='rbel-lint-') as directory:
        work = Path(directory)
        # Relocate the bundle and trap scripting runtimes on PATH.
        bundle = work / 'bundle with spaces'
        shutil.copytree(COMMAND.parents[1], bundle)
        command = bundle / 'bin/rbel'
        traps = work / 'traps'
        traps.mkdir()
        for name in ['node', 'python', 'python3', 'rustc', 'cargo', 'deno']:
            script = traps / name
            script.write_text('#!/bin/sh\nexit 99\n')
            script.chmod(0o755)
        env = {**os.environ, 'PATH': str(traps) + ':/usr/bin:/bin'}

        def case(name, data, args=(), expected='', code=0, suffix='.md'):
            path = work / ('input $() `literal`' + suffix)
            path.write_bytes(data)
            result = subprocess.run([command, 'lint', path, *args], env=env, capture_output=True, text=True)
            output = result.stdout.replace(str(path), 'FILE')
            assert result.returncode == code, (name, result.returncode, result.stderr)
            if code == 0:
                assert output == expected, (name, output, expected)
                assert not result.stderr, (name, result.stderr)
            else:
                assert not result.stdout and result.stderr, (name, result.stdout, result.stderr)
            checks.append(name)

        case('empty', b'')
        case('exact-file', b'x\n' * 500)
        case('over-file', b'x\n' * 501, expected='FILE:1: warning: file has 501 lines (limit 500)\n')
        case('unterminated-line', b'x\n' * 500 + b'x', expected='FILE:1: warning: file has 501 lines (limit 500)\n')
        case('crlf', b'x\r\n' * 501, expected='FILE:1: warning: file has 501 lines (limit 500)\n')
        case('blank-comment', b'\n;comment\n', ('--file-limit', '1'), 'FILE:1: warning: file has 2 lines (limit 1)\n')
        case('exact-block', b'```rbel definition=a\n' + b'x\n' * 80 + b'```\n')
        case('over-block', b'```rbel definition=a\n' + b'x\n' * 81 + b'```\n', expected='FILE:1: warning: rbel definition=a block has 81 lines (limit 80)\n')
        case('fence-variants', b'title\r\n   ~~~~ rbel test=demo\r\n\r\n;comment\r\n   ~~~~~\r\n', ('--block-limit', '1'), 'FILE:2: warning: rbel test=demo block has 2 lines (limit 1)\n')
        case('combined', b'```rbel\na\nb\n```\n', ('--block-limit', '1', '--file-limit', '3'), 'FILE:1: warning: file has 4 lines (limit 3)\nFILE:1: warning: rbel block has 2 lines (limit 1)\n')
        case('multiple-blocks', b'```rbel\na\n```\ntext\n~~~rbel example=b\nb\n~~~', ('--block-limit', '0'), 'FILE:1: warning: rbel block has 1 lines (limit 0)\nFILE:5: warning: rbel example=b block has 1 lines (limit 0)\n')
        case('ordinary-fence', b'```text\n~~~rbel\na\n~~~\n```\n', ('--block-limit', '0'))
        case('similar-language', b'```rbelish\nx\n```\n', ('--block-limit', '0'))
        case('empty-block', b'```rbel\n```\n', ('--block-limit', '0'))
        case('closing-width', b'````rbel\n```\n````\n', ('--block-limit', '0'), 'FILE:1: warning: rbel block has 1 lines (limit 0)\n')
        case('four-space-indent', b'    ```rbel\nx\n    ```\n', ('--block-limit', '0'))
        case('raw-file', b'```rbel\nx\n```\n', ('--block-limit', '0'), suffix='.rbel')
        case('unicode', 'héllo 😀\n'.encode())
        case('raw-large-count', b'\n' * 131072, ('--file-limit', '131071'), 'FILE:1: warning: file has 131072 lines (limit 131071)\n')
        case('many-blocks', b'```rbel\nx\n```\n' * 2048, ('--file-limit', '10000'))
        expected = ''.join(f'FILE:{i*3+1}: warning: rbel block has 1 lines (limit 0)\n' for i in range(2048))
        case('many-findings', b'```rbel\nx\n```\n' * 2048, ('--file-limit', '10000', '--block-limit', '0'), expected)
        case('too-many-blocks', b'```rbel\nx\n```\n' * 2049, code=1)
        for args in [('--file-limit', '-1'), ('--block-limit', 'a'), ('--file-limit', ''), ('--file-limit', '131073'), ('--file-limit',), ('--file-limit','1','--file-limit','2'), ('--unknown','1'), ('extra-file',)]:
            case('invalid-options-' + repr(args), b'', args, code=2)
        case('unclosed-rbel', b'```rbel\nx\n', code=1)
        case('unclosed-other', b'```text\nx\n', code=1)
        case('invalid-utf8', b'\xff', code=1)
        case('nul', b'\0', code=1)
        case('too-large', b'x' * 131073, code=1)
        for name, args, code in [('missing-file', ['lint', work / 'missing.md'], 1), ('directory', ['lint', work], 1), ('missing-source', ['lint'], 2)]:
            r = subprocess.run([command, *args], capture_output=True, text=True, env=env)
            assert r.returncode == code and not r.stdout and r.stderr, (name, r)
            checks.append(name)
        # Demonstrate the complete user workflow without changing its source.
        path = ROOT / 'libcwr/workflow.rbel.md'
        r = subprocess.run([command, 'lint', path], capture_output=True, text=True, env=env, check=True)
        expected = f'{path}:1: warning: file has {len(path.read_bytes().splitlines())} lines (limit 500)\n'
        lines = path.read_text().splitlines()
        for index, line in enumerate(lines):
            if line.startswith('```rbel'):
                end = lines.index('```', index + 1)
                count = end - index - 1
                if count > 80:
                    label = line[3:]
                    expected += f'{path}:{index+1}: warning: {label} block has {count} lines (limit 80)\n'
        assert r.stdout == expected, (r.stdout, expected)
        checks.append('actual-workflow-file')
        evidence = ROOT / 'target/lint-verification'
        evidence.mkdir(exist_ok=True)
        (evidence / 'workflow.txt').write_text('$ rbel lint libcwr/workflow.rbel.md\n' + r.stdout)
        (evidence / 'results.json').write_text(json.dumps({'checks': checks, 'count': len(checks)}, indent=2) + '\n')
        print(f'{len(checks)} native lint checks passed')


if __name__ == '__main__':
    main()
