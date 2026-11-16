#!/usr/bin/env python3
"""Extract and verify the first literate rbel chapter; standard library only."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
HEADER = re.compile(r"rbel (definition|example)=([a-z][a-z0-9-]*)\Z")
FENCE = re.compile(r" {0,3}(`{3,}|~{3,})([^\r\n]*)\r?\n?\Z")


def extract(text):
    definitions, examples, names = [], [], set()
    lines = text.splitlines(keepends=True)
    index = 0
    while index < len(lines):
        opening = FENCE.fullmatch(lines[index])
        index += 1
        if not opening:
            continue
        marker, info = opening.groups()
        info = info.strip()
        header = HEADER.fullmatch(info)
        if info.startswith('rbel') and not header:
            raise ValueError(f"line {index}: unsupported rbel fence: {info}")
        start = index
        closing = re.compile(r" {0,3}" + re.escape(marker[0]) +
                             "{" + str(len(marker)) + r",}[ \t]*\r?\n?\Z")
        while index < len(lines) and not closing.fullmatch(lines[index]):
            index += 1
        if index == len(lines):
            raise ValueError(f"line {start}: unclosed fence")
        body = ''.join(lines[start:index])
        index += 1
        if not header:
            continue
        kind, name = header.groups()
        if name in names:
            raise ValueError(f"line {start}: duplicate block name {name}")
        names.add(name)
        if not body.strip():
            raise ValueError(f"{name}: empty block")
        if kind == 'definition':
            definitions.append(body)
        else:
            # A whole separator line keeps => in strings/comments unambiguous.
            chunks = re.split(r"(?m)^=>\r?$", body)
            if len(chunks) != 2 or not all(chunk.strip() for chunk in chunks):
                raise ValueError(f"{name}: expected expression, => line, expected value")
            expression, expected = (chunk.strip() for chunk in chunks)
            # Newlines prevent a trailing Bel comment swallowing row syntax.
            examples.append(f"({name}\n{expression}\n{expected}\n)\n")
    if not definitions or not examples:
        raise ValueError('chapter must contain definitions and examples')
    return ''.join(definitions).encode('utf-8'), ''.join(examples), len(examples)


def check_pin(definitions):
    pin = json.loads((ROOT / 'literate/source-pin.json').read_text())
    origin = json.loads((ROOT / 'ORIGIN.json').read_text())
    # Pin original source identity, not mutable notes about later local changes.
    identity = ('repository', 'revision', 'upstream_path', 'published_name')
    if any(pin['upstream'].get(key) != origin.get(key) for key in identity):
        raise ValueError('upstream provenance differs from source pin')
    reference = (ROOT / pin['foundation']['path']).read_bytes()
    for label, content in [('extracted definitions', definitions), ('reference', reference)]:
        if hashlib.sha256(content).hexdigest() != pin['foundation']['sha256']:
            raise ValueError(f'{label}: foundation SHA-256 differs from source pin')
    if definitions != reference:
        raise ValueError('extracted definitions differ from reference bytes')
    fragments = sorted((ROOT / 'experiments/bel-rust-repl/examples/bootstrap').glob('*.bel'))
    complete = b''.join(path.read_bytes() for path in fragments)
    if (len(fragments) != pin['bootstrap']['fragments'] or
            hashlib.sha256(complete).hexdigest() != pin['bootstrap']['sha256']):
        raise ValueError('complete bootstrap differs from source pin')
    return pin


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--chapter', type=Path, default=ROOT / 'literate/foundation.rbel.md')
    parser.add_argument('--extract-only', action='store_true', help='verify bytes and extract without Rust')
    args = parser.parse_args()
    # Read bytes before decoding so universal-newline conversion cannot hide drift.
    definitions, examples, count = extract(args.chapter.read_bytes().decode('utf-8'))
    pin = check_pin(definitions)
    output = ROOT / 'target/literate'
    output.mkdir(parents=True, exist_ok=True)
    source_path, examples_path = output / 'foundation.rbel', output / 'foundation-examples.bel'
    source_path.write_bytes(definitions)
    examples_path.write_text(examples, encoding='utf-8')
    print(f"Pinned foundation and {pin['bootstrap']['fragments']}-fragment bootstrap verified; "
          f'extracted {count} examples.', flush=True)
    if not args.extract_only:
        subprocess.run(['cargo', 'run', '--locked', '--quiet', '-p', 'bel-rust-repl',
                        '--bin', 'rbel-check-examples', '--', str(source_path), str(examples_path)],
                       cwd=ROOT, check=True)


if __name__ == '__main__':
    try:
        main()
    except (ValueError, OSError, subprocess.CalledProcessError) as error:
        print(f'error: {error}', file=sys.stderr)
        sys.exit(1)
