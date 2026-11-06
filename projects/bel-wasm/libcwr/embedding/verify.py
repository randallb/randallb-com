#!/usr/bin/env python3
"""Verify the same literate workflow through Wasm and a packaged native host."""
import argparse
import hashlib
import json
import os
import shutil
from pathlib import Path
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[2]
HERE = Path(__file__).resolve().parent


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--package', type=Path, default=ROOT / 'target/libcwr-embedding')
    parser.add_argument('--rbel', type=Path, default=ROOT / 'target/rbel-command/bin/rbel')
    args = parser.parse_args()
    package, rbel = args.package.resolve(), args.rbel.resolve()
    executable = package / 'libcwr'
    evidence = ROOT / 'target/libcwr-embedding-checks'
    evidence.mkdir(parents=True, exist_ok=True)
    report_path = evidence / 'verification.json'
    report_path.unlink(missing_ok=True)
    report = {'passed': False, 'groups': [], 'source_sha256': sha(ROOT / 'libcwr/workflow.rbel.md')}
    def check(name, truth):
        if not truth:
            raise AssertionError(name)
        report['groups'].append({'name': name, 'passed': True})
        print('PASS', name, flush=True)
    try:
        manifest = json.loads((package / 'manifest.json').read_text())
        # Require the complete maintained input set, not just self-consistent artifacts.
        input_names = {
            'libcwr/workflow.rbel.md', 'literate/wasm-values.mjs', 'literate/test.py',
            'LICENSE', 'NOTICE.txt', 'ORIGIN.json',
            *['libcwr/embedding/' + name for name in [
                'build.py', 'build-native.py', 'verify.py', 'regressions.py', 'core.mjs',
                'host.cjs', 'native-values.h', 'native-values.c', 'native-core.c', 'native-host.c', 'readme.md', 'native-build.md',
                'example-context.json', 'example-events.jsonl']],
        }
        def inputs_current():
            return set(manifest['inputs']) == input_names and all(
                sha(ROOT / name) == digest for name, digest in manifest['inputs'].items())
        check('maintained-input-correspondence', inputs_current())
        check('source-correspondence', manifest['source_sha256'] == report['source_sha256'])
        check('packaged-file-integrity', all(sha(package / name) == digest for name, digest in manifest['files'].items()))
        subprocess.run([sys.executable, str(HERE / 'regressions.py'), '--package', str(package)], check=True, timeout=60)
        check('packaging-regressions', True)
        native = json.loads((package / 'libcwr.json').read_text())
        check('native-core-and-executable-integrity', sha(package / 'core.wasm') == native['core_wasm_sha256'] and sha(executable) == native['executable_sha256'])
        check('native-source-correspondence', native['format'] == 'libcwr-native-wasm2c-v1' and set(native['native_source_sha256']) == {'native-values.h', 'native-values.c', 'native-core.c', 'native-host.c'} and all(sha(HERE / name) == digest for name, digest in native['native_source_sha256'].items()))
        report['core_wasm_sha256'] = sha(package / 'core.wasm')
        report['native_sha256'] = sha(executable)
        reports = []
        for mode, host, env_add in [
            ('baseline', 'libcwr/host.mjs', {}),
            ('wasm', 'libcwr/host.mjs', {'LIBCWR_CORE_WASM': str(package / 'core.wasm')}),
            ('native', 'libcwr/native-host.mjs', {'LIBCWR_NATIVE': str(executable)}),
        ]:
            path = evidence / f'{mode}.json'
            path.unlink(missing_ok=True)
            env = {k: v for k, v in os.environ.items() if k not in ['LIBCWR_CORE_WASM', 'LIBCWR_NATIVE', 'NODE_OPTIONS', 'NODE_PATH']}
            env.update(env_add)
            run = subprocess.run([sys.executable, str(ROOT / 'literate/test.py'), str(ROOT / 'libcwr/workflow.rbel.md'),
                '--entry', 'cw-engine', '--host', str(ROOT / host), '--rbel', str(rbel), '--report', str(path)],
                env=env, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=120)
            (evidence / f'{mode}.txt').write_text(run.stdout)
            result = json.loads(path.read_text()) if path.exists() else {}
            check(f'{mode}-adjacent-workflow', run.returncode == 0 and result.get('passed') and len(result['cases']) == 17
                  and sum(len(c['assertions']) for c in result['cases']) == 114)
            reports.append(result)
        def observable(result):
            return [(c['name'], c['assertions'], c.get('host')) for c in result['cases']]
        check('identical-assertions-and-host-traces', observable(reports[0]) == observable(reports[1]) == observable(reports[2]))
        script = '''
import {readFileSync} from 'node:fs';
const {Core} = await import(process.argv[1]);
const bytes=readFileSync(process.argv[2]);
const module=new WebAssembly.Module(bytes);
if(WebAssembly.Module.imports(module).length) throw Error('imports');
const core=new Core(bytes);
for(let i=0;i<1024;i++) core.transition(null, [['op','unknown']], []);
for(const bad of [42, {x:1}, {char:'ab'}, {char:'\\ud800'}]) {
  let failed=false; try {core.transition(bad, [], []);} catch {failed=true;}
  if(!failed) throw Error('bad transport accepted');
}
core.close(); let closed=false;try {core.transition(null,[],[]);}catch{closed=true;}
if(!closed) throw Error('closed core');
'''
        subprocess.run(['node', '--input-type=module', '-e', script, (package / 'core.mjs').as_uri(), str(package / 'core.wasm')], check=True, timeout=60)
        check('esm-import-free-bounds-and-instance-lifecycle', True)
        with tempfile.TemporaryDirectory(prefix='libcwr-bad-core-') as directory:
            bad = Path(directory)
            for name in ['core.mjs', 'values.mjs']:
                shutil.copy2(package / name, bad / name)
            (bad / 'core.wasm').write_bytes(b'not wasm')
            env = {k: v for k, v in os.environ.items() if k not in ['LIBCWR_CORE_WASM', 'LIBCWR_NATIVE']}
            env['LIBCWR_CORE_WASM'] = str(bad / 'core.wasm')
            run = subprocess.run([sys.executable, str(ROOT / 'literate/test.py'), str(ROOT / 'libcwr/workflow.rbel.md'),
                '--entry', 'cw-engine', '--host', str(ROOT / 'libcwr/host.mjs'), '--rbel', str(rbel)],
                env=env, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=60)
            check('broken-packaged-core-cannot-fall-back-to-test-module', run.returncode != 0)
        # Normal executable acceptance uses an empty PATH and hostile Node options.
        # The executable contains compiled C and ignores Node-specific variables,
        # and accepts events only, unlike the explicitly selected fixture interface.
        env = {k: v for k, v in os.environ.items() if k not in ['NODE_OPTIONS', 'NODE_PATH']}
        env.update(PATH='/nonexistent', NODE_OPTIONS='--require=/nonexistent/injection.cjs', NODE_PATH='/nonexistent')
        with tempfile.TemporaryDirectory(prefix='libcwr-executable-') as directory:
            work = Path(directory)
            standalone = work / 'libcwr'
            shutil.copy2(executable, standalone)
            context = json.loads((package / 'example-context.json').read_text())
            events = [json.loads(line) for line in (package / 'example-events.jsonl').read_text().splitlines()]
            context_path = work / 'context.json'
            context_path.write_text(json.dumps(context))
            def invoke(state, values, *, fixture=False, raw=None):
                command = [str(standalone), '--directory', str(state)]
                command += ['--test-host'] if fixture else ['--context', str(context_path)]
                return subprocess.run(command, input=raw if raw is not None else ''.join(json.dumps(v)+'\n' for v in values),
                    text=True, capture_output=True, env=env, timeout=30)
            state = work / 'ordinary'
            run = invoke(state, events)
            answers = [json.loads(line) for line in run.stdout.splitlines()]
            check('relocated-native-without-sidecars-or-external-tools', run.returncode == 0 and [v[0] for v in answers] == ['created', 'admitted', 'completed'])
            run = invoke(state, [events[2]])
            check('fresh-process-restores-completion-without-repeating-effect', run.returncode == 0 and json.loads(run.stdout)[0] == 'completed'
                and len((state / 'executions.jsonl').read_text().splitlines()) == 1)
            # Independent digest vectors cover C SHA256 padding and UTF-8/NUL transport.
            texts = ['', 'a' * 55, 'b' * 56, 'c' * 63, 'd' * 64, 'e' * 65,
                     'f' * 127, 'g' * 128, '\x00naïve 世界🙂\n']
            for index, body in enumerate(texts):
                changed = json.loads(json.dumps(events))
                for row in changed[1]:
                    if row[0] == 'input': row[1] = [{'char': c} for c in body]
                vector_state = work / f'digest-{index}'
                result = invoke(vector_state, changed)
                digest = hashlib.sha256(body.encode()).hexdigest()
                value = json.loads(result.stdout.splitlines()[-1]) if result.stdout else []
                artifact = vector_state / ('artifact-' + hashlib.sha256(dict(changed[2])['ref'].encode()).hexdigest() + '.txt')
                check(f'native-digest-and-text-vector-{index}', result.returncode == 0 and value[0] == 'completed'
                      and ''.join(c['char'] for c in dict(value[1])['result']) == digest
                      and artifact.read_bytes() == body.encode())
            for index, raw in enumerate([b'\xff\n', b'[["op",{"char":"a","char":"b"}]]\n',
                                         b'[' * 140 + b']' * 140 + b'\n']):
                result = subprocess.run([str(standalone), '--directory', str(work / f'bad-c-json-{index}'),
                                         '--context', str(context_path)], input=raw, capture_output=True, env=env, timeout=30)
                check(f'native-strict-json-vector-{index}', result.returncode != 0)
            controls = invoke(state, [{'operation': 'configure', 'arguments': [['caller', 'evil']]}])
            check('normal-interface-rejects-fixture-controls', controls.returncode != 0)
            reference = dict(events[2])['ref']
            def fixture(operation, value=None): return {'operation': operation, 'arguments': value}
            uncertain = work / 'uncertain'
            commands = [fixture('dispatch', events[0]), fixture('dispatch', events[1]), fixture('fault', 'lose-completion'), fixture('dispatch', events[2])]
            run = invoke(uncertain, commands, fixture=True)
            check('native-retains-interrupted-invocation', run.returncode == 0 and json.loads(run.stdout.splitlines()[-1])['value'][0] == 'running')
            reconcile = [['op', 'reconcile'], ['ref', reference]]
            run = invoke(uncertain, [reconcile, events[2]])
            check('fresh-process-reconciles-bound-receipt', run.returncode == 0 and [json.loads(l)[0] for l in run.stdout.splitlines()] == ['completed', 'completed']
                and len((uncertain / 'executions.jsonl').read_text().splitlines()) == 1)
            for name, raw in [('malformed-json', '{broken\n'), ('invalid-utf8-surrogate', '[["op", {"char":"\\ud800"}]]\n'),
                              ('oversized-line', 'x' * (2 * 1024 * 1024 + 1))]:
                run = invoke(work / name, [], raw=raw)
                check(f'native-rejects-{name}', run.returncode != 0)
            lock_state = work / 'locked'
            process = subprocess.Popen([str(standalone), '--directory', str(lock_state), '--context', str(context_path)], stdin=subprocess.PIPE,
                                       stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, env=env)
            try:
                for _ in range(200):
                    if (lock_state / '.libcwr-lock').exists(): break
                    if process.poll() is not None: break
                    time.sleep(0.01)
                other = invoke(lock_state, [])
                check('simultaneous-session-rejected', other.returncode != 0 and 'locked' in other.stderr)
                process.kill()
                process.communicate(timeout=5)
                other = invoke(lock_state, [])
                check('crashed-owner-lock-requires-explicit-operator-recovery', other.returncode != 0 and 'locked' in other.stderr)
                (lock_state / '.libcwr-lock').unlink()  # Isolated fixture; owner confirmed dead above.
                other = invoke(lock_state, [])
                check('disk-recovery-after-confirmed-dead-owner', other.returncode == 0)
            finally:
                if process.poll() is None:
                    process.kill(); process.communicate(timeout=5)
            license_run = subprocess.run([str(standalone), '--runtime-license'], capture_output=True, env=env, timeout=30)
            check('embedded-wabt-runtime-license', license_run.returncode == 0 and license_run.stdout == (package / 'libcwr.runtime-license.txt').read_bytes())
        check('maintained-inputs-remain-current', inputs_current())
        check('source-remains-current', report['source_sha256'] == sha(ROOT / 'libcwr/workflow.rbel.md'))
        report['passed'] = True
    except Exception as error:
        report['error'] = str(error)
        raise
    finally:
        report_path.write_text(json.dumps(report, indent=2) + '\n')
    print(f"Embedding acceptance: {len(report['groups'])} groups passed")


if __name__ == '__main__':
    main()
