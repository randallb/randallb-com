#!/usr/bin/env python3
"""Compile adjacent rbel examples/tests and run their continuations on Wasm."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]
FENCE = re.compile(r' {0,3}(`{3,}|~{3,})([^\r\n]*)\r?\n?\Z')
HEADER = re.compile(r'rbel (definition|example|test)=([a-z][a-z0-9-]*)\Z')


def blocks(path, data=None):
    if data is None:
        data = path.read_bytes()
    if len(data) > 524288:
        raise ValueError(f'{path}: source exceeds 512 KiB')
    lines = data.decode('utf-8').splitlines(keepends=True)
    index, result = 0, []
    while index < len(lines):
        opening = FENCE.fullmatch(lines[index])
        index += 1
        if not opening:
            continue
        marker, info = opening.groups()
        info = info.strip()
        header = HEADER.fullmatch(info)
        if info.startswith('rbel') and not header:
            raise ValueError(f'{path}:{index}: unsupported rbel fence {info}')
        start = index
        close = re.compile(r' {0,3}' + re.escape(marker[0]) +
                           '{' + str(len(marker)) + r',}[ \t]*\r?\n?\Z')
        while index < len(lines) and not close.fullmatch(lines[index]):
            index += 1
        if index == len(lines):
            raise ValueError(f'{path}:{start}: unclosed fence')
        body = ''.join(lines[start:index])
        index += 1
        if header:
            kind, name = header.groups()
            if not body.strip():
                raise ValueError(f'{path}:{start}: {name}: empty block')
            result.append(dict(kind=kind, name=name, line=start, path=str(path), body=body))
    return result


def prepare(paths, entry, snapshots=None):
    items, names = [], set()
    for path in paths:
        for item in blocks(path, None if snapshots is None else snapshots[path]):
            if item['name'] in names:
                raise ValueError(f"{item['path']}:{item['line']}: duplicate block {item['name']}")
            names.add(item['name'])
            items.append(item)
    definitions = [item['body'] for item in items if item['kind'] == 'definition']
    cases = []
    for item in items:
        if item['kind'] == 'definition':
            continue
        expression = item['body']
        if item['kind'] == 'example':
            chunks = re.split(r'(?m)^=>\r?$', expression)
            if len(chunks) != 2 or not all(c.strip() for c in chunks):
                raise ValueError(f"{item['path']}:{item['line']}: {item['name']}: malformed example")
            expression = (f"(rt-assert '{item['name']}\n{chunks[0]}\n'{chunks[1]}\n"
                          '(fn (ignored) (rt-done)))')
        cases.append(dict(item, expression=expression))
    if not definitions or not cases:
        raise ValueError('source requires definitions and examples or tests')
    if not re.fullmatch(r'[a-z][a-z0-9-]*', entry):
        raise ValueError('entry must be a compiled function name')
    selection = "(err 'unknown-test)"
    for case in reversed(cases):
        selection = f"(if (id selector '{case['name']})\n{case['expression']}\n{selection})"
    dispatcher = f'''(def rt-dispatch (request)
  (if (id (car request) 'call) ({entry} (car (cdr request)))
    (id (car request) 'test) (rt-select (car (cdr request)))
    (id (car request) 'resume) ((car (cdr request)) (car (cdr (cdr request))))
    (err 'bad-test-driver-request)))
(def rt-select (selector) {selection})
(def rt-identity (value) value)
'''
    runtime = (ROOT / 'literate/test-runtime.bel').read_text()
    return dispatcher + runtime + '\n' + '\n'.join(definitions), cases, definitions


def run(args):
    paths = [p.resolve() for p in [args.source, *args.include]]
    if len(paths) > 32:
        raise ValueError('at most 32 source files')
    protected = paths + [args.rbel.resolve(), ROOT / 'literate/test-runtime.bel',
                         ROOT / 'literate/test-runner.mjs', ROOT / 'literate/wasm-values.mjs', Path(__file__).resolve()]
    if args.host:
        protected.append(args.host.resolve())
    bundle = args.rbel.resolve().parent.parent
    # The installed compiler's resources are inputs too. Protect the complete
    # bundle, including aliases that live outside it, before clearing outputs.
    protected.extend(p for p in bundle.rglob('*') if p.is_file())
    outputs = [p.resolve() for p in (args.docs, args.report) if p]
    def aliases(a, b):
        return a == b or (a.exists() and b.exists() and a.samefile(b))
    for index, output in enumerate(outputs):
        if output.is_relative_to(bundle) or any(aliases(output, source) for source in protected + outputs[:index]):
            raise ValueError('output must not overwrite an input, tool, or another output')
    # Clear previous success evidence only after validating every output alias.
    for output in outputs:
        output.unlink(missing_ok=True)
    snapshots = {p: p.read_bytes() for p in paths}
    source, cases, definitions = prepare(paths, args.entry, snapshots)
    if len(source.encode('utf-8')) > 524288:
        raise ValueError('combined compiler input exceeds 512 KiB')
    rbel = args.rbel.resolve()
    assembler = rbel.parent.parent / 'libexec/rbel/wat2wasm'
    # Every run uses a new private output directory. No prior success can be reused.
    with tempfile.TemporaryDirectory(prefix='rbel-wasm-tests-') as directory:
        work = Path(directory)
        raw = work / 'tests.rbel'
        raw.write_text(source)
        # The existing rbel reader owns source parsing. Check reserved definitions
        # on its syntax data, never with a host expression evaluator.
        def proper(value):
            values = []
            while value is not None:
                if not isinstance(value, dict) or set(value) != {'pair'}:
                    raise ValueError('definition must be a proper list')
                head, value = value['pair']
                values.append(head)
            return values
        seen = set()
        # Read each authored block separately to respect the command's bounded
        # debug JSON renderer. The compiler reads the entire combined module.
        for body in definitions:
            part = work / 'definition.rbel'
            part.write_text(body)
            parsed = subprocess.run([str(rbel), 'read', str(part)], capture_output=True, text=True, timeout=60)
            if parsed.returncode:
                raise ValueError(parsed.stderr.strip())
            for form in proper(json.loads(parsed.stdout)):
                fields = proper(form)
                if len(fields) < 4 or fields[0] != 'def' or not isinstance(fields[1], str):
                    raise ValueError('only named definitions are supported')
                name = fields[1]
                if name in seen or name.startswith('rt-'):
                    raise ValueError(f'reserved or duplicate definition: {name}')
                seen.add(name)
        for case in cases:
            part = work / 'test-expression.rbel'
            part.write_text(case['expression'])
            parsed = subprocess.run([str(rbel), 'read', str(part)], capture_output=True, text=True, timeout=60)
            if parsed.returncode and 'reader output traversal limit' in parsed.stderr:
                # Parsing succeeded; only the optional bounded JSON rendering failed.
                # The compiler still validates the full expression in its module.
                continue
            if parsed.returncode or len(proper(json.loads(parsed.stdout))) != 1:
                raise ValueError(f"{case['path']}:{case['line']}: {case['name']}: malformed or bounded test expression; {parsed.stderr.strip()}")
        wat, wasm = work / 'tests.wat', work / 'tests.wasm'
        subprocess.run([str(rbel), 'emit', str(raw), '-o', str(wat)], check=True, timeout=120)
        subprocess.run([str(assembler), str(wat), '-o', str(wasm)], check=True, timeout=60)
        manifest = work / 'tests.json'
        manifest.write_text(json.dumps({
            'cases': [{k: c[k] for k in ('name', 'line', 'path', 'kind')} for c in cases],
            'provenance': {'sources': {str(p): hashlib.sha256(snapshots[p]).hexdigest() for p in paths},
                'compiler_sha256': hashlib.sha256(rbel.read_bytes()).hexdigest(),
                'wasm_sha256': hashlib.sha256(wasm.read_bytes()).hexdigest(),
                'host_sha256': hashlib.sha256(args.host.read_bytes()).hexdigest() if args.host else None}
        }))
        command = ['node', str(ROOT / 'literate/test-runner.mjs'), str(wasm), str(manifest)]
        if args.host:
            command.extend(['--host', str(args.host.resolve())])
        if args.report:
            command.extend(['--report', str(args.report.resolve())])
        if args.verbose:
            command.append('--verbose')
        result = subprocess.run(command, timeout=args.timeout)
        if result.returncode:
            raise ValueError(f'Wasm tests failed (exit {result.returncode})')
    try:
        unchanged = all(p.read_bytes() == snapshots[p] for p in paths)
    except OSError:
        unchanged = False
    if not unchanged:
        for output in outputs:
            output.unlink(missing_ok=True)
        raise ValueError('source changed during verification; no success evidence retained')
    if args.docs:
        # Generated docs are the exact authored chapter, with a source hash.
        output = args.docs.resolve()
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_text('<!-- Generated from maintained literate sources; do not edit. -->\n' +
            '\n'.join(f'<!-- {p.name}: sha256 {hashlib.sha256(snapshots[p]).hexdigest()} -->\n' +
                      snapshots[p].decode('utf-8') for p in paths))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('source', type=Path)
    parser.add_argument('--include', type=Path, action='append', default=[])
    parser.add_argument('--entry', default='rt-identity')
    parser.add_argument('--rbel', type=Path, default=ROOT / 'target/rbel-command/bin/rbel')
    parser.add_argument('--host', type=Path, help='explicit trusted host mechanism module')
    parser.add_argument('--report', type=Path)
    parser.add_argument('--docs', type=Path)
    parser.add_argument('--timeout', type=int, default=120)
    parser.add_argument('--verbose', action='store_true', help='print named assertion outcomes')
    args = parser.parse_args()
    try:
        run(args)
        return 0
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        print(f'{args.source}: test failure: {error}', file=sys.stderr)
        return 1


if __name__ == '__main__':
    sys.exit(main())
