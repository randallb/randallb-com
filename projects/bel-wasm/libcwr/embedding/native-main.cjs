// Standalone host for the embedded Wasm core. Only Node builtins are runtime dependencies.
const {readFileSync} = require('node:fs');
const {resolve} = require('node:path');
const {once} = require('node:events');
const sea = require('node:sea');
const {Core} = require('./core.cjs');
const {createSession, fixtureEnvironment} = require('./host.cjs');

const MAX_LINE = 2 * 1024 * 1024;
const decoder = new TextDecoder('utf-8', {fatal: true});

function options(argv) {
  const result = {};
  for (let i = 0; i < argv.length; i++) {
    const key = argv[i];
    if (key === '--test-host' || key === '--help' || key === '--runtime-license') {
      if (result[key]) throw Error(`duplicate option ${key}`);
      result[key] = true;
    } else if (['--directory', '--context', '--wasm'].includes(key)) {
      if (result[key] || !argv[i + 1] || argv[i + 1].startsWith('--')) throw Error(`invalid option ${key}`);
      result[key] = argv[++i];
    } else throw Error(`unknown option ${key}`);
  }
  return result;
}

async function main() {
  const args = options(process.argv.slice(2));
  if (args['--runtime-license']) {
    if (!sea.isSea()) throw Error('--runtime-license is available in the packaged executable');
    process.stdout.write(sea.getAsset('node-license.txt', 'utf8'));
    return;
  }
  if (args['--help']) {
    console.log('Usage: libcwr --directory STATE --context TRUSTED.json\nReads one event as JSON per line; writes one protocol response per line.\n--test-host enables isolated acceptance fixture controls.\nDevelopment only: --wasm FILE selects the core outside a packaged executable.');
    return;
  }
  if (!args['--directory']) throw Error('--directory is required');
  if (args['--test-host'] && args['--context']) throw Error('--test-host cannot be combined with --context');
  if (!args['--test-host'] && !args['--context']) throw Error('--context is required');
  if (sea.isSea() && args['--wasm']) throw Error('packaged executable uses its embedded core');
  const bytes = sea.isSea() ? sea.getAsset('core.wasm')
    : readFileSync(args['--wasm'] || resolve(__dirname, 'core.wasm'));
  const environment = args['--test-host'] ? fixtureEnvironment()
    : JSON.parse(decoder.decode(readFileSync(args['--context'])));
  const session = createSession({directory: resolve(args['--directory']),
    engineFactory: () => new Core(bytes), environment, fixtures: Boolean(args['--test-host'])});
  const stop = signal => { session.close(); process.exit(signal === 'SIGINT' ? 130 : 143); };
  const interrupt = () => stop('SIGINT');
  const terminate = () => stop('SIGTERM');
  process.on('SIGINT', interrupt);
  process.on('SIGTERM', terminate);
  try {
    if (!args['--test-host']) session.recover();
    async function line(data) {
      if (data.length > MAX_LINE) throw Error('request exceeds line bound');
      const value = JSON.parse(decoder.decode(data));
      let response;
      if (args['--test-host']) {
        if (!value || Array.isArray(value) || typeof value.operation !== 'string'
            || Object.keys(value).some(k => !['operation', 'arguments'].includes(k))) {
          throw Error('fixture request must contain operation and optional arguments');
        }
        response = {value: await session.request(value.operation, value.arguments ?? null), evidence: session.evidence()};
      } else {
        response = session.dispatch(value);
      }
      const output = JSON.stringify(response) + '\n';
      if (Buffer.byteLength(output) > 16 * MAX_LINE) throw Error('response exceeds line bound');
      if (!process.stdout.write(output)) await once(process.stdout, 'drain');
    }
    let pending = Buffer.alloc(0);
    for await (const chunk of process.stdin) {
      pending = Buffer.concat([pending, chunk]);
      let newline;
      while ((newline = pending.indexOf(10)) !== -1) {
        const data = pending.subarray(0, newline);
        pending = pending.subarray(newline + 1);
        await line(data);
      }
      if (pending.length > MAX_LINE) throw Error('request exceeds line bound');
    }
    if (pending.length) await line(pending);
  } finally {
    process.removeListener('SIGINT', interrupt);
    process.removeListener('SIGTERM', terminate);
    session.close();
  }
}

main().catch(error => {
  console.error(`libcwr: ${error.message}`);
  process.exitCode = 1;
});
