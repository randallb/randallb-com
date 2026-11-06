#!/usr/bin/env python3
"""Build checksum-pinned native WABT tools; bootstrap tooling only."""
import hashlib
import io
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tarfile
import urllib.request

ROOT = Path(__file__).resolve().parents[4]
VERSION = '1.0.39'
SHA = '8aaafc02afdb8832bd51140b833816512a834601a65348e1f433cece6bbf2db7'
PICO = '27fcf6979298949e8a462e16d09a0351c18fcaf2'
PICO_FILES = {'picosha2.h': '8f183eaae529cd9d6a3d4843c7559e2a3e3d68b6caaa223e7c24c3c899b3d988',
              'LICENSE': '6c30eb1f37554ec4199cb82a8c86d9e7a852da78a757832bebb03ac9ddc44ff1'}


def download(url, digest):
    blob = urllib.request.urlopen(url, timeout=60).read()
    if hashlib.sha256(blob).hexdigest() != digest:
        raise ValueError(f'checksum mismatch: {url}')
    return blob


def main():
    if sys.platform != 'darwin':
        raise ValueError('this command bundle currently targets macOS')
    source, build = ROOT / 'target/wabt-source-1.0.39', ROOT / 'target/wabt-native-build'
    if not (source / 'CMakeLists.txt').exists():
        blob = download(f'https://codeload.github.com/WebAssembly/wabt/tar.gz/refs/tags/{VERSION}', SHA)
        with tarfile.open(fileobj=io.BytesIO(blob), mode='r:gz') as archive:
            for member in archive:
                if not member.isfile():
                    continue
                parts = Path(member.name).parts[1:]
                if not parts or '..' in parts:
                    raise ValueError('invalid archive path')
                path = source.joinpath(*parts)
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(archive.extractfile(member).read())
    for name, digest in PICO_FILES.items():
        path = source / 'third_party/picosha2' / name
        if not path.exists() or hashlib.sha256(path.read_bytes()).hexdigest() != digest:
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(download(f'https://raw.githubusercontent.com/okdshin/PicoSHA2/{PICO}/{name}', digest))
    # Fix upstream custom-command quoting for the connected Drive path's spaces.
    cmakefile = source / 'CMakeLists.txt'
    text = cmakefile.read_text()
    text = re.sub(r'COMMAND \$\{CMAKE_COMMAND\} -D out="([^"]+)" -D in="([^"]+)" -D symbol="([^"]+)" -P \$\{TEMPLATE_CMAKE\}',
                  r'COMMAND ${CMAKE_COMMAND} "-Dout=\1" "-Din=\2" "-Dsymbol=\3" -P "${TEMPLATE_CMAKE}"', text)
    if 'VERBATIM\n  DEPENDS ${WABT_SOURCE_DIR}/src/template/wasm2c.top.h' not in text:
        text = text.replace('  DEPENDS ${WABT_SOURCE_DIR}/src/template/wasm2c.top.h',
                            '  VERBATIM\n  DEPENDS ${WABT_SOURCE_DIR}/src/template/wasm2c.top.h')
    cmakefile.write_text(text)
    cmake = shutil.which('cmake')
    if not cmake:
        cmake = ROOT / 'target/cmake-tools/cmake/data/bin/cmake'
        if not cmake.exists():
            subprocess.run([sys.executable, '-m', 'pip', 'install', '--target',
                            str(ROOT / 'target/cmake-tools'), 'cmake==3.31.6'], check=True)
    subprocess.run([str(cmake), '-S', str(source), '-B', str(build), '-G', 'Unix Makefiles',
                    '-DCMAKE_BUILD_TYPE=Release', '-DCMAKE_C_COMPILER=/usr/bin/clang',
                    '-DCMAKE_CXX_COMPILER=/usr/bin/clang++', '-DCMAKE_MAKE_PROGRAM=/usr/bin/make',
                    '-DBUILD_TESTS=OFF', '-DBUILD_LIBWASM=OFF', '-DBUILD_TOOLS=ON',
                    '-DUSE_INTERNAL_SHA256=ON'], check=True)
    subprocess.run([str(cmake), '--build', str(build), '--target', 'wat2wasm', 'wasm2c', 'wasm-validate', '-j', '8'], check=True)
    for tool in ['wat2wasm', 'wasm2c', 'wasm-validate']:
        assert subprocess.check_output([str(build / tool), '--version'], text=True).strip() == VERSION
    print(f'Native WABT {VERSION} tools ready: {build}')


if __name__ == '__main__':
    main()
