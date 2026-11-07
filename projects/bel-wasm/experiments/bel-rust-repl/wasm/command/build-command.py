#!/usr/bin/env python3
"""Bootstrap the relocatable native rbel command; installed command uses no scripts."""
import argparse
import hashlib
import json
import os
import platform
import re
from pathlib import Path
import shutil
import subprocess
import sys

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[3]
sys.path.insert(0, str(ROOT / 'literate'))
from check import extract


def run(*args):
    subprocess.run(list(map(str, args)), check=True, cwd=ROOT)


def native_core(wat, work, output, tools):
    wasm = work / 'core.wasm'
    run(tools / 'wat2wasm', wat, '-o', wasm)
    run(tools / 'wasm2c', wasm, '-n', 'bel', '-o', work / 'module.c')
    runtime = ROOT / 'target/wabt-native-1.0.39/wasm2c'
    if not runtime.exists():
        run(sys.executable, HERE.parent / 'native/setup-runtime.py')
    run('/usr/bin/clang', '-std=c99', '-O2', '-Wall', '-Wextra', '-Werror',
        '-Wno-unused-function', '-Wno-unused-parameter', '-Wno-unused-but-set-variable',
        '-fno-optimize-sibling-calls', '-frounding-math', '-DWASM_RT_USE_MMAP=0',
        '-DWASM_RT_MEMCHECK_BOUNDS_CHECK=1', '-DWASM_RT_MEMCHECK_GUARD_PAGES=0',
        '-DWASM_RT_STACK_DEPTH_COUNT=1', '-DWASM_RT_STACK_EXHAUSTION_HANDLER=0',
        '-DWASM_RT_MAX_CALL_STACK_DEPTH=4096', '-I', runtime, '-I', work,
        HERE / 'rbel.c', work / 'module.c', runtime / 'wasm-rt-impl.c',
        runtime / 'wasm-rt-mem-impl.c', runtime / 'wasm-rt-exceptions-impl.c',
        '-lm', '-pthread', '-o', output)


def main():
    if sys.platform != 'darwin' or platform.machine() != 'arm64':
        raise ValueError('this command bundle currently targets macOS ARM64')
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--seed', type=Path, help='existing native rbel command instead of Rust/Node bootstrap')
    args = parser.parse_args()
    work, bundle = ROOT / 'target/rbel-command-build', ROOT / 'target/rbel-command'
    work.mkdir(parents=True, exist_ok=True)
    (bundle / 'bin').mkdir(parents=True, exist_ok=True)
    for document in list(bundle.rglob('*.md')) if bundle.exists() else []:
        if document.name != document.name.lower():
            temporary = document.with_name('.rbel-filename-change-' + document.name)
            document.rename(temporary)
            temporary.rename(document.with_name(document.name.lower()))

    tools = ROOT / 'target/wabt-native-build'
    if not all((tools / name).exists() for name in ['wat2wasm', 'wasm2c', 'wasm-validate']):
        run(sys.executable, HERE / 'setup-tools.py')
    reader, examples, _ = extract((ROOT / 'literate/reader.rbel.md').read_bytes().decode())
    numeric = b'\n'.join(extract((ROOT / 'literate' / name).read_text())[0]
                         for name in ['integers.rbel.md', 'floats.rbel.md'])
    source = work / 'core.bel'
    source.write_bytes((HERE / 'frontend.bel').read_bytes() + b'\n' + reader + b'\n' + numeric + b'\n' +
                       (HERE.parent / 'self-compiler.bel').read_bytes())
    wat, wasm = work / 'core.wat', work / 'core.wasm'
    if args.seed:
        # First teach the old reader's compiler about the new primitive ABI.
        # This intermediate core uses the seed's reader and needs no new numeric
        # syntax. It then compiles the new reader and arithmetic from source.
        seed = args.seed.resolve()
        old_source = seed.parent.parent / 'source'
        seed_reader = extract((old_source / 'reader.rbel.md').read_text())[0]
        if b'(def rn-seed-operator ' not in seed_reader:
            seed_reader = seed_reader.replace(b'(def rn-operator ', b'(def rn-seed-operator ')
            seed_reader += b'\n(def rn-operator (xs) (if (rn-text-equal xs "*") t (if (rn-text-equal xs "/") t (rn-seed-operator xs))))\n'
        transition = work / 'transition'
        (transition / 'bin').mkdir(parents=True, exist_ok=True)
        for name in ['share', 'libexec']:
            link = transition / name
            if not link.exists():
                link.symlink_to(seed.parent.parent / name, target_is_directory=True)
        seed_numeric = b''
        if (old_source / 'floats.rbel.md').exists():
            seed_numeric = b'\n'.join(extract((old_source / name).read_text())[0]
                                     for name in ['integers.rbel.md', 'floats.rbel.md'])
        bridge = transition / 'bridge.rbel'
        bridge.write_bytes((old_source / 'frontend.bel').read_bytes() + b'\n' + seed_reader + b'\n' + seed_numeric + b'\n' +
                           (HERE.parent / 'self-compiler.bel').read_bytes())
        bridge_wat = transition / 'core.wat'
        run(seed, 'emit', bridge, '-o', bridge_wat)
        native_core(bridge_wat, transition, transition / 'bin/rbel', tools)
        run(transition / 'bin/rbel', 'emit', source, '-o', wat)
    else:
        data = work / 'core.json'
        data.write_bytes(subprocess.check_output([str(ROOT / 'target/debug/bel-read-json'), str(source)]))
        run(sys.executable, HERE.parent / 'self-compile.py', ROOT / 'target/bel-selfhost/stage1.wasm',
            data, wasm, '--wat2wasm', tools / 'wat2wasm')
        wat = wasm.with_suffix('.wat')
    native_core(wat, work, work / 'rbel', tools)
    runtime = ROOT / 'target/wabt-native-1.0.39/wasm2c'
    libexec, share = bundle / 'libexec/rbel', bundle / 'share/rbel'
    libexec.mkdir(parents=True, exist_ok=True)
    share.mkdir(parents=True, exist_ok=True)
    for name in ['wat2wasm', 'wasm2c', 'wasm-validate']:
        shutil.copy2(tools / name, libexec / name)
    shutil.copytree(runtime, share / 'runtime', dirs_exist_ok=True)
    shutil.copy2(HERE / 'check-main.c', share / 'check-main.c')
    shutil.copy2(HERE.parent / 'native/filter-main.c', share / 'filter-main.c')
    licenses = bundle / 'licenses'
    licenses.mkdir(exist_ok=True)
    shutil.copy2(ROOT / 'target/wabt-source-1.0.39/LICENSE', licenses / 'WABT-LICENSE')
    shutil.copy2(ROOT / 'target/wabt-source-1.0.39/third_party/picosha2/LICENSE', licenses / 'PicoSHA2-LICENSE')
    shutil.copy2(ROOT / 'LICENSE', licenses / 'PROJECT-LICENSE')
    samples = bundle / 'examples'
    samples.mkdir(exist_ok=True)
    for name in ['foundation.rbel.md', 'unix-filter.rbel.md', 'foundation-filter.rbel.md', 'reader.rbel.md', 'integers.rbel.md', 'floats.rbel.md']:
        shutil.copy2(ROOT / 'literate' / name, samples / name)
    for name in ['NOTICE.txt', 'ORIGIN.json', 'NATIVE-ORIGIN.json']:
        shutil.copy2(ROOT / name, bundle / name)
    (bundle / 'readme.md').write_text("""# rbel for macOS ARM64

Keep this directory intact: bin/rbel locates libexec and share beside itself.
Apple's command-line tools are required to build; Rust, Node, and Python are not.

From this directory:

```sh
bin/rbel build examples/unix-filter.rbel.md -o ../hello
printf 'hello, rbel!\\n' | ../hello
bin/rbel check examples/foundation.rbel.md
bin/rbel check examples/reader.rbel.md --include examples/integers.rbel.md --include examples/floats.rbel.md
```

For the foundation-based program:

```sh
bin/rbel build examples/foundation-filter.rbel.md --include examples/foundation.rbel.md --include examples/unix-filter.rbel.md -o ../foundation-uppercase
```

The output must be outside this bundle to protect its resources. Produced commands
are independent byte-list stdin/stdout filters and need only macOS system libraries.
Expected example results are data; failed examples stop publication of an executable.

These are source snapshots, not a new editing home. The maintained project is
randallb.com/Memos/code/randallb-com/projects/bel-wasm. The source directory contains
the reader chapter, compiler, frontend, command adapter, and generated combined
core. `bin/rbel emit source/core.rbel -o ../core.wat` reads and compiles that core
without Rust or Node. Rebuilding the command's native C shell then uses WABT and clang.

Use `bin/rbel --help`. Full commands, syntax, bounds, bootstrap instructions, and
verification evidence are in [the command guide](docs/native-command.md). Original foundation attribution
and project origin are retained in NOTICE.txt, ORIGIN.json, source, and licenses.
""")
    docs = bundle / 'docs'
    docs.mkdir(exist_ok=True)
    for name in ['native-command.md', 'higher-order.md', 'selfhost.md']:
        shutil.copy2(HERE.parent / name, docs / name)
    shutil.copy2(HERE.parent / 'native/readme.md', docs / 'native-filter.md')
    shutil.copy2(ROOT / 'literate/numbers.md', docs / 'numbers.md')
    source_snapshot = bundle / 'source'
    source_snapshot.mkdir(exist_ok=True)
    for source_file in [HERE / 'rbel.c', HERE / 'check-main.c', HERE / 'frontend.bel',
                        HERE.parent / 'self-compiler.bel', ROOT / 'literate/reader.rbel.md',
                        ROOT / 'literate/integers.rbel.md', ROOT / 'literate/floats.rbel.md']:
        shutil.copy2(source_file, source_snapshot / source_file.name)
    shutil.copy2(source, source_snapshot / 'core.rbel')
    # Preserve executable blocks while making exported documentation links portable.
    for path in [*samples.glob('*.md'), *docs.glob('*.md'), source_snapshot / 'reader.rbel.md']:
        body = path.read_text()
        is_literate = path.name.endswith('.rbel.md')
        before = extract(body) if is_literate else None
        def link(match):
            target = match.group(1)
            resolved = (path.parent / target).resolve()
            if '://' in target or (resolved.exists() and resolved.is_relative_to(bundle.resolve())):
                return match.group(0)
            filename = Path(target).name
            candidates = [samples / filename, source_snapshot / filename, docs / filename]
            if target.endswith('native/readme.md'):
                candidates.insert(0, docs / 'native-filter.md')
            destination = next((p for p in candidates if p.exists()), bundle / 'readme.md')
            return '](' + os.path.relpath(destination, path.parent) + ')'
        body = re.sub(r'\]\(([^)]+)\)', link, body)
        if is_literate and extract(body) != before:
            raise ValueError('portable link rewrite changed executable blocks')
        path.write_text(body)
    os.replace(work / 'rbel', bundle / 'bin/rbel')
    manifest = {'platform': 'macOS ARM64', 'bootstrap': 'existing native seed' if args.seed else 'Rust reader and Wasm compiler via Node',
                'reader_sha256': hashlib.sha256(reader).hexdigest(),
                'compiler_sha256': hashlib.sha256((HERE.parent / 'self-compiler.bel').read_bytes()).hexdigest(),
                'wabt_version': '1.0.39', 'wabt_archive_sha256': '8aaafc02afdb8832bd51140b833816512a834601a65348e1f433cece6bbf2db7',
                'wabt_build_adaptations': 'CMake custom-command quoting for spaces; pinned PicoSHA2 submodule; Apple clang',
                'files': {str(p.relative_to(bundle)): hashlib.sha256(p.read_bytes()).hexdigest()
                          for p in sorted(bundle.rglob('*')) if p.is_file() and p.name != 'MANIFEST.json'}}
    (bundle / 'MANIFEST.json').write_text(json.dumps(manifest, indent=2) + '\n')
    print(f'Built native command bundle: {bundle}')


if __name__ == '__main__':
    main()
