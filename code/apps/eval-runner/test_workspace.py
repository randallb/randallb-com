import io
import json
import os
from pathlib import Path
import tarfile
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

import workspace


class WorkspaceTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.source = self.root / 'source'
        self.source.mkdir()
        (self.source / 'docs').mkdir()
        (self.source / 'docs' / 'request.md').write_text('Original request\n')
        self.archive = self.root / 'transfer.tar'

    def make_archive(self, members):
        with tarfile.open(self.archive, 'w') as tar:
            for name, kind, data in members:
                entry = tarfile.TarInfo(name)
                entry.type = kind
                if kind == tarfile.REGTYPE:
                    entry.size = len(data)
                    tar.addfile(entry, io.BytesIO(data))
                else:
                    entry.linkname = 'outside'
                    tar.addfile(entry)
        return self.archive

    def test_snapshot_is_frozen_and_roundtrips_files_empty_dirs_and_modes(self):
        (self.source / 'empty').mkdir()
        program = self.source / 'run.sh'
        program.write_text('#!/bin/sh\necho hi\n'); program.chmod(0o755)
        snap = self.root / 'snapshot'
        receipt = workspace.snapshot(self.source, snap)
        (self.source / 'docs/request.md').write_text('Later change\n')
        exported = self.root / 'restored'
        actual = workspace.extract(snap / 'archive.tar', exported)
        self.assertEqual(actual, receipt)
        self.assertEqual((exported / 'docs/request.md').read_text(), 'Original request\n')
        self.assertTrue((exported / 'empty').is_dir())
        self.assertTrue((exported / 'run.sh').stat().st_mode & 0o111)

    def test_changed_workspace_no_longer_matches_manifest(self):
        expected = workspace.manifest(self.source)
        (self.source / 'docs/request.md').write_text('Changed\n')
        with self.assertRaisesRegex(ValueError, 'differs'):
            workspace.validate_manifest(self.source, expected)

    def test_diff_for_files_without_final_newlines_remains_readable(self):
        (self.source / 'one').write_text('before')
        before = self.root / 'before'
        workspace.snapshot(self.source, before)
        (self.source / 'one').write_text('after')
        out = self.root / 'changes'; out.mkdir()
        workspace.diff(before / 'workspace', self.source, out)
        patch = (out / 'diff.patch').read_text()
        self.assertIn('-before\n\\ No newline at end of file\n', patch)
        self.assertIn('+after\n\\ No newline at end of file\n', patch)

    def test_large_or_many_line_text_retains_files_but_skips_expensive_diff(self):
        (self.source / 'large').write_text('old\n')
        (self.source / 'many-lines').write_text('old\n')
        before = self.root / 'before'
        workspace.snapshot(self.source, before)
        (self.source / 'large').write_text('new' * workspace.MAX_DIFF_BYTES)
        (self.source / 'many-lines').write_text('new\n' * (workspace.MAX_DIFF_LINES + 1))
        out = self.root / 'changes'; out.mkdir()
        with patch.object(workspace.difflib, 'unified_diff', side_effect=AssertionError('large text reached diff engine')):
            result = workspace.diff(before / 'workspace', self.source, out)
        self.assertEqual(len(result['files']), 2)
        self.assertIn('Text diff omitted by inspection limits: large', (out / 'diff.patch').read_text())
        self.assertIn('Text diff omitted by inspection limits: many-lines', (out / 'diff.patch').read_text())
        self.assertEqual(workspace.manifest(self.source)['files']['large']['size'], 3 * workspace.MAX_DIFF_BYTES)

    def test_patch_budget_caps_output_and_keeps_complete_change_manifest(self):
        for i in range(30):
            (self.source / f'file-{i:02}').write_text('a' * 100 + '\n')
        before = self.root / 'before'
        workspace.snapshot(self.source, before)
        for i in range(30):
            (self.source / f'file-{i:02}').write_text('b' * 100 + '\n')
        out = self.root / 'changes'; out.mkdir()
        with patch.object(workspace, 'MAX_PATCH_BYTES', 1024):
            result = workspace.diff(before / 'workspace', self.source, out)
        self.assertLessEqual((out / 'diff.patch').stat().st_size, 1024)
        self.assertEqual(len(result['files']), 30)
        self.assertIn('patch-size limit', (out / 'diff.patch').read_text())

    def test_cancelled_diff_stops_before_host_text_processing(self):
        import lifecycle
        before = self.root / 'before'
        workspace.snapshot(self.source, before)
        out = self.root / 'changes'; out.mkdir()
        with patch.object(lifecycle, 'interrupted', 15):
            with self.assertRaises(lifecycle.Cancelled):
                workspace.diff(before / 'workspace', self.source, out)

    def test_runner_records_input_snapshot_before_prerequisite_failure(self):
        import tart_backend
        args = SimpleNamespace(command=['/bin/true'], interactive=False, boot_timeout=180,
                               cpus=4, memory=8192, state=str(self.root / 'state'),
                               workspace_input=self.source, workspace_output=self.root / 'export')
        with patch.object(tart_backend.platform, 'machine', return_value='unsupported'), \
                patch.object(tart_backend, 'Machine') as machine:
            self.assertEqual(tart_backend.run(args, {'revision': None}), 1)
        machine.assert_not_called()
        record_path, = (self.root / 'state').glob('run-*/result.json')
        record = json.loads(record_path.read_text())
        self.assertIn('requires Apple Silicon', record['error'])
        self.assertEqual(record['inputs']['workspace']['manifest'], workspace.manifest(self.source))
        self.assertTrue((Path(record['inputs']['workspace']['path']) / 'workspace/docs/request.md').is_file())
        self.assertEqual(record['workspace_export']['status'], 'incomplete')
        self.assertEqual(record['cleanup']['status'], 'not_needed')

    def test_diff_retains_added_modified_deleted_and_binary_files(self):
        (self.source / 'delete.txt').write_text('removed\n')
        before = self.root / 'before'
        workspace.snapshot(self.source, before)
        (self.source / 'delete.txt').unlink()
        (self.source / 'docs/request.md').write_text('New request\n')
        (self.source / 'new.bin').write_bytes(b'\x00\xff')
        output = self.root / 'diff'; output.mkdir()
        changes = workspace.diff(before / 'workspace', self.source, output)
        self.assertEqual({r['path']: r['status'] for r in changes['files']},
                         {'delete.txt': 'deleted', 'docs/request.md': 'modified', 'new.bin': 'added'})
        self.assertIn('-Original request', (output / 'diff.patch').read_text())
        self.assertIn('+New request', (output / 'diff.patch').read_text())
        self.assertIn('Binary file added: new.bin', (output / 'diff.patch').read_text())

    def test_input_links_and_special_files_are_rejected(self):
        for kind in ('symlink', 'hardlink', 'fifo'):
            with self.subTest(kind=kind):
                bad = self.source / 'bad'
                if kind == 'symlink': bad.symlink_to(self.source / 'docs/request.md')
                elif kind == 'hardlink': os.link(self.source / 'docs/request.md', bad)
                else: os.mkfifo(bad)
                try:
                    with self.assertRaises(ValueError): workspace.manifest(self.source)
                finally:
                    bad.unlink()

    def test_snapshot_cannot_be_written_inside_its_source(self):
        with self.assertRaises(ValueError):
            workspace.snapshot(self.source, self.source / 'nested')

    def test_archive_paths_cannot_escape_destination(self):
        for member in ('../outside', '/absolute', './a/../../outside', 'a//b', 'a\\b', 'a\nnew'):
            with self.subTest(member=member):
                self.make_archive([(member, tarfile.REGTYPE, b'bad')])
                with self.assertRaises(ValueError):
                    workspace.extract(self.archive, self.root / ('rejected-' + str(len(list(self.root.iterdir())))))
        self.assertFalse((self.root / 'outside').exists())

    def test_archive_links_and_special_files_are_rejected(self):
        for index, kind in enumerate((tarfile.SYMTYPE, tarfile.LNKTYPE, tarfile.FIFOTYPE, tarfile.CHRTYPE)):
            self.make_archive([('bad', kind, b'')])
            with self.assertRaises(ValueError):
                workspace.extract(self.archive, self.root / f'bad-{index}')

    def test_duplicate_members_cannot_replace_prior_contents(self):
        self.make_archive([('same', tarfile.REGTYPE, b'first'), ('same', tarfile.REGTYPE, b'second')])
        with self.assertRaisesRegex(ValueError, 'duplicate'):
            workspace.extract(self.archive, self.root / 'duplicates')

    def test_byte_entry_and_archive_limits_apply_before_acceptance(self):
        with patch.object(workspace, 'MAX_BYTES', 2):
            with self.assertRaises(ValueError): workspace.manifest(self.source)
        self.make_archive([('large', tarfile.REGTYPE, b'12345')])
        with patch.object(workspace, 'MAX_BYTES', 4):
            with self.assertRaises(ValueError): workspace.extract(self.archive, self.root / 'too-large')
        with patch.object(workspace, 'MAX_ARCHIVE', 2):
            with self.assertRaises(ValueError): workspace.extract(self.archive, self.root / 'archive-too-large')
        with patch.object(workspace, 'MAX_FILES', 1):
            with self.assertRaises(ValueError): workspace.manifest(self.source)
        self.make_archive([('one', tarfile.REGTYPE, b''), ('two', tarfile.REGTYPE, b'')])
        with patch.object(workspace, 'MAX_FILES', 1):
            with self.assertRaises(ValueError): workspace.extract(self.archive, self.root / 'too-many')

    def test_export_reads_full_binary_stream_file_and_produces_inspectable_diff(self):
        before = self.root / 'before'
        workspace.snapshot(self.source, before)
        (self.source / 'docs/request.md').write_text('Agent edit\n')
        self.make_archive([('docs', tarfile.DIRTYPE, b''),
                           ('docs/request.md', tarfile.REGTYPE, b'Agent edit\n'),
                           ('artifact.bin', tarfile.REGTYPE, b'x' * (1024 * 1024 + 10))])
        prefix = self.root / 'operation'
        Path(str(prefix) + '.stdout').write_bytes(self.archive.read_bytes())
        class Machine:
            record = {'machine': 'test-guest'}
            def call(self, argv, **kwargs):
                self.argv = argv
                return 0, b'truncated-control-buffer', b'', prefix
        machine = Machine()
        result = workspace.export(machine, before, self.root / 'export')
        self.assertEqual(result['status'], 'complete')
        self.assertEqual(result['manifest']['files']['artifact.bin']['size'], 1024 * 1024 + 10)
        self.assertIn('+Agent edit', (self.root / 'export/diff.patch').read_text())
        self.assertIn(workspace.GUEST_ROOT, machine.argv)
        self.assertTrue((self.root / 'export/workspace/docs/request.md').is_file())


if __name__ == '__main__':
    unittest.main()
