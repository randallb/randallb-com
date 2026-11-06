// Newly authored deterministic test host. Mechanisms only; cw-step owns decisions.
import {readFileSync, writeFileSync, renameSync, existsSync, appendFileSync} from 'node:fs';
import {join} from 'node:path';
import {createHash, randomUUID} from 'node:crypto';

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

export async function createHost({module, WasmValues, directory}) {
  let engine = new WasmValues(module);
  const snapshot = join(directory, 'state.json');
  let state = null;
  let environment = [
    ['runtime', ref('runtime')], ['caller', ref('alice')], ['ingress', ref('ingress')],
    ['context', ref('workspace')], ['available', [ref('alice'), ref('worker')]], ['allow', [target]],
    ['binding', [['target', target], ['required', ['workspace.write']],
      ['grants', ['workspace.write']], ['capabilities', ['workspace.write']],
      ['route', ref('route-one')], ['repeat-safe', 't']]],
  ];
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
    const value = engine.call([state, event, trusted]);
    if (!Array.isArray(value) || value.length !== 3) throw Error('bad transition result');
    const [next, response, effect] = value;
    commit(next);
    trace.push({event: get(event, 'op'), response: response[0], effect: effect?.[0] ?? null});
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
    if (action === 'store--text') {
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
      ['target', exact], ['input', input], ['result', chars(hash(body))]];
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
      engine.destroy();
      engine = null;
      state = null;
      engine = new WasmValues(module);
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
    async request(operation, argumentsValue) {
      if (!Object.hasOwn(operations, operation)) throw Error(`unknown test host operation: ${operation}`);
      return operations[operation](argumentsValue);
    },
    evidence() { return {trace, restarts}; },
    close() { engine?.destroy(); state = null; },
  };
}
