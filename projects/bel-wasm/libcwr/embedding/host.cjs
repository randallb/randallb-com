// Persistence/action mechanisms for the compiled libcwr core. Protocol rules stay in Wasm.
const {readFileSync, writeFileSync, renameSync, existsSync, appendFileSync, mkdirSync, openSync, closeSync, unlinkSync} = require('node:fs');
const {join} = require('node:path');
const {createHash, randomUUID} = require('node:crypto');

const get = (record, key) => record?.find(entry => entry[0] === key)?.[1] ?? null;
const set = (record, key, value) => [...(record ?? []).filter(entry => entry[0] !== key), [key, value]];
const text = value => (value ?? []).map(c => {
  if (!c || typeof c.char !== 'string') throw Error('action input must be text characters');
  return c.char;
}).join('');
const chars = value => Array.from(value, char => ({char}));
const hash = value => createHash('sha256').update(value).digest('hex');
const ref = tail => `cwp://org.example.libcwr/${tail}`;
const target = [ref('runtime'), ref('worker'), ref('role'), 'store--text', ref('binding'), ref('contract')];

function fixtureEnvironment() {
  return [
    ['runtime', ref('runtime')], ['caller', ref('alice')], ['ingress', ref('ingress')],
    ['context', ref('workspace')], ['available', [ref('alice'), ref('worker')]], ['allow', [target]],
    ['binding', [['target', target], ['required', ['workspace.write']],
      ['grants', ['workspace.write']], ['capabilities', ['workspace.write']],
      ['route', ref('route-one')], ['repeat-safe', 't']]],
  ];
}

function lockDirectory(directory) {
  mkdirSync(directory, {recursive: true, mode: 0o700});
  const path = join(directory, '.libcwr-lock');
  let fd;
  try { fd = openSync(path, 'wx', 0o600); }
  catch (error) {
    if (error.code !== 'EEXIST') throw error;
    // Never take over an existing lock automatically: racing stale-lock
    // recovery can remove a new owner's lock. Recovery is an operator action.
    throw Error('state directory is locked; after confirming its owner is stopped, remove .libcwr-lock before recovery');
  }
  try { writeFileSync(fd, String(process.pid) + '\n'); } finally { closeSync(fd); }
  return () => unlinkSync(path);
}

function validateEnvironment(environment) {
  function record(value, name, required) {
    if (!Array.isArray(value)) throw Error(`${name} must be an association list`);
    const keys = new Set();
    for (const entry of value) {
      if (!Array.isArray(entry) || entry.length !== 2 || typeof entry[0] !== 'string' || keys.has(entry[0])) {
        throw Error(`${name} requires unique string keys and two-field entries`);
      }
      keys.add(entry[0]);
    }
    if (required.some(key => !keys.has(key))) throw Error(`${name} is missing a required field`);
  }
  const symbol = value => typeof value === 'string' && value.length > 0;
  const symbols = value => value === null || Array.isArray(value) && value.every(symbol);
  const targetValue = value => Array.isArray(value) && value.length === 6 && value.every(symbol);
  record(environment, 'trusted environment', ['runtime', 'caller', 'ingress', 'context', 'available', 'allow', 'binding']);
  if (!['runtime', 'caller', 'ingress', 'context'].every(key => symbol(get(environment, key)))
      || !symbols(get(environment, 'available'))) throw Error('trusted identity fields must contain symbols');
  const allowed = get(environment, 'allow');
  if (allowed !== null && (!Array.isArray(allowed) || !allowed.every(targetValue))) throw Error('allow must contain six-field targets');
  const binding = get(environment, 'binding');
  record(binding, 'binding', ['target', 'required', 'grants', 'capabilities', 'route', 'repeat-safe']);
  if (!targetValue(get(binding, 'target')) || !symbol(get(binding, 'route'))
      || !['required', 'grants', 'capabilities'].every(key => symbols(get(binding, key)))
      || ![null, 't'].includes(get(binding, 'repeat-safe'))) throw Error('invalid trusted binding structure');
}

function createSession({engineFactory, directory, environment, fixtures = false}) {
  if (typeof engineFactory !== 'function') throw Error('engineFactory is required');
  validateEnvironment(environment);
  // Host-authenticated claim, executor and recovery evidence never come from events.
  environment = environment.filter(entry => !['claim', 'executor', 'evidence', 'system'].includes(entry[0]));
  const unlock = lockDirectory(directory);
  let engine;
  const snapshot = join(directory, 'state.json');
  let state;
  try {
    engine = engineFactory();
    state = existsSync(snapshot) ? JSON.parse(readFileSync(snapshot, 'utf8')) : null;
  } catch (error) { unlock(); throw error; }
  let closed = false;
  let fault = null;
  let restarts = 0;
  const trace = [];
  const receiptPath = reference => join(directory, `receipt-${hash(reference)}.json`);
  const artifactPath = reference => join(directory, `artifact-${hash(reference)}.txt`);
  const atomic = (path, value) => {
    const temporary = `${path}.new`;
    writeFileSync(temporary, JSON.stringify(value) + '\n', {flag: 'w', mode: 0o600});
    renameSync(temporary, path);
  };
  const receipt = reference => existsSync(receiptPath(reference))
    ? JSON.parse(readFileSync(receiptPath(reference), 'utf8')) : null;
  function commit(next) {
    if (fault === 'fail-save') {
      fault = null;
      throw Error('injected snapshot commit failure');
    }
    atomic(snapshot, next);
    state = next;
  }
  function transition(event, trusted = environment) {
    if (closed) throw Error('session is closed');
    const value = engine.transition(state, event, trusted);
    if (!Array.isArray(value) || value.length !== 3) throw Error('bad transition result');
    const [next, response, effect] = value;
    commit(next);
    if (fixtures) trace.push({event: get(event, 'op'), response: response[0], effect: effect?.[0] ?? null});
    return {response, effect};
  }
  function complete(evidence, replacement = undefined) {
    const event = [['op', 'complete'], ['ref', get(evidence, 'ref')],
      ['claim', get(evidence, 'claim')], ['result', replacement ?? get(evidence, 'result')]];
    return transition(event, set(environment, 'executor', get(evidence, 'target'))).response;
  }
  function execute(invocation) {
    const reference = get(invocation, 'ref');
    const exact = get(invocation, 'target');
    const input = get(invocation, 'input');
    const body = text(input);
    const action = exact[3];
    let result = chars(hash(body));
    if (action === 'uppercase--text') {
      result = [];
      for (let offset = 0; offset < (input?.length ?? 0); offset += 256) {
        const value = engine.transition(null, [['op', 'uppercase-value'], ['input', input.slice(offset, offset + 256)]], null);
        if (value?.[1]?.[0] !== 'transformed') throw Error('compiled uppercase action failed');
        result.push(...(value[1][1] ?? []));
      }
      const temporary = artifactPath(reference) + '.new';
      writeFileSync(temporary, text(result), {mode: 0o600});
      renameSync(temporary, artifactPath(reference));
    } else if (action === 'store--text') {
      const temporary = artifactPath(reference) + '.new';
      writeFileSync(temporary, body, {mode: 0o600});
      renameSync(temporary, artifactPath(reference));
    } else if (action === 'append--text') {
      appendFileSync(artifactPath(reference), body, {mode: 0o600});
    } else {
      throw Error(`unsupported fixture action: ${action}`);
    }
    appendFileSync(join(directory, 'executions.jsonl'), JSON.stringify({reference, action, sha256: hash(body)}) + '\n');
    const evidence = [['ref', reference], ['claim', get(invocation, 'claim')],
      ['target', exact], ['input', input], ['result', result]];
    if (fault === 'lose-receipt') {
      fault = null;
      return null;
    }
    atomic(receiptPath(reference), evidence);
    return evidence;
  }
  const operations = {
    dispatch(event) {
      let trusted = set(environment, 'claim', ref(`claims/${randomUUID()}`));
      if (get(event, 'op') === 'reconcile') {
        let evidence = receipt(get(event, 'ref'));
        if (fault === 'tamper-receipt' && evidence) {
          evidence = set(evidence, 'input', chars('different'));
          fault = null;
        }
        trusted = set(trusted, 'evidence', evidence);
      }
      const {response, effect} = transition(event, trusted);
      if (!effect) return response;
      if (effect[0] !== 'execute' || effect.length !== 2) throw Error('unknown mechanism effect');
      if (fault === 'skip-execution') {
        fault = null;
        return response;
      }
      const evidence = execute(effect[1]);
      if (!evidence) return response;
      if (fault === 'lose-completion') {
        fault = null;
        return response;
      }
      return complete(evidence);
    },
    restart() {
      engine.close();
      engine = null;
      state = null;
      engine = engineFactory();
      state = existsSync(snapshot) ? JSON.parse(readFileSync(snapshot, 'utf8')) : null;
      restarts++;
      return transition([['op', 'recover']], set(environment, 'system', 't')).response;
    },
    configure(changes) {
      // Test fixture control, not a public ingress API. No event may set these.
      for (const [key, value] of changes ?? []) environment = set(environment, key, value);
      return null;
    },
    fault(value) {
      if (!['fail-save', 'skip-execution', 'lose-completion', 'lose-receipt', 'tamper-receipt'].includes(value)) {
        throw Error('unknown failure injection');
      }
      fault = value;
      return null;
    },
    snapshot() {
      return existsSync(snapshot) ? JSON.parse(readFileSync(snapshot, 'utf8')) : null;
    },
    redeliver(reference) {
      const evidence = receipt(reference);
      if (!evidence) throw Error('no durable receipt');
      return complete(evidence);
    },
    'redeliver-different'(reference) {
      const evidence = receipt(reference);
      if (!evidence) throw Error('no durable receipt');
      return complete(evidence, chars('attempted overwrite'));
    },
    observe(reference) {
      const executions = existsSync(join(directory, 'executions.jsonl'))
        ? readFileSync(join(directory, 'executions.jsonl'), 'utf8').trim().split('\n').filter(Boolean) : [];
      return [['executions', executions.map(() => 't')], ['restarts', Array(restarts).fill('t')],
        ['artifact', existsSync(artifactPath(reference)) ? chars(readFileSync(artifactPath(reference), 'utf8')) : null],
        ['receipt', receipt(reference)]];
    },
    'try-dispatch'(event) {
      try { return ['ok', operations.dispatch(event)]; }
      catch (error) { return ['host-error', chars(error.message)]; }
    },
  };
  return {
    dispatch: operations.dispatch,
    recover: operations.restart,
    async request(operation, argumentsValue) {
      if (!fixtures) throw Error('fixture operations are disabled');
      if (!Object.hasOwn(operations, operation)) throw Error(`unknown test host operation: ${operation}`);
      return operations[operation](argumentsValue);
    },
    evidence() { return {trace, restarts}; },
    close() {
      if (closed) return;
      closed = true;
      try { engine?.close(); state = null; } finally { unlock(); }
    },
  };
}

module.exports = {createSession, fixtureEnvironment};
