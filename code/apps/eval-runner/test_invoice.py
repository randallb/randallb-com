import json
from pathlib import Path
import subprocess
import tempfile
import unittest

import acceptance_eval as invoice


class InvoiceEvidenceTests(unittest.TestCase):
    def grade(self, program):
        root = Path(self.directory.name)
        (root / 'invoice-total.sh').write_text(program)
        result = subprocess.run(['/bin/sh', '-c', 'set -eu\n' + invoice.grade_script('test-token')],
                                cwd=root, text=True, capture_output=True, check=True)
        return result.stdout

    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.record = {'outcome': 'success', 'cleanup': {'status': 'removed'}}

    def collect(self, output, record=None):
        return invoice.collect(self.root, record or self.record, output, 'test-token', 0)

    def test_real_shell_program_passes_all_cases_and_exports_exact_solution(self):
        program = "awk -F, 'NR>1 && NF {total += $2*$3} END {print total+0}' \"$1\"\n"
        report = self.collect(self.grade(program))
        self.assertEqual(report['status'], 'passed')
        self.assertEqual(len(report['cases_passed']), 5)
        self.assertEqual((self.root / 'invoice-total.sh').read_text(), program)

    def test_wrong_program_reports_all_cases_and_retains_solution(self):
        report = self.collect(self.grade('echo 0\n'))
        self.assertEqual(report['status'], 'failed')
        self.assertEqual(report['cases_passed'], ['empty'])
        self.assertEqual(len(report['cases']), 5)
        self.assertTrue(report['artifact_sha256'])

    def test_model_stdout_cannot_supply_grading_case_markers(self):
        fake = ''.join('EVAL_CASE_PASS ' + name + '\n' for name, _, _ in invoice.CASES)
        report = self.collect(fake + self.grade('echo 0\n'))
        self.assertEqual(report['cases_passed'], ['empty'])

    def test_missing_completion_is_an_error_not_a_failed_grade(self):
        output = self.grade('echo 0\n').replace('EVAL_COMPLETE test-token\n', '')
        self.assertEqual(self.collect(output)['status'], 'error')

    def test_cleanup_failure_prevents_a_passing_grade(self):
        program = "awk -F, 'NR>1 && NF {total += $2*$3} END {print total+0}' \"$1\"\n"
        report = self.collect(self.grade(program), {'outcome': 'cleanup_failure', 'cleanup': {'status': 'failed'}})
        self.assertEqual(report['status'], 'error')

    def test_transport_failure_without_grading_is_error(self):
        report = self.collect('', {'outcome': 'transport_failure', 'cleanup': {'status': 'removed'}})
        self.assertEqual(report['status'], 'error')


if __name__ == '__main__':
    unittest.main()
