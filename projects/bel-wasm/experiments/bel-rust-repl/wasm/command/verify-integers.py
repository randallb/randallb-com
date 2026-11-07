#!/usr/bin/env python3
"""Check compiled signed integers through the installed native rbel command.

These checks use rbel's reader, compiler, and executable examples. Python builds
fixtures and verifies process outcomes; it does not evaluate rbel expressions.
"""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[4]


def chapter(name, definitions, cases):
    blocks = [f'```rbel definition={name}\n{definitions}\n```\n']
    for label, expression, expected in cases:
        blocks.append(f'```rbel example={name}-{label}\n{expression}\n=>\n{expected}\n```\n')
    return '\n'.join(blocks)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--rbel', type=Path, default=ROOT / 'target/rbel-command/bin/rbel')
    parser.add_argument('--limit', type=int, default=256,
                        help='documented maximum supported integer magnitude')
    parser.add_argument('--report', type=Path, default=ROOT / 'target/integer-verification/results.json')
    args = parser.parse_args()
    command = args.rbel.resolve()
    if args.limit < 20:
        parser.error('--limit must be at least 20')
    report = {'passed': False, 'checks': [], 'limit': args.limit,
              'command_sha256': hashlib.sha256(command.read_bytes()).hexdigest(),
              'integer_source_sha256': hashlib.sha256((ROOT / 'literate/integers.rbel.md').read_bytes()).hexdigest()}
    args.report.parent.mkdir(parents=True, exist_ok=True)
    args.report.unlink(missing_ok=True)

    def invoke(label, mode, source, *options, success=True, timeout=90):
        result = subprocess.run([str(command), mode, str(source), *map(str, options)],
                                text=True, capture_output=True, timeout=timeout)
        expected_status = 0 if success else 1
        if result.returncode != expected_status:
            raise AssertionError((label, result.returncode, result.stdout[-4000:], result.stderr[-4000:]))
        if not success and (result.stdout or not result.stderr):
            raise AssertionError((label, 'failure must have diagnostics and no stdout', result))
        report['checks'].append({'name': label, 'exit': result.returncode})
        print('PASS', label, flush=True)
        return result

    try:
        with tempfile.TemporaryDirectory(prefix='rbel integer acceptance ') as directory:
            work = Path(directory)
            shutil.copy2(ROOT / 'literate/integers.rbel.md', work / 'integers.rbel.md')
            definitions = '(import "integers.rbel.md")\n(def integer-acceptance (x) x)'
            cases = [
                ('literal', '14', '14'),
                ('negative', '-3', '-3'),
                ('signed-positive', '+2', '2'),
                ('leading-zero', '00014', '14'),
                ('normalized-negative-zero', '-0', '0'),
                ('normalized-positive-zero', '+000', '0'),
                ('add', '(+ 14 2)', '16'),
                ('first-class-add', '((fn (operation) (operation 14 2)) +)', '16'),
                ('closure-captures-number', '(((fn (bonus) (fn (face) (+ face bonus))) 2) 14)', '16'),
                ('subtract', '(- 14 2)', '12'),
                ('add-negative', '(+ 2 -3)', '-1'),
                ('subtract-negative', '(- 2 -3)', '5'),
                ('both-negative', '(+ -2 -3)', '-5'),
                ('cross-zero', '(- -2 -3)', '1'),
                ('cancel', '(+ -14 14)', '0'),
                ('zero-is-integer', '(integer 0)', 't'),
                ('zero-is-true', '(if 0 \'yes \'no)', 'yes'),
                ('zero-not-nil', '(id 0 nil)', 'nil'),
                ('equal-numbers', '(= (+ 12 2) 14)', 't'),
                ('unequal-numbers', '(= 14 -14)', 'nil'),
                ('less-positive', '(< 2 14)', 't'),
                ('less-negative', '(< -14 -2)', 't'),
                ('less-equal-false', '(< 2 2)', 'nil'),
                ('less-or-equal', '(<= 2 2)', 't'),
                ('greater', '(> -2 -14)', 't'),
                ('greater-or-equal', '(>= (+ 14 2) 10)', 't'),
                ('greater-false', '(> 2 14)', 'nil'),
                ('greater-or-equal-false', '(>= -3 2)', 'nil'),
                ('less-or-equal-false', '(<= 2 -3)', 'nil'),
                ('negative-less-zero', '(< -1 0)', 't'),
                ('zero-less-positive', '(< 0 1)', 't'),
                ('integer-symbol-false', '(integer \'seed)', 'nil'),
                ('integer-text-false', '(integer "14")', 'nil'),
                ('integer-nil-false', '(integer nil)', 'nil'),
                ('integer-pair-false', '(integer \'(a b))', 'nil'),
                ('integer-fraction-false', "(integer '(lit num (+ (t) (t t)) (+ nil (t))))", 'nil'),
                ('integer-negative-zero-false', "(integer '(lit num (- nil (t)) (+ nil (t))))", 'nil'),
                ('integer-nonzero-imaginary-false', "(integer '(lit num (+ (t) (t)) (+ (t) (t))))", 'nil'),
                ('quoted-record', "'((face 14) (bonus 2) (total 16))", '((face 14) (bonus 2) (total 16))'),
                ('computed-record', "(join 'total (join (+ 14 2) nil))", '(total 16)'),
                ('read-as-data', "'(14 -3 +2 0)", '(14 -3 2 0)'),
                ('decimal-print', '(integer-text (+ 14 2))', '"16"'),
                ('negative-print', '(integer-text -14)', '"-14"'),
                ('zero-print', '(integer-text 0)', '"0"'),
                ('identifier-with-digit', "'d20", 'd20'),
            ]
            source = work / 'positive.rbel.md'
            source.write_text(chapter('integer-acceptance', definitions, cases))
            invoke('native-integer-values-arithmetic-comparison', 'check', source, timeout=180)

            # This is separate so the maximum does not multiply module constants
            # across the ordinary arithmetic coverage above.
            source = work / 'bound.rbel.md'
            source.write_text(chapter('integer-bound', definitions, [
                ('positive-bound', f'(integer {args.limit})', 't'),
                ('negative-bound', f'(integer -{args.limit})', 't'),
                ('sum-at-bound', f'(= (+ {args.limit - 1} 1) {args.limit})', 't'),
            ]))
            invoke('native-integer-accepted-bound', 'check', source, timeout=180)

            # Malformed and unsupported literals must fail in reading, rather
            # than silently becoming symbols or partly parsed expressions.
            source = work / 'invalid.rbel'
            bad_tokens = ['1.5', '1/2', '1e3', '0x10', '-3abc', '12abc', '+-2', '1+',
                          str(args.limit + 1), '-' + str(args.limit + 1), '9' * 256]
            for index, token in enumerate(bad_tokens):
                source.write_text(f'(def bad (x) {token})\n')
                invoke(f'reject-numeric-notation-{index}', 'read', source, success=False, timeout=20)

            # A syntactically valid program can still violate the arithmetic
            # contract. Run examples so this tests execution, not merely parsing.
            bad_operations = [
                ('wrong-type-add', '(+ "14" 2)'),
                ('wrong-type-subtract', "(- 'seed 2)"),
                ('wrong-type-equality', '(= nil 0)'),
                ('wrong-type-ordering', '(< 0 "1")'),
                ('malformed-number-add', "(+ '(lit num (+ (t) (t t)) (+ nil (t))) 2)"),
                ('positive-overflow', f'(+ {args.limit} 1)'),
                ('negative-overflow', f'(- -{args.limit} 1)'),
            ]
            source = work / 'invalid-operation.rbel.md'
            for label, expression in bad_operations:
                source.write_text(chapter('integer-invalid', definitions, [('must-fail', expression, 'nil')]))
                invoke(label, 'check', source, success=False, timeout=90)

            # Build an actual native byte-list command. It must print decimal
            # characters, independently of JSON/raw-pair diagnostic transport.
            source = work / 'print.rbel'
            source.write_text('(import "integers.rbel.md")\n'
                              '(def integer-command (ignored) (integer-text (+ 14 2)))\n')
            output = work / 'integer command'
            invoke('native-integer-build', 'build', source, '-o', output, timeout=180)
            result = subprocess.run([str(output)], input='x', text=True, capture_output=True,
                                    timeout=10, env={'PATH': ''})
            if result.returncode != 0 or result.stdout != '16' or result.stderr:
                raise AssertionError(('native-integer-output', result))
            report['checks'].append({'name': 'native-integer-output', 'exit': result.returncode})
            print('PASS native-integer-output', flush=True)
        report['passed'] = True
    finally:
        args.report.write_text(json.dumps(report, indent=2) + '\n')
    print(f"{len(report['checks'])} native integer checks passed")


if __name__ == '__main__':
    main()
