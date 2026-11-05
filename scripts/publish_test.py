"""Validate source isolation and content-change behavior of the Drive publisher."""
import json
from pathlib import Path
import tempfile
import unittest
from publish import build, MAX_SOURCE_BYTES


class PublishTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.source = self.root / 'drive'
        self.source.mkdir()
        (self.source / 'page.html').write_text('first')
        self.manifest = {'version': 1, 'noindex': True,
                         'routes': {'/': {'file': 'page.html', 'content_type': 'text/html'}},
                         'not_found': {'file': 'page.html', 'content_type': 'text/html'}}
        self.write_manifest()
        self.output = self.root / 'build'

    def write_manifest(self):
        (self.source / 'site.json').write_text(json.dumps(self.manifest))

    def test_drive_edit_changes_build_and_revision_without_code_change(self):
        first = build(self.source, self.output)
        (self.source / 'page.html').write_text('second')
        second = build(self.source, self.output)
        self.assertNotEqual(first['revision'], second['revision'])
        content = json.loads((self.output / 'site-content.json').read_text())
        self.assertEqual((self.output / 'assets' / content['routes']['/']['asset'].lstrip('/')).read_bytes(), b'second')
        (self.source / 'private.txt').write_text('unpublished')
        self.assertEqual(build(self.source, self.output)['revision'], second['revision'])

    def test_escape_and_missing_files_fail_before_build_output(self):
        outside = self.root / 'private.txt'
        outside.write_text('private')
        (self.source / 'link.txt').symlink_to(outside)
        for path in ['../private.txt', 'link.txt', 'missing.txt']:
            self.manifest['routes']['/']['file'] = path
            self.write_manifest()
            with self.assertRaises((ValueError, FileNotFoundError)):
                build(self.source, self.output)
            self.assertFalse(self.output.exists())

    def test_route_validation_and_binary_assets(self):
        for route in ['/../secret', '//other.test/path', '/p?x=1', '/%2e%2e/secret']:
            self.manifest['routes'][route] = self.manifest['not_found']
            self.write_manifest()
            with self.assertRaises(ValueError):
                build(self.source, self.output)
            del self.manifest['routes'][route]
        (self.source / 'asset.bin').write_bytes(bytes([0, 255, 128]))
        self.manifest['routes']['/asset'] = {'file': 'asset.bin', 'content_type': 'application/octet-stream'}
        self.write_manifest()
        build(self.source, self.output)
        content = json.loads((self.output / 'site-content.json').read_text())
        self.assertEqual((self.output / 'assets' / content['routes']['/asset']['asset'].lstrip('/')).read_bytes(), bytes([0, 255, 128]))

    def test_large_site_and_exact_limit(self):
        # Repeated routes count a file once; a >4 MB asset is stored outside code.
        data = b'x' * 5_000_000
        (self.source / 'page.html').write_bytes(data)
        receipt = build(self.source, self.output)
        self.assertGreater(receipt['source_bytes'], 4_000_000)
        self.assertLess((self.output / 'site-content.json').stat().st_size, 2000)
        from unittest.mock import patch
        with patch('publish.MAX_SOURCE_BYTES', receipt['source_bytes']):
            build(self.source, self.output)
        with patch('publish.MAX_SOURCE_BYTES', receipt['source_bytes'] - 1):
            with self.assertRaisesRegex(ValueError, '100 MB'):
                build(self.source, self.output)

    def test_rebuild_removes_old_assets_and_keeps_indexing_scoped(self):
        build(self.source, self.output)
        (self.source / 'page.html').write_text('replacement')
        self.manifest['routes']['/']['noindex'] = False
        self.write_manifest()
        build(self.source, self.output)
        self.assertEqual(len(list((self.output / 'assets').iterdir())), 1)
        site = json.loads((self.output / 'site-content.json').read_text())
        self.assertFalse(site['routes']['/']['noindex'])
        self.assertTrue(site['not_found']['noindex'])


if __name__ == '__main__':
    unittest.main()
