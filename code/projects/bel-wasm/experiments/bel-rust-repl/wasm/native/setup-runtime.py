#!/usr/bin/env python3
"""Fetch checksum-pinned WABT 1.0.39 C runtime sources into build output."""
import hashlib
import io
from pathlib import Path
import tarfile
import urllib.request

ROOT = Path(__file__).resolve().parents[4]
DEST = ROOT / 'target/wabt-native-1.0.39'
URL = 'https://codeload.github.com/WebAssembly/wabt/tar.gz/refs/tags/1.0.39'
SHA256 = '8aaafc02afdb8832bd51140b833816512a834601a65348e1f433cece6bbf2db7'


def main():
    data = urllib.request.urlopen(URL, timeout=60).read()
    if hashlib.sha256(data).hexdigest() != SHA256:
        raise ValueError('WABT archive checksum mismatch')
    with tarfile.open(fileobj=io.BytesIO(data), mode='r:gz') as archive:
        for member in archive.getmembers():
            name = member.name.split('/', 1)[-1]
            # Only top-level runtime files and their license; no archive paths
            # are passed to tarfile extraction or used without validation.
            if member.isfile() and (name == 'LICENSE' or
                (name.startswith('wasm2c/') and name.count('/') == 1 and
                 name.endswith(('.h', '.c', '.inc')))):
                path = DEST / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(archive.extractfile(member).read())
    print(f'WABT 1.0.39 runtime sources: {DEST}')


if __name__ == '__main__':
    main()
