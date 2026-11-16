import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import Mock, patch

import host_auth


class HostAuthTests(unittest.TestCase):
    def test_current_home_and_explicit_override_do_not_change_source(self):
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / 'auth.json'
            original = b'{"tokens":{"access_token":"synthetic-secret"}}'
            path.write_bytes(original)
            with patch.dict(os.environ, {'CODEX_HOME': folder}):
                self.assertEqual(host_auth.source(), path)
                self.assertEqual(host_auth.source(folder), path)
            self.assertEqual(path.read_bytes(), original)

    def test_missing_or_invalid_cache_fails_without_revealing_contents(self):
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / 'auth.json'
            for value in (None, b'synthetic-secret-invalid-json', b'{}', b'[]'):
                if value is not None:
                    path.write_bytes(value)
                with self.assertRaises(RuntimeError) as error:
                    host_auth.source(folder)
                self.assertNotIn('synthetic-secret', str(error.exception))

    def test_credentials_use_stdin_and_never_enter_receipt_or_argv(self):
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / 'auth.json'
            path.write_text('{"OPENAI_API_KEY":"synthetic-secret"}')
            machine = Mock()
            machine.record = {'machine': 'synthetic-guest'}
            receipt = host_auth.install(machine, path)
            args, kwargs = machine.call.call_args
            self.assertEqual(kwargs['input_path'], path)
            self.assertNotIn('synthetic-secret', json.dumps([args, receipt]))
            self.assertIn('umask 077', args[0][5])
            self.assertFalse(receipt['host_writeback'])
