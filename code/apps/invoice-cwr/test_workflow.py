import hashlib
import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / 'eval-runner'))
import workspace
import workflow


class SavedWorkspaceGradingTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.run_id = 'a' * 32
        self.saved = self.root / 'runs' / self.run_id / 'evaluation'
        self.files = self.saved / 'export' / 'workspace'
        self.files.mkdir(parents=True)
        (self.root / 'runner').mkdir()
        (self.root / 'runner' / 'acceptance_eval.py').write_text('grader source fixture')
        self.output = self.root / 'grade'
        self.output.mkdir()

    def seal(self, program=None):
        if program is not None:
            (self.files / 'invoice-total.sh').write_text(program)
        manifest = self.saved / 'export' / 'manifest.json'
        manifest.write_text(json.dumps(workspace.manifest(self.files)))
        contract = self.saved / 'grading-contract.json'
        contract.write_text(json.dumps({'grader': 'invoice-total-v1', 'version': 1}))
        (self.saved / 'eval-result.json').write_text(json.dumps({
            'status': 'completed', 'workspace_manifest_sha256': workflow.digest(manifest),
            'contract_sha256': workflow.digest(contract)}))

    def grade(self):
        return workflow.grade_result(self.root, self.output, {'run_id': self.run_id})

    def test_missing_solution_is_failed_grade_without_starting_model_or_vm(self):
        self.seal()
        with patch.object(workflow, 'run_cli', side_effect=AssertionError('must not execute')):
            result = self.grade()
        self.assertEqual(result['status'], 'failed')
        self.assertEqual(len(result['cases']), 5)
        self.assertEqual(result['cases_passed'], [])
        self.assertEqual(result['source_run_id'], self.run_id)

    def test_empty_solution_is_failed_grade_and_retained_for_inspection(self):
        self.seal('')
        with patch.object(workflow, 'run_cli', side_effect=AssertionError('must not execute')):
            result = self.grade()
        self.assertEqual(result['status'], 'failed')
        self.assertEqual((self.output / 'invoice-total.sh').read_bytes(), b'')
        self.assertEqual(result['artifact_sha256'], hashlib.sha256(b'').hexdigest())

    def test_modified_saved_solution_is_rejected_before_running_it(self):
        self.seal('echo 0\n')
        (self.files / 'invoice-total.sh').write_text('echo 1350\n')
        with patch.object(workflow, 'run_cli', side_effect=AssertionError('must not execute')):
            with self.assertRaisesRegex(ValueError, 'saved workspace changed'):
                self.grade()

    def test_changed_input_between_validation_and_freeze_is_rejected(self):
        self.seal('echo 0\n')
        wrong = {'inputs': {'workspace': {'manifest': {'wrong': 'snapshot'}}}}
        with patch.object(workflow, 'run_cli', return_value=(0, self.output / 'result.json', wrong)):
            with self.assertRaisesRegex(ValueError, 'input changed'):
                self.grade()

    def test_changed_grading_contract_is_rejected(self):
        self.seal('echo 0\n')
        (self.saved / 'grading-contract.json').write_text('{}')
        with self.assertRaisesRegex(ValueError, 'grading contract changed'):
            self.grade()


if __name__ == '__main__':
    unittest.main()
