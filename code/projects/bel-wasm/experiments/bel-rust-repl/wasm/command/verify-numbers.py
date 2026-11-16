#!/usr/bin/env python3
"""Exercise compiled exact arithmetic and finite binary64 against independent data.

Python's int/Fraction/float/struct generate expected fixture data only. Every
expression under test is parsed and compiled by rbel and runs as Wasm or native
code; Python never evaluates the implementation's rbel arithmetic.
"""
import argparse
from fractions import Fraction
import hashlib
import json
from pathlib import Path
import random
import struct
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[4]


def bits(value):
    return f'{struct.unpack(">Q", struct.pack(">d", value))[0]:064b}'


def chapter(name, cases):
    return (f'```rbel definition={name}\n(def {name} (x) x)\n```\n' +
            '\n'.join(f'```rbel example={name}-{label}\n{expression}\n=>\n{expected}\n```\n'
                      for label, expression, expected in cases))


def oracle_cases():
    cases = [
        ('big-carry', '(+ 999999999999999999999999999999 1)', '1000000000000000000000000000000'),
        ('big-borrow', '(- 1000000000000000000000000000000 1)', '999999999999999999999999999999'),
        ('decimal-exact', '(= (+ 0.1 0.2) 0.3)', 't'),
        ('exponent-exact', '(= 1.25e3 1250)', 't'),
        ('negative-exponent', '(= 12.5e-3 1/80)', 't'),
        ('normalized-zero', '(exact-text -000/23)', '"0"'),
        ('reduced-fraction', '(exact-text 120/450)', '"4/15"'),
        ('zero-is-true', "(if 0 'yes 'no)", 'yes'),
        ('fraction-not-integer', '(integer 1/3)', 'nil'),
        ('integer-rational', '(integer 20/4)', 't'),
        ('integer-print', '(integer-text -123456789012345678901234567890)', '"-123456789012345678901234567890"'),
        ('fraction-print', '(number-text -120/450)', '"-4/15"'),
        ('closure', '(((fn (bonus) (fn (face) (+ face bonus))) 1000000000000) 14)', '1000000000014'),
        ('first-class', '((fn (operation) (operation 1/3 1/6)) +)', '1/2'),
        ('quoted-record', "'((count 12345678901234567890) (rate 1/3))", '((count 12345678901234567890) (rate 1/3))'),
        ('float-rounding-visible', '(= (+ 0.1f 0.2f) 0.3f)', 'nil'),
        ('float-recover', '(exact 0.1f)', str(Fraction.from_float(0.1))),
        ('float-negative-zero', '(float-text -0f)', '"-0f"'),
        ('float-negative-zero-bits', '(n-third -0f)', '"' + bits(-0.0) + '"'),
        ('float-zero-equality', '(= 0f -0f)', 't'),
        ('float-zero-order', '(< -0f 0f)', 'nil'),
        ('float-print', '(number-text 0.5f)', '"1/2f"'),
        ('reject-infinity-value', '(float-number \'(lit float "0111111111110000000000000000000000000000000000000000000000000000"))', 'nil'),
        ('reject-nan-value', '(float-number \'(lit float "0111111111111000000000000000000000000000000000000000000000000000"))', 'nil'),
        ('reject-short-bits', '(float-number \'(lit float "0"))', 'nil'),
        ('reject-nonreduced', '(exact-number \'(lit num + "2" "4"))', 'nil'),
    ]
    rng = random.Random(0x5242454C)
    for index in range(12):
        a, b = rng.randrange(-(10**30), 10**30), rng.randrange(1, 10**20)
        for label, operator, answer in [('add', '+', a+b), ('sub', '-', a-b), ('mul', '*', a*b)]:
            cases.append((f'int-{index}-{label}', f'({operator} {a} {b})', str(answer)))
        cases.append((f'int-{index}-order', f'(< {a} {b})', 't' if a < b else 'nil'))
    for index in range(12):
        a = Fraction(rng.randrange(-10000, 10000), rng.randrange(1, 10000))
        b = Fraction(rng.randrange(1, 10000), rng.randrange(1, 10000))
        for label, operator, answer in [('add', '+', a+b), ('sub', '-', a-b), ('mul', '*', a*b), ('div', '/', a/b)]:
            cases.append((f'fraction-{index}-{label}', f'({operator} {a} {b})', str(answer)))
        cases.append((f'fraction-{index}-order', f'(< {a} {b})', 't' if a < b else 'nil'))
    # Compare complete binary representations, not printed approximations.
    for index, spelling in enumerate(['0.1', '-0.2', '1.25', '1.25e3', '1e-20', '1e20',
                                      '9007199254740993', '1/3', '1/10', '-5/7']):
        exact = Fraction(spelling)
        cases.append((f'float-literal-{index}', f'(n-third {spelling}f)', '"' + bits(float(exact)) + '"'))
        cases.append((f'float-convert-{index}', f'(n-third (float {spelling}))', '"' + bits(float(exact)) + '"'))
    for index, exact in enumerate([Fraction(2**53+1), Fraction(2**53+3),
                                   Fraction(2**53+1, 2**53), Fraction(2**53+3, 2**53)]):
        cases.append((f'float-tie-{index}', f'(n-third (float {exact}))', '"' + bits(float(exact)) + '"'))
    for index, (op, a, b, answer) in enumerate([
        ('+', '0.1', '0.2', 0.1+0.2), ('-', '1.0', '0.1', 1.0-0.1),
        ('*', '1.5', '-2.0', 1.5*-2.0), ('/', '1.0', '3.0', 1.0/3.0)]):
        cases.append((f'float-operation-{index}', f'(n-third ({op} {a}f {b}f))', '"' + bits(answer) + '"'))
    return cases


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--rbel', type=Path, default=ROOT / 'target/rbel-command/bin/rbel')
    parser.add_argument('--portable', action='store_true', help='run examples on Wasm; leave native executable checks unverified')
    parser.add_argument('--report', type=Path, default=ROOT / 'target/numeric-verification/results.json')
    args = parser.parse_args()
    compiler = args.rbel.resolve()
    includes = [ROOT / 'literate/integers.rbel.md', ROOT / 'literate/floats.rbel.md']
    report = {'passed': False, 'checks': [], 'portable': args.portable, 'native_execution_checked': False,
              'compiler_sha256': hashlib.sha256(compiler.read_bytes()).hexdigest(),
              'sources': {str(p.relative_to(ROOT)): hashlib.sha256(p.read_bytes()).hexdigest() for p in includes}}
    args.report.parent.mkdir(parents=True, exist_ok=True)
    args.report.unlink(missing_ok=True)

    def invoke(label, source, success=True, timeout=240, extra=()):
        if args.portable:
            command = [sys.executable, str(ROOT / 'literate/test.py'), str(source), '--rbel', str(compiler)]
        else:
            command = [str(compiler), 'check', str(source)]
        for include in [*includes, *extra]:
            command.extend(['--include', str(include)])
        result = subprocess.run(command, text=True, capture_output=True, timeout=timeout)
        if (result.returncode == 0) != success:
            raise AssertionError((label, result.returncode, result.stdout[-6000:], result.stderr[-6000:]))
        if not success and not (result.stdout or result.stderr):
            raise AssertionError((label, 'failure had no diagnostic'))
        report['checks'].append({'name': label, 'passed': True, 'exit': result.returncode})
        print('PASS', label, flush=True)

    try:
        with tempfile.TemporaryDirectory(prefix='rbel numeric acceptance ') as directory:
            work = Path(directory)
            cases = oracle_cases()
            # Keep each compiled suite small enough for the bounded core.
            for start in range(0, len(cases), 24):
                source = work / f'oracle-{start}.rbel.md'
                source.write_text(chapter('numeric-oracle', cases[start:start+24]))
                invoke(f'oracle-cases-{start}-{min(start+24, len(cases))}', source)
            # Boundary conversion uses a separate module so ordinary checks do
            # not repeatedly materialize the large exact subnormal denominator.
            boundaries = [('minimum-subnormal', '5e-324'), ('minimum-normal', '2.2250738585072014e-308'),
                          ('maximum-finite', '1.7976931348623157e308'), ('positive-underflow', '1e-400'),
                          ('negative-underflow', '-1e-400')]
            for label, spelling in boundaries:
                source = work / 'boundary.rbel.md'
                source.write_text(chapter('numeric-boundary', [(label, f'(n-third {spelling}f)', '"'+bits(float(spelling))+'"')]))
                invoke(label, source, timeout=300)
            # Printing must remain readable at the representable boundaries,
            # including when conversion executes inside an emitted program.
            roundtrips = []
            for index, value in enumerate([0.0, -0.0, float.fromhex('0x0.0000000000001p-1022'),
                                            float.fromhex('0x1p-1022'), float.fromhex('0x1.fffffffffffffp1023'),
                                            0.1, -1/3]):
                raw = "'(lit float \"" + bits(value) + "\")"
                roundtrips.append((f'print-read-{index}',
                    f'(car (rn-read-all (float-text {raw})))', raw[1:]))
            source = work / 'roundtrip.rbel.md'
            source.write_text(chapter('numeric-roundtrip', roundtrips))
            invoke('float-print-read-boundaries', source,
                   extra=[ROOT / 'literate/reader.rbel.md'], timeout=300)
            invalid_tokens = ['1/0', '1/-2', '1/', '1.', '.5', '1e', '1e+', '1e--2', '1ff', '1.2.3', '0x10', '12abc', '1e4097']
            for index, token in enumerate(invalid_tokens):
                source = work / 'invalid.rbel.md'
                source.write_text(chapter('numeric-invalid', [('reject', token, 'nil')]))
                invoke(f'reject-literal-{index}', source, success=False)
            failures = [
                ('exact-zero-divisor', '(/ 1 0)'), ('float-zero-divisor', '(/ 1f 0f)'),
                ('mixed-add', '(+ 1 1f)'), ('mixed-sub', '(- 1f 1)'),
                ('mixed-mul', '(* 1f 1)'), ('mixed-div', '(/ 1 1f)'),
                ('mixed-equality', '(= 1 1f)'), ('mixed-order', '(< 1f 1)'),
                ('wrong-type', '(+ "1" 1)'), ('float-overflow', '(* 1e308f 2f)'),
                ('literal-overflow', '1e309f'), ('integer-text-fraction', '(integer-text 1/2)'),
                ('exact-rejects-infinity', '(exact \'(lit float "0111111111110000000000000000000000000000000000000000000000000000"))'),
            ]
            for label, expression in failures:
                source = work / 'failure.rbel.md'
                source.write_text(chapter('numeric-failure', [('reject', expression, 'nil')]))
                invoke(label, source, success=False)
            if not args.portable:
                source = work / 'numeric-command.rbel'
                source.write_text('(def numeric-command (ignored) (number-text (+ 999999999999999999999 1)))\n')
                output = work / 'numeric command'
                command = [str(compiler), 'build', str(source), '-o', str(output)]
                for include in includes:
                    command.extend(['--include', str(include)])
                build = subprocess.run(command, text=True, capture_output=True, timeout=240)
                if build.returncode:
                    raise AssertionError(('native-build', build.stdout[-4000:], build.stderr[-4000:]))
                execution = subprocess.run([str(output)], input='x', text=True, capture_output=True, env={'PATH': ''}, timeout=20)
                if execution.returncode or execution.stdout != '1000000000000000000000' or execution.stderr:
                    raise AssertionError(('native-execution', execution))
                report['native_execution_checked'] = True
                report['checks'].append({'name': 'native-standalone-output', 'passed': True, 'exit': 0})
                print('PASS native-standalone-output', flush=True)
        report['passed'] = True
    finally:
        args.report.write_text(json.dumps(report, indent=2)+'\n')
    print(f"{len(report['checks'])} numeric acceptance groups passed")


if __name__ == '__main__':
    main()
