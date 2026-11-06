// Runs compiled rbel continuations; assertion truth is computed in Wasm.
import {readFileSync, writeFileSync, mkdirSync, mkdtempSync, rmSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {join, dirname} from 'node:path';
import {pathToFileURL} from 'node:url';
import {WasmValues} from './wasm-values.mjs';

const [binary, manifest, ...options] = process.argv.slice(2);
const option = name => {
  const index = options.indexOf(name);
  return index < 0 ? null : options[index + 1];
};
const report = {cases: [], passed: false};
let directory;
try {
  const module = new WebAssembly.Module(readFileSync(binary));
  const input = JSON.parse(readFileSync(manifest, 'utf8'));
  const cases = input.cases;
  report.provenance = input.provenance;
  const hostModule = option('--host') ? await import(pathToFileURL(option('--host'))) : null;
  directory = mkdtempSync(join(tmpdir(), 'rbel-test-host-'));
  for (const test of cases) {
    const result = {name: test.name, source: test.path, line: test.line, assertions: [], passed: false};
    report.cases.push(result);
    const driver = new WasmValues(module);
    const stateDirectory = join(directory, test.name);
    mkdirSync(stateDirectory);
    const host = hostModule ? await hostModule.createHost({module, WasmValues, directory: stateDirectory}) : null;
    try {
      let value = driver.exports.choose(driver.encode(['test', test.name]));
      for (let step = 0; ; step++) {
        if (step >= 2048) throw Error('test step limit');
        const fields = driver.fields(value);
        const tag = driver.decode(fields[0]);
        if (tag === 'done' && fields.length === 1) break;
        if (tag !== 'await' || fields.length !== 4) throw Error('test must return await or done');
        const operation = driver.decode(fields[1]);
        const argumentsValue = driver.decode(fields[2]);
        let reply = null;
        if (operation === 'assert') {
          if (!Array.isArray(argumentsValue) || argumentsValue.length !== 4) throw Error('malformed assertion');
          const [label, truth, actual, expected] = argumentsValue;
          result.assertions.push({label, passed: truth === 't', actual, expected});
          if (options.includes('--verbose')) console.log(`  ${truth === 't' ? 'OK' : 'FAIL'} ${label}`);
          if (truth !== 't') throw Error(`assertion ${label}: expected ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}`);
        } else {
          if (!host) throw Error(`no host for operation ${operation}`);
          reply = await host.request(operation, argumentsValue);
        }
        value = driver.exports.choose(driver.list(driver.encode('resume'), fields[3], driver.encode(reply)));
      }
      if (!result.assertions.length) throw Error('test completed without an assertion');
      result.passed = true;
      if (host?.evidence) result.host = host.evidence();
      console.log(`PASS ${test.path}:${test.line} ${test.name} (${result.assertions.length} assertions)`);
    } catch (error) {
      result.error = error.message;
      if (host?.evidence) result.host = host.evidence();
      console.error(`FAIL ${test.path}:${test.line} ${test.name}: ${error.message}`);
      throw error;
    } finally {
      await host?.close?.();
      driver.destroy();
    }
  }
  report.passed = true;
  console.log(`Wasm: ${report.cases.length} cases, ${report.cases.reduce((n, c) => n + c.assertions.length, 0)} assertions passed`);
} catch (error) {
  report.error = error.message;
  process.exitCode = 1;
} finally {
  if (directory) rmSync(directory, {recursive: true, force: true});
  if (option('--report')) {
    mkdirSync(dirname(option('--report')), {recursive: true});
    writeFileSync(option('--report'), JSON.stringify(report, null, 2) + '\n');
  }
}
