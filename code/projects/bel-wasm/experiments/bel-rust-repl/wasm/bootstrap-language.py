#!/usr/bin/env python3
"""Cross the explicit ABI1->ABI2 source bootstrap, then build successor stages.

Python transports syntax and substitutes the pinned bootstrap character emitter;
all parsing/compilation decisions execute in the source compilers. No host rbel
interpreter or arithmetic implementation is added.
"""
import hashlib
import json
import re
from pathlib import Path
import subprocess
import sys

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
OUT = ROOT / 'target/bel-selfhost'
SEED = HERE / 'bootstrap-compiler-v1.bel'
ASSEMBLER = ROOT / 'target/bel-wasm-tools/node_modules/.bin/wat2wasm'


def run(*args, output=None):
    result = subprocess.run(list(map(str,args)), check=True, capture_output=True)
    if output is not None:
        output.write_bytes(result.stdout)
    return result.stdout


def read(path):
    return json.loads(run(ROOT / 'target/debug/bel-read-json',path))


def source_bytes():
    return b'\n'.join((HERE / name).read_bytes()
                      for name in ['self-compiler.bel','core-runtime.bel'])


def bootstrap():
    OUT.mkdir(parents=True,exist_ok=True)
    final_source = OUT / 'compiler-source.bel'
    final_source.write_bytes(source_bytes())
    seed_ast = read(SEED)
    seed_wat = OUT / 'legacy-seed.wat'
    run(ROOT / 'target/debug/bel-eval-text','--max-text-bytes','8388608',
        '--max-eval-depth','4096','--call','sc-compile',SEED,SEED,output=seed_wat)
    seed_binary = seed_wat.with_suffix('.wasm')
    run(ASSEMBLER,seed_wat,'-o',seed_binary)
    reader_source = HERE / 'bootstrap-reader-v1.bel'
    reader_syntax = OUT / 'legacy-reader.json'
    reader_syntax.write_text(json.dumps(read(reader_source)))
    reader_wat = OUT / 'legacy-reader.wat'
    run('node',HERE / 'bootstrap-wat-v1.mjs',seed_binary,reader_syntax,output=reader_wat)
    reader_binary = reader_wat.with_suffix('.wasm')
    run(ASSEMBLER,reader_wat,'-o',reader_binary)
    ast = json.loads(run('node',HERE / 'bootstrap-read-v1.mjs',reader_binary,final_source))
    legacy_char = next(form for form in seed_ast if form[:2] == ['def','sc-char'])
    cases = []
    node = legacy_char[3]
    while isinstance(node,list) and node and node[0] == 'if':
        condition, text, node = node[1:]
        character = condition[2]
        spelling = ''.join(c['char'] for c in text)
        tagged = int(re.fullmatch(r'\(i32.const (\d+)\)',spelling)[1])
        scalar = (tagged - 2) // 8
        if scalar != ord(character['char']):
            raise ValueError('legacy character seed table inconsistency')
        cases.append((character, str(scalar)))
    fallback = ['err',['quote','unsupported-bootstrap-character']]
    for character, digits in reversed(cases):
        fallback = ['if',['id','c',character],[{'char':c} for c in digits],fallback]
    bridge_helper = ['def','sc-character-digits',['c'],fallback]
    bridge_ast = [bridge_helper if form[:2] == ['def','sc-character-digits'] else form for form in ast]
    if sum(form[:2] == ['def','sc-character-digits'] for form in ast) != 1:
        raise ValueError('missing character emitter for explicit bootstrap bridge')
    syntax = OUT / 'compiler.ast.json'; syntax.write_text(json.dumps(ast))
    bridge_syntax = OUT / 'bridge.ast.json'; bridge_syntax.write_text(json.dumps(bridge_ast))
    bridge_wat = OUT / 'bridge.wat'; bridge_binary = bridge_wat.with_suffix('.wasm')
    run('node',HERE / 'bootstrap-wat-v1.mjs',seed_binary,bridge_syntax,output=bridge_wat)
    run(ASSEMBLER,bridge_wat,'-o',bridge_binary)
    first_wat = OUT / 'first-abi2.wat'; first_binary = first_wat.with_suffix('.wasm')
    run('node',HERE / 'bootstrap-wat-v1.mjs',bridge_binary,syntax,output=first_wat)
    run(ASSEMBLER,first_wat,'-o',first_binary)
    stage = OUT / 'stage0.wat'
    run('node',HERE / 'self-wat.mjs',first_binary,syntax,output=stage)
    run(ASSEMBLER,stage,'-o',stage.with_suffix('.wasm'))
    files = [SEED, reader_source,reader_syntax,reader_wat,reader_binary, final_source, syntax, bridge_syntax, seed_wat, seed_binary,
             bridge_wat,bridge_binary,first_wat,first_binary,stage,stage.with_suffix('.wasm')]
    manifest = {'legacy_seed_revision':'d38c27fb565961e04c3828bedb61cd1f2421d864',
                'bridge':'final compiler AST with only sc-character-digits replaced by ASCII scalar data derived from pinned ABI1 sc-char table',
                'files':{str(path):hashlib.sha256(path.read_bytes()).hexdigest() for path in files}}
    (OUT / 'abi-bootstrap.json').write_text(json.dumps(manifest,indent=2)+'\n')
    print('PASS: explicit pinned ABI1 seed -> ASCII bridge -> full Unicode ABI2 compiler')
    return ast


if __name__ == '__main__':
    try:
        bootstrap()
    except (OSError,ValueError,subprocess.SubprocessError) as error:
        print(f'language bootstrap failed: {error}',file=sys.stderr)
        if isinstance(error,subprocess.CalledProcessError):
            print(error.stderr.decode(errors='replace')[-8000:],file=sys.stderr)
        sys.exit(1)
