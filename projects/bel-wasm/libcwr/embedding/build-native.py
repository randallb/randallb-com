#!/usr/bin/env python3
"""Compile the packaged libcwr Wasm core through WABT wasm2c and a system C compiler."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import sys
import tempfile

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
SOURCES = ['native-values.h', 'native-values.c', 'native-core.c', 'native-host.c', 'native-host.h', 'native-cli.c', 'native-reload.c', 'native-reload.h', 'native-workspace.c', 'native-workspace.h']


def sha(data):
    return hashlib.sha256(data).hexdigest()


def build(args):
    wasm, output = args.wasm.resolve(), args.output.resolve()
    tools, runtime = args.tools.resolve(), args.runtime.resolve()
    license_source = args.runtime_license.resolve()
    selected_cc = shutil.which(str(args.cc))
    if selected_cc is None:
        raise ValueError('C compiler not found')
    cc = Path(selected_cc).resolve()
    manifest_output = output.with_name(output.name + '.json')
    license_output = output.with_name(output.name + '.runtime-license.txt')
    outputs = [output, manifest_output, license_output]
    package_manifest = wasm.parent / 'manifest.json'
    package_names = {'manifest.json', 'core.wasm', 'core.mjs', 'core.cjs', 'values.mjs', 'values.cjs',
                     'host.cjs', 'cli-guide.md', 'chat-guide.md', 'readme.md', 'native-build.md', 'example-context.json',
                     'example-events.jsonl', 'LICENSE', 'NOTICE.txt', 'ORIGIN.json', 'workflow.rbel.md'}
    metadata = json.loads(package_manifest.read_text()) if package_manifest.exists() else None
    maintained = [*HERE.iterdir(), ROOT / 'libcwr/workflow.rbel.md', ROOT / 'literate/wasm-values.mjs',
                  ROOT / 'literate/test.py', *[ROOT / n for n in ['LICENSE', 'NOTICE.txt', 'ORIGIN.json']]]
    if metadata:
        for name in metadata['files']:
            if Path(name).name != name or name in ('.', '..'):
                raise ValueError('invalid package filename')
            package_names.add(name)
        for name in metadata['inputs']:
            path = (ROOT / name).resolve()
            if not path.is_relative_to(ROOT):
                raise ValueError('invalid maintained input path')
            maintained.append(path)
    runtime_files = sorted(p for p in runtime.iterdir() if p.is_file()) if runtime.exists() else []
    tool_files = [tools / 'wasm2c', tools / 'wasm-validate']
    protected = [wasm, cc, license_source, *maintained, *runtime_files, *tool_files,
                 *[(wasm.parent / name).resolve() for name in package_names]]
    for index, destination in enumerate(outputs):
        if ((destination.is_relative_to(ROOT) and not destination.is_relative_to(ROOT / 'target'))
                or destination.is_relative_to(tools) or destination.is_relative_to(runtime)):
            raise ValueError('native outputs must be outside source and tool/runtime directories')
        for source in protected + outputs[:index]:
            if destination == source or (destination.exists() and source.exists() and destination.samefile(source)):
                raise ValueError('native output aliases an input, runtime, or another output')
    output.parent.mkdir(parents=True, exist_ok=True)
    for destination in outputs:
        destination.unlink(missing_ok=True)
    compiler_bytes = cc.read_bytes()
    snapshots = {HERE / name: (HERE / name).read_bytes() for name in SOURCES}
    snapshots.update({path: path.read_bytes() for path in runtime_files + tool_files + [wasm, license_source]})
    snapshots[cc] = compiler_bytes
    if snapshots[wasm][:8] != b'\0asm\x01\0\0\0':
        raise ValueError('expected a version-1 Wasm module')
    if not metadata or metadata.get('format') != 'libcwr-embedding-v1':
        raise ValueError('recognized standalone package manifest required')
    def current_package():
        return (all(sha((ROOT / name).read_bytes()) == digest for name, digest in metadata['inputs'].items())
                and all(sha((wasm.parent / name).read_bytes()) == digest for name, digest in metadata['files'].items())
                and json.loads(package_manifest.read_text()) == metadata)
    if not current_package():
        raise ValueError('standalone package inputs or artifacts are stale')
    license_bytes = snapshots[license_source]
    if b'Apache License' not in license_bytes:
        raise ValueError('WABT license required')
    for tool in tool_files:
        if subprocess.check_output([str(tool), '--version'], text=True, timeout=30).strip() != '1.0.39':
            raise ValueError('requires WABT 1.0.39 matching pinned runtime sources')
    try:
        with tempfile.TemporaryDirectory(prefix='.libcwr-native-', dir=output.parent) as directory:
            work = Path(directory)
            for name in SOURCES:
                (work / name).write_bytes(snapshots[HERE / name])
            runtime_copy = work / 'runtime'
            runtime_copy.mkdir()
            for path in runtime_files:
                (runtime_copy / path.name).write_bytes(snapshots[path])
            binary = work / 'core.wasm'
            binary.write_bytes(snapshots[wasm])
            (work / 'build-info.h').write_text('#define LIBCWR_BUILD_REVISION ' + json.dumps(metadata.get('source_revision', 'unversioned-source')) + '\n#define LIBCWR_CORE_SHA256 ' + json.dumps(sha(snapshots[wasm])) + '\n')
            (work / 'runtime-license.h').write_text('static const char NATIVE_RUNTIME_LICENSE[] = ' + json.dumps(license_bytes.decode()) + ';\n')
            subprocess.run([str(tools / 'wasm-validate'), str(binary)], check=True, timeout=60)
            subprocess.run([str(tools / 'wasm2c'), str(binary), '-n', 'bel', '-o', str(work / 'module.c')], check=True, timeout=120)
            candidate = work / 'libcwr'
            command = [str(cc), '-std=c11', '-O2', '-Wall', '-Wextra', '-Werror',
                       '-Wno-unused-function', '-Wno-unused-parameter', '-Wno-unused-but-set-variable',
                       '-fno-optimize-sibling-calls', '-frounding-math', '-D_POSIX_C_SOURCE=200809L',
                       '-DWASM_RT_USE_MMAP=0', '-DWASM_RT_MEMCHECK_BOUNDS_CHECK=1',
                       '-DWASM_RT_MEMCHECK_GUARD_PAGES=0', '-DWASM_RT_STACK_DEPTH_COUNT=1',
                       '-DWASM_RT_STACK_EXHAUSTION_HANDLER=0', '-DWASM_RT_MAX_CALL_STACK_DEPTH=1024',
                       '-I', str(runtime_copy), '-I', str(work),
                       *[str(work / n) for n in SOURCES if n.endswith('.c')], str(work / 'module.c'),
                       *[str(runtime_copy / n) for n in ['wasm-rt-impl.c', 'wasm-rt-mem-impl.c', 'wasm-rt-exceptions-impl.c']],
                       '-lm', '-pthread', '-o', str(candidate)]
            # Keep strict warnings for authored C; wasm2c's TRAP macro emits a
            # known unused comma-result warning under GCC, confined to generated C.
            source_start = command.index(str(work / 'native-values.c'))
            flags = command[:source_start]
            source_files = [work / n for n in SOURCES if n.endswith('.c')] + [work / 'module.c'] + [
                runtime_copy / n for n in ['wasm-rt-impl.c', 'wasm-rt-mem-impl.c', 'wasm-rt-exceptions-impl.c']]
            objects = []
            for index, source in enumerate(source_files):
                obj = work / f'part-{index}.o'
                warning_flags = ['-Wno-unused-value'] if source.name == 'module.c' else []
                subprocess.run([*flags, *warning_flags, '-c', str(source), '-o', str(obj)], check=True, timeout=180)
                objects.append(str(obj))
            subprocess.run([str(cc), *objects, '-lm', '-pthread', '-o', str(candidate)], check=True, timeout=60)
            if cc.read_bytes() != compiler_bytes:
                raise ValueError('C compiler changed during native compilation')
            linkage = None
            if sys.platform == 'darwin':
                linkage = subprocess.check_output(['/usr/bin/otool', '-L', str(candidate)], text=True, timeout=30)
                deps = [line.strip().split(' (')[0] for line in linkage.splitlines()[1:] if line.strip()]
                if any(not p.startswith(('/usr/lib/', '/System/Library/')) for p in deps):
                    raise ValueError('native executable has non-system dynamic libraries')
            if any(p.read_bytes() != data for p, data in snapshots.items()) or not current_package():
                raise ValueError('native build inputs changed during compilation')
            evidence = {
                'format': 'libcwr-native-wasm2c-v1',
                'source_revision': metadata.get('source_revision', 'unversioned-source'),
                'runtime': {'platform': sys.platform, 'arch': platform.machine(), 'backend': 'WABT wasm2c 1.0.39; system C compiler'},
                'runtime_linkage': linkage,
                'runtime_license_sha256': sha(license_bytes),
                'core_wasm_sha256': sha(snapshots[wasm]),
                'native_source_sha256': {p.name: sha(snapshots[p]) for p in [HERE / n for n in SOURCES]},
                'runtime_source_sha256': {p.name: sha(snapshots[p]) for p in runtime_files},
                'tool_sha256': {p.name: sha(snapshots[p]) for p in tool_files},
                'compiler_sha256': sha(compiler_bytes),
                'build_inputs': {'tools': str(tools), 'runtime': str(runtime), 'runtime_license': str(license_source), 'cc': str(cc)},
                'generated_c_sha256': sha((work / 'module.c').read_bytes()),
                'executable_sha256': sha(candidate.read_bytes()),
                'executable_bytes': candidate.stat().st_size,
                'runtime_dependencies': 'platform system libraries only; no Node or separately installed Wasm engine',
                'signing': 'compiler default; no release signing or notarization',
            }
            (work / 'manifest.json').write_text(json.dumps(evidence, indent=2) + '\n')
            (work / 'license.txt').write_bytes(license_bytes)
            candidate.chmod(0o755)
            os.replace(candidate, output)
            os.replace(work / 'manifest.json', manifest_output)
            os.replace(work / 'license.txt', license_output)
    except BaseException:
        for destination in outputs:
            destination.unlink(missing_ok=True)
        raise
    print(f'Native wasm2c executable: {output} ({evidence["executable_bytes"]} bytes)')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--wasm', type=Path, default=ROOT / 'target/libcwr-embedding/core.wasm')
    parser.add_argument('--output', type=Path, default=ROOT / 'target/libcwr-embedding/libcwr')
    parser.add_argument('--tools', type=Path, default=ROOT / 'target/wabt-native-build')
    parser.add_argument('--runtime', type=Path, default=ROOT / 'target/wabt-native-1.0.39/wasm2c')
    parser.add_argument('--runtime-license', type=Path, default=ROOT / 'target/wabt-native-1.0.39/LICENSE')
    parser.add_argument('--cc', default='/usr/bin/clang' if sys.platform == 'darwin' else 'cc')
    try:
        build(parser.parse_args())
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        print(f'libcwr native compilation: {error}', file=sys.stderr)
        return 1
    return 0


if __name__ == '__main__':
    sys.exit(main())
