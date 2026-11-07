"""Snapshot source files and follow imports returned by the rbel scan-unit frontend.

This module owns I/O, canonical file identity and literate fence extraction only.
The supplied query(request) callback speaks ordinary rbel value transport: Python
strings denote symbols; character lists are [{"char": scalar}, ...]. No imports,
namespace resolution or rbel expressions are parsed or rewritten here.
"""
from dataclasses import dataclass
import os
from pathlib import Path
import re

MAX_FILES = 32
MAX_SOURCE = 512 * 1024
MAX_EXAMPLES = 2048
FENCE = re.compile(r' {0,3}(`{3,}|~{3,})([^\r\n]*)\r?\n?\Z')
HEADER = re.compile(r'rbel (definition|example)=([a-z][a-z0-9-]*)\Z')


def chars(text):
    if any(0xD800 <= ord(c) <= 0xDFFF for c in text):
        raise ValueError('source contains a surrogate scalar')
    return [{'char': c} for c in text]


def text(value):
    if not isinstance(value, list) or any(not isinstance(c, dict) or set(c) != {'char'} or
            not isinstance(c['char'], str) or len(c['char']) != 1 for c in value):
        raise ValueError('scan-unit returned invalid character-list metadata')
    return ''.join(c['char'] for c in value)


@dataclass
class Snapshot:
    identity: tuple
    path: Path
    unit: list
    examples: list


def extract_source(path, label, source):
    """Return located complete source blocks and example source rows."""
    if path.suffix != '.md':
        return [[chars('1'), chars(source)]], []
    pieces = source.split('\n')
    lines = [piece + '\n' for piece in pieces[:-1]] + ([pieces[-1]] if pieces[-1] else [])
    blocks, examples, names = [], [], set()
    index = 0
    while index < len(lines):
        opening = FENCE.fullmatch(lines[index])
        index += 1
        if not opening:
            continue
        marker, info = opening.groups()
        info = info.strip(' \t\r\n\v\f')
        header = HEADER.fullmatch(info)
        if info.startswith('rbel') and not header:
            raise ValueError(f'{label}:{index}: unsupported rbel fence')
        start = index
        close = re.compile(r' {0,3}' + re.escape(marker[0]) + '{' + str(len(marker)) + r',}[ \t]*\r?\n?\Z')
        while index < len(lines) and not close.fullmatch(lines[index]):
            index += 1
        if index == len(lines):
            raise ValueError(f'{label}:{start}: unclosed fence')
        body = ''.join(lines[start:index])
        index += 1
        if not header:
            continue
        kind, name = header.groups()
        if name in names:
            raise ValueError(f'{label}:{start}: duplicate block {name}')
        names.add(name)
        if not body.strip(' \t\r\n\v\f'):
            raise ValueError(f'{label}:{start}: empty block {name}')
        if kind == 'definition':
            blocks.append([chars(str(start + 1)), chars(body)])
        else:
            separators = [i for i in range(start, index - 1) if lines[i].rstrip('\r\n') == '=>']
            if len(separators) != 1:
                raise ValueError(f'{label}:{start}: malformed example {name}')
            split = separators[0]
            expression, expected = ''.join(lines[start:split]), ''.join(lines[split+1:index-1])
            if not expression.strip(' \t\r\n\v\f') or not expected.strip(' \t\r\n\v\f'):
                raise ValueError(f'{label}:{start}: malformed example {name}')
            examples.append([chars(str(start + 1)), name, chars(expression), chars(expected)])
    if not blocks:
        raise ValueError(f'{label}: literate source requires a definition block')
    return blocks, examples


def load_bundle(paths, query):
    """Return source-bundle-v1; query is the ABI-aware rbel request callback."""
    paths = list(paths)
    if not paths:
        raise ValueError('a source root is required')
    base = Path(paths[0]).resolve().parent
    snapshots, identities, active, edges, edge_keys = [], {}, set(), [], set()
    total = 0

    def load(path, importer=None, authored=None, line=None):
        nonlocal total
        try:
            canonical = Path(path).resolve(strict=True)
            stat = canonical.stat()
            if not canonical.is_file():
                raise ValueError('source must be a regular file')
        except (OSError, ValueError) as error:
            where = f'{importer}:{line}: import {authored!r}' if importer is not None else str(path)
            raise ValueError(f'{where}: {error}') from error
        identity = stat.st_dev, stat.st_ino
        if identity in active:
            raise ValueError(f'{importer}:{line}: import cycle through {authored!r}')
        if identity in identities:
            return identities[identity]
        if len(snapshots) >= MAX_FILES:
            raise ValueError('too many source files (maximum 32)')
        raw = canonical.read_bytes()
        total += len(raw)
        if total > MAX_SOURCE:
            raise ValueError('combined source exceeds 512 KiB')
        try:
            source = raw.decode('utf-8', errors='strict')
        except UnicodeError as error:
            raise ValueError(f'{canonical.name}: invalid UTF-8 source') from error
        if any(byte < 32 and byte not in (9, 10, 13) for byte in raw):
            raise ValueError(f'{canonical.name}: unsupported source control byte; use a scalar escape')
        label = Path(os.path.relpath(canonical, base)).as_posix()
        blocks, examples = extract_source(canonical, label, source)
        unit_id = f'unit-{len(snapshots)}'
        unit = [unit_id, chars(label), blocks]
        snapshot = Snapshot(identity, canonical, unit, examples)
        snapshots.append(snapshot)
        identities[identity] = unit_id
        active.add(identity)
        imports = query(['scan-unit', unit])
        if imports is None:
            imports = []
        if not isinstance(imports, list) or len(imports) > MAX_FILES:
            raise ValueError(f'{label}: invalid or excessive imports from scan-unit')
        for declaration in imports:
            if not isinstance(declaration, list) or len(declaration) != 3:
                raise ValueError(f'{label}: invalid import declaration from scan-unit')
            imported, alias, form_line = declaration
            imported, form_line = text(imported), text(form_line)
            if (not imported or imported.startswith(('/', '~')) or ':' in imported or
                    any(ord(c) < 32 or ord(c) == 127 for c in imported)):
                raise ValueError(f'{label}:{form_line}: import requires a relative path')
            target = load(canonical.parent / imported, label, imported, form_line)
            key = unit_id, imported
            if key not in edge_keys:
                edges.append([unit_id, chars(imported), target])
                edge_keys.add(key)
        active.remove(identity)
        return unit_id

    roots = [load(path) for path in paths]
    examples = [[snapshot.unit[0], *example] for snapshot in snapshots for example in snapshot.examples]
    if len(examples) > MAX_EXAMPLES:
        raise ValueError('too many executable examples')
    return ['source-bundle-v1', roots, [s.unit for s in snapshots], edges, examples]
