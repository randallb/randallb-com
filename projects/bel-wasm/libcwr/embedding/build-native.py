#!/usr/bin/env python3
"""Build a local native executable containing the Wasm core and Node runtime.

The generated executable needs neither Node on PATH nor external JS/Wasm files.
This is packaging only: protocol rules remain in the bundled core.wasm.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]


def sha(data):
    return hashlib.sha256(data).hexdigest()


def bundle(sources):
    """Bundle the four fixed first-party modules; no dependency discovery/eval."""
    factories = []
    for name, data in sources.items():
        body = data.decode('utf-8')
        if body.startswith('#!'):
            body = body.partition('\n')[2]
        factories.append(json.dumps(name) + ': function(module, exports, require) {\n' + body + '\n}')
    return ("'use strict';\nconst nativeRequire = require;\nconst factories = {\n" +
            ',\n'.join(factories) + "\n};\nconst cache = Object.create(null);\n" +
            "function load(name) {\n"
            "  if (!Object.hasOwn(factories, name)) throw Error('unbundled module: ' + name);\n"
            "  if (Object.hasOwn(cache, name)) return cache[name].exports;\n"
            "  const module = {exports: {}}; cache[name] = module;\n"
            "  factories[name](module, module.exports, dependency => {\n"
            "    if (dependency.startsWith('node:')) return nativeRequire(dependency);\n"
            "    return load(dependency);\n"
            "  });\n"
            "  return module.exports;\n"
            "}\nload('./native-main.cjs');\n")


def build(args):
    wasm = args.wasm.resolve()
    main = args.main.resolve()
    selected_node = shutil.which(str(args.node))
    if selected_node is None:
        raise ValueError('Node executable not found; building requires Node with --build-sea')
    node = Path(selected_node).resolve()
    output = args.output.resolve()
    manifest = output.with_name(output.name + '.json')
    license_output = output.with_name(output.name + '.node-license.txt')
    license_source = (args.node_license or node.parent.parent / 'LICENSE').resolve()
    inputs = {
        './values.cjs': wasm.parent / 'values.cjs',
        './core.cjs': wasm.parent / 'core.cjs',
        './host.cjs': wasm.parent / 'host.cjs',
        './native-main.cjs': main,
    }
    # Protect the entire standalone package, including adapters, docs and notices.
    # Fixed names also protect incomplete packages; the manifest covers additions.
    package_names = {
        'manifest.json', 'core.wasm', 'core.mjs', 'core.cjs', 'values.mjs', 'values.cjs',
        'host.cjs', 'native-main.cjs', 'workflow.rbel.md', 'readme.md', 'native-build.md',
        'example-context.json', 'example-events.jsonl', 'LICENSE', 'NOTICE.txt', 'ORIGIN.json',
    }
    package_manifest = wasm.parent / 'manifest.json'
    if package_manifest.exists():
        metadata = json.loads(package_manifest.read_text())
        for name in metadata['files']:
            if Path(name).name != name or name in ('.', '..'):
                raise ValueError('invalid package filename in manifest')
            package_names.add(name)
    protected = [wasm, node, license_source, Path(__file__).resolve(), *inputs.values(),
                 *[(wasm.parent / name).resolve() for name in package_names]]
    outputs = [output, manifest, license_output]
    for index, destination in enumerate(outputs):
        if destination.is_relative_to(HERE) or destination.is_relative_to(node.parent):
            raise ValueError('native outputs must be outside source and Node executable directories')
        for source in protected + outputs[:index]:
            if destination == source or (destination.exists() and source.exists() and destination.samefile(source)):
                raise ValueError('native output aliases an input, runtime, or another output')
    output.parent.mkdir(parents=True, exist_ok=True)
    # Every validated new attempt invalidates old success products.
    for destination in outputs:
        destination.unlink(missing_ok=True)
    snapshots = {name: path.read_bytes() for name, path in inputs.items()}
    license_bytes = license_source.read_bytes()
    if not license_bytes or b'Node' not in license_bytes:
        raise ValueError('Node distribution LICENSE with third-party notices is required')
    binary = wasm.read_bytes()
    if binary[:8] != b'\x00asm\x01\x00\x00\x00':
        raise ValueError('expected a version-1 Wasm module')
    env = {key: value for key, value in os.environ.items() if key not in ('NODE_OPTIONS', 'NODE_PATH')}
    help_output = subprocess.check_output([str(node), '--help'], env=env, timeout=30, text=True)
    if '--build-sea' not in help_output:
        raise ValueError('this Node executable lacks --build-sea (requires Node 25.5 or newer)')
    runtime = json.loads(subprocess.check_output(
        [str(node), '-p', 'JSON.stringify({version:process.version,platform:process.platform,arch:process.arch})'],
        env=env, timeout=30, text=True))
    node_hash = sha(node.read_bytes())
    linkage = None
    if sys.platform == 'darwin':
        linkage = subprocess.check_output(['/usr/bin/otool', '-L', str(node)], text=True, timeout=30)
        dependencies = [line.strip().split(' (')[0] for line in linkage.splitlines()[1:] if line.strip()]
        if any(not path.startswith(('/usr/lib/', '/System/Library/')) for path in dependencies):
            raise ValueError('Node has non-system dylib dependencies; use an official portable Node distribution via --node')
    try:
        with tempfile.TemporaryDirectory(prefix='.libcwr-native-', dir=output.parent) as directory:
            work = Path(directory)
            script = work / 'main.cjs'
            script.write_text(bundle(snapshots))
            asset = work / 'core.wasm'
            asset.write_bytes(binary)
            license_asset = work / 'node-license.txt'
            license_asset.write_bytes(license_bytes)
            executable = work / ('libcwr.exe' if sys.platform == 'win32' else 'libcwr')
            config = {
                'main': str(script), 'mainFormat': 'commonjs',
                'output': str(executable), 'assets': {'core.wasm': str(asset), 'node-license.txt': str(license_asset)},
                'useCodeCache': False, 'useSnapshot': False,
                'disableExperimentalSEAWarning': True, 'execArgvExtension': 'none',
            }
            config_path = work / 'sea.json'
            config_path.write_text(json.dumps(config, indent=2) + '\n')
            subprocess.run([str(node), '--build-sea', str(config_path)], check=True, env=env, timeout=120)
            if sys.platform == 'darwin':
                subprocess.run(['/usr/bin/codesign', '--force', '--sign', '-', str(executable)],
                               check=True, timeout=60)
            executable.chmod(0o755)
            if wasm.read_bytes() != binary or any(inputs[name].read_bytes() != data for name, data in snapshots.items()):
                raise ValueError('embedding inputs changed during native packaging')
            if license_source.read_bytes() != license_bytes:
                raise ValueError('Node license changed during native packaging')
            if sha(node.read_bytes()) != node_hash:
                raise ValueError('Node runtime changed during native packaging')
            evidence = {
                'format': 'libcwr-native-sea-v1',
                'runtime': runtime, 'runtime_sha256': node_hash,
                'node_license_sha256': sha(license_bytes),
                'runtime_linkage': linkage,
                'core_wasm_sha256': sha(binary),
                'module_sha256': {name: sha(data) for name, data in snapshots.items()},
                'bundled_main_sha256': sha(script.read_bytes()),
                'executable_sha256': sha(executable.read_bytes()),
                'executable_bytes': executable.stat().st_size,
                'runtime_dependencies': 'platform system libraries; embedded Node and V8/Wasm runtime',
                'signing': 'ad hoc' if sys.platform == 'darwin' else 'not signed by this builder',
                'source': 'https://nodejs.org/api/single-executable-applications.html',
            }
            temporary_manifest = work / 'manifest.json'
            temporary_manifest.write_text(json.dumps(evidence, indent=2) + '\n')
            os.replace(executable, output)
            os.replace(temporary_manifest, manifest)
            os.replace(license_asset, license_output)
    except BaseException:
        for destination in outputs:
            destination.unlink(missing_ok=True)
        raise
    print(f'Native embedded-Wasm executable: {output}')
    print(f'Packaging manifest: {manifest}')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--wasm', type=Path, default=ROOT / 'target/libcwr-embedding/core.wasm')
    parser.add_argument('--main', type=Path, default=HERE / 'native-main.cjs')
    parser.add_argument('--output', type=Path, default=ROOT / 'target/libcwr-embedding/libcwr')
    parser.add_argument('--node', type=Path, default=Path('node'))
    parser.add_argument('--node-license', type=Path, help='Node LICENSE including third-party notices; defaults beside distribution bin/')
    args = parser.parse_args()
    try:
        build(args)
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        print(f'libcwr native packaging: {error}', file=sys.stderr)
        return 1
    return 0


if __name__ == '__main__':
    sys.exit(main())
