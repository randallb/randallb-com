import io
import signal
import termios
import unittest
from unittest.mock import Mock, patch

import interactive
import lifecycle as vm


class InteractivePreflightTests(unittest.TestCase):
    def test_redirected_input_is_rejected(self):
        with patch.object(interactive.sys, 'stdin', io.StringIO()):
            with self.assertRaisesRegex(RuntimeError, 'terminal stdin and stdout'):
                interactive.require_terminal()

    def test_uncontrolled_terminal_is_rejected(self):
        terminal = Mock()
        terminal.isatty.return_value = True
        terminal.fileno.return_value = 0
        with patch.object(interactive.sys, 'stdin', terminal), patch.object(interactive.sys, 'stdout', terminal), \
             patch.object(interactive.os, 'tcgetpgrp', side_effect=OSError('no controlling terminal')):
            with self.assertRaisesRegex(RuntimeError, 'controlling terminal'):
                interactive.require_terminal()

    def test_background_process_is_rejected(self):
        terminal = Mock()
        terminal.isatty.return_value = True
        terminal.fileno.return_value = 0
        with patch.object(interactive.sys, 'stdin', terminal), patch.object(interactive.sys, 'stdout', terminal), \
             patch.object(interactive.os, 'tcgetpgrp', return_value=123), patch.object(interactive.os, 'getpgrp', return_value=456):
            with self.assertRaisesRegex(RuntimeError, 'foreground terminal'):
                interactive.require_terminal()

    def test_terminal_loss_records_failure_without_masking_cancel_or_timeout(self):
        for interrupted, exception in [(signal.SIGTERM, vm.Cancelled), (0, TimeoutError)]:
            with self.subTest(interrupted=interrupted):
                machine = Mock()
                machine.record = {'backend': '/fake/tart', 'machine': 'owned'}
                machine.environment = {}
                child = Mock(pid=12345, returncode=-15)
                child.poll.return_value = None
                with patch.object(interactive, 'require_terminal'), \
                     patch.object(interactive.termios, 'tcgetattr', return_value=[]), \
                     patch.object(interactive.termios, 'tcsetattr', side_effect=termios.error(25, 'terminal lost')), \
                     patch.object(interactive.subprocess, 'Popen', return_value=child), \
                     patch.object(interactive.os, 'killpg'), patch.object(vm, 'interrupted', interrupted):
                    with self.assertRaises(exception):
                        interactive.execute(machine, ['true'], timeout=0)
                entry = machine.record['operations'][-1]
                self.assertEqual(entry['terminal_restoration']['status'], 'failed')
                self.assertIn('terminal lost', entry['terminal_restoration']['error'])
                self.assertIn('elapsed_seconds', entry)
                machine.persist.assert_called()


if __name__ == '__main__':
    unittest.main()
