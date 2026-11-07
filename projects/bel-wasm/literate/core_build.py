"""Build the source-aware core through its own semantic frontend.

The provisional core is bootstrapped from syntax data. The published core uses
exactly the same located bundle pipeline as a native-seeded rebuild, and must
reproduce its WAT through that pipeline before publication.
"""
import hashlib
import json
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
COMMAND = ROOT / 'experiments/bel-rust-repl/wasm/command'
sys.path.insert(0, str(Path(__file__).resolve().parent))
from check import extract
sys.path.insert(0, str(COMMAND))
from module_loader import load_bundle


def composition():
    paths = [COMMAND / 'frontend.bel', *[ROOT / 'literate' / name for name in
             ['reader.rbel.md', 'integers.rbel.md', 'floats.rbel.md']],
             COMMAND.parent / 'self-compiler.bel', COMMAND.parent / 'core-runtime.bel',
             COMMAND / 'modules.bel']
    snapshots = {p:p.read_bytes() for p in paths}
    pieces = [extract(data.decode('utf-8'))[0] if p.name.endswith('.rbel.md') else data
              for p,data in snapshots.items()]
    return b'\n'.join(pieces), snapshots


def build_core(directory, assembler=None):
    directory = Path(directory).resolve(); directory.mkdir(parents=True, exist_ok=True)
    assembler = Path(assembler or ROOT / 'target/bel-wasm-tools/node_modules/.bin/wat2wasm')
    source = directory / 'core.rbel'
    body, snapshots = composition(); source.write_bytes(body)
    syntax = directory / 'core.json'
    syntax.write_bytes(subprocess.check_output(['node','--stack-size=4096',
        str(COMMAND.parent / 'bootstrap-read-v1.mjs'),
        str(ROOT / 'target/bel-selfhost/legacy-reader.wasm'),str(source)]))
    provisional = directory / 'provisional-core.wasm'
    subprocess.run([sys.executable,str(COMMAND.parent / 'self-compile.py'),
        str(ROOT / 'target/bel-selfhost/stage1.wasm'),str(syntax),str(provisional),
        '--wat2wasm',str(assembler)],check=True)
    request = directory / 'query.json'
    def query(value,mode='value',binary=provisional):
        request.write_text(json.dumps(value,ensure_ascii=False))
        result = subprocess.run(['node',str(COMMAND / 'module-query.mjs'),str(binary),str(request),mode],
                                check=True,capture_output=True,timeout=300)
        return result.stdout if mode == 'wat' else json.loads(result.stdout)
    bundle = load_bundle([source], query)
    wat = directory / 'core.wat'; binary = directory / 'core.wasm'
    wat.write_bytes(query(['compile-bundle',bundle],'wat'))
    subprocess.run([str(assembler),str(wat),'-o',str(binary)],check=True)
    successor = query(['compile-bundle',bundle],'wat',binary)
    if successor != wat.read_bytes():
        raise ValueError('source-aware core does not reproduce canonical WAT')
    if any(p.read_bytes() != data for p,data in snapshots.items()):
        raise ValueError('core source changed during build; no current-source acceptance')
    (directory / 'core-build.json').write_text(json.dumps({
        'abi_version':2,'language_version':2,'canonical_label':'core.rbel',
        'source_sha256':hashlib.sha256(body).hexdigest(),
        'canonical_wat_sha256':hashlib.sha256(wat.read_bytes()).hexdigest(),
        'canonical_wasm_sha256':hashlib.sha256(binary.read_bytes()).hexdigest(),
        'source_aware_reproduction':True,
        'components':{str(p.relative_to(ROOT)):hashlib.sha256(data).hexdigest() for p,data in snapshots.items()},
    },indent=2)+'\n')
    return wat,binary,source
