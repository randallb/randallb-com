"""Checks for extraction and source drift; run with python3 literate/check_test.py."""
import unittest
from unittest.mock import patch
import json
import tempfile
from pathlib import Path

from check import ROOT, check_pin, extract


class ExtractionTests(unittest.TestCase):
    def setUp(self):
        self.chapter = (ROOT / 'literate/foundation.rbel.md').read_bytes().decode()

    def test_chapter_reconstructs_reference_bytes(self):
        source, _, count = extract(self.chapter)
        check_pin(source)
        self.assertEqual(source, (ROOT / 'experiments/bel-rust-repl/examples/bootstrap/00-foundation.bel').read_bytes())
        self.assertEqual(count, 67)

    def test_upstream_identity_must_match_even_with_local_adaptations(self):
        pin = json.loads((ROOT / 'literate/source-pin.json').read_text())
        origin = json.loads((ROOT / 'ORIGIN.json').read_text())
        origin['revision'] = 'different-upstream'
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'literate').mkdir()
            (root / 'literate/source-pin.json').write_text(json.dumps(pin))
            (root / 'ORIGIN.json').write_text(json.dumps(origin))
            with patch('check.ROOT', root), self.assertRaisesRegex(ValueError, 'provenance'):
                check_pin(b'')

    def test_rejects_changed_definition_and_line_endings(self):
        source, _, _ = extract(self.chapter)
        for altered in (source.replace(b'(id x nil)', b'(id x t)'), source.replace(b'\n', b'\r\n')):
            with self.subTest(altered=altered[:30]), self.assertRaisesRegex(ValueError, 'SHA-256'):
                check_pin(altered)

    def test_prose_and_unrelated_fences_do_not_execute(self):
        original = extract(self.chapter)
        wrapped = '````text\n```rbel definition=fake\nnot code\n```\n````\n' + self.chapter
        self.assertEqual(extract(wrapped), original)

    def test_rejects_broken_blocks(self):
        for suffix, message in [
            ('```rbel definition=no\nx\n```\n', 'duplicate'),
            ('```rbel policy=unknown\nx\n```\n', 'unsupported'),
            ('```rbel example=unfinished\nx\n', 'unclosed'),
            ('```rbel example=empty-body\n=>\nx\n```\n', 'expected expression'),
            ('```rbel example=extra\nx\n=>\ny\n=>\nz\n```\n', 'expected expression'),
        ]:
            with self.subTest(message=message), self.assertRaisesRegex(ValueError, message):
                extract(self.chapter + suffix)


if __name__ == '__main__':
    unittest.main()
