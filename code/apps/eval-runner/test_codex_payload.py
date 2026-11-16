import hashlib
import io
import json
from pathlib import Path
import sys
import tarfile
import tempfile
import unittest
from unittest.mock import patch

import codex_payload as codex
import lifecycle as vm


class CodexPayloadTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.archive = self.root / 'codex.tar.gz'
        self.binary = b'synthetic test executable'
        self.pin = dict(codex.PIN)
        self.write_archive()

    def tearDown(self):
        self.temp.cleanup()

    def write_archive(self, symlink=False):
        with tarfile.open(self.archive, 'w:gz') as archive:
            entry = tarfile.TarInfo(self.pin['archive_member'])
            if symlink:
                entry.type = tarfile.SYMTYPE
                entry.linkname = '/unrelated/host/path'
                archive.addfile(entry)
            else:
                entry.size = len(self.binary)
                archive.addfile(entry, io.BytesIO(self.binary))
        self.pin['sha256'] = vm.digest(self.archive)
        (self.root / 'manifest.json').write_text(json.dumps(self.pin))

    def test_payload_records_actual_binary_digest(self):
        with patch.object(codex, 'PIN', self.pin):
            result = codex.verify(self.root)
        self.assertEqual(result['receipt']['binary_sha256'], hashlib.sha256(self.binary).hexdigest())

    def test_archive_tampering_is_rejected(self):
        self.archive.write_bytes(b'corrupt archive')
        with patch.object(codex, 'PIN', self.pin), self.assertRaisesRegex(RuntimeError, 'checksum mismatch'):
            codex.verify(self.root)

    def test_manifest_cannot_select_unpinned_download(self):
        (self.root / 'manifest.json').write_text(json.dumps({**self.pin, 'version': 'unapproved'}))
        with patch.object(codex, 'PIN', self.pin), self.assertRaisesRegex(RuntimeError, 'repository pin'):
            codex.verify(self.root)

    def test_archive_links_are_rejected_before_extraction(self):
        self.write_archive(symlink=True)
        with patch.object(codex, 'PIN', self.pin), self.assertRaisesRegex(RuntimeError, 'archive layout'):
            codex.verify(self.root)

    def test_guest_checksum_failure_prevents_installation(self):
        with patch.object(codex, 'PIN', self.pin):
            payload = codex.verify(self.root)
        class Guest:
            record = {'id': 'a' * 32, 'machine': 'owned'}
            def __init__(self):
                self.calls = []
            def call(self, argv, **kwargs):
                self.calls.append(argv)
                return 0, b'wrong-checksum  payload\n', b'', None
        guest = Guest()
        with self.assertRaisesRegex(RuntimeError, 'guest Codex archive checksum'):
            codex.install(guest, payload)
        self.assertEqual(len(guest.calls), 3)
        self.assertFalse(any('/usr/bin/tar' in str(argv) for argv in guest.calls))

    def test_file_stdin_transfer_does_not_mix_with_output(self):
        source = self.root / 'input'
        data = b'\x00binary\xff\n' * 200000
        source.write_bytes(data)
        code, stdout, stderr = vm.command([sys.executable, '-c',
            'import sys,hashlib; print(hashlib.sha256(sys.stdin.buffer.read()).hexdigest()); print("diagnostic",file=sys.stderr)'],
            timeout=5, input_path=source)
        self.assertEqual(code, 0)
        self.assertEqual(stdout.strip().decode(), hashlib.sha256(data).hexdigest())
        self.assertEqual(stderr, b'diagnostic\n')


if __name__ == '__main__':
    unittest.main()
