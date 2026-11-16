// Acceptance transport: the native executable owns its core and disk mechanisms.
import {spawn} from 'node:child_process';
import {StringDecoder} from 'node:string_decoder';

export async function createHost({directory}) {
  const executable = process.env.LIBCWR_NATIVE;
  if (!executable) throw Error('LIBCWR_NATIVE must name the packaged native executable');
  const child = spawn(executable, ['--test-host', '--directory', directory], {stdio: ['pipe', 'pipe', 'pipe']});
  const pending = [];
  let buffer = '';
  let stderr = '';
  let failure;
  let ending = false;
  let evidence = {trace: [], restarts: 0};
  const decoder = new StringDecoder('utf8');
  const fail = error => {
    failure ??= error;
    while (pending.length) pending.shift().reject(failure);
    if (!ending) child.kill();
  };
  child.on('error', fail);
  child.stdin.on('error', fail);
  child.stderr.on('data', chunk => { stderr = (stderr + chunk.toString()).slice(-8192); });
  child.stdout.on('data', chunk => {
    try {
      buffer += decoder.write(chunk);
      let index;
      while ((index = buffer.indexOf('\n')) !== -1) {
        const line = buffer.slice(0, index);
        buffer = buffer.slice(index + 1);
        if (line.length > 32 * 1024 * 1024) throw Error('native reply exceeds bound');
        const result = JSON.parse(line);
        const request = pending.shift();
        if (!request || !Object.hasOwn(result, 'value') || !result.evidence) throw Error('unexpected native reply');
        evidence = result.evidence;
        request.resolve(result.value);
      }
      if (buffer.length > 32 * 1024 * 1024) throw Error('native reply exceeds bound');
    } catch (error) { fail(error); }
  });
  const exited = new Promise(resolve => child.on('exit', (code, signal) => {
    if (code !== 0 || pending.length || !ending) fail(Error(`native host exited (${code ?? signal}): ${stderr.trim()}`));
    resolve();
  }));
  return {
    request(operation, argumentsValue) {
      if (failure) return Promise.reject(failure);
      if (ending) return Promise.reject(Error('native host is closed'));
      const text = JSON.stringify({operation, arguments: argumentsValue}) + '\n';
      if (Buffer.byteLength(text) > 2 * 1024 * 1024) return Promise.reject(Error('native request exceeds bound'));
      return new Promise((resolve, reject) => {
        pending.push({resolve, reject});
        child.stdin.write(text);
      });
    },
    evidence() { return evidence; },
    async close() {
      if (ending) return exited;
      ending = true;
      child.stdin.end();
      const timer = setTimeout(() => child.kill('SIGKILL'), 2000);
      try { await exited; } finally { clearTimeout(timer); }
      if (failure) throw failure;
    },
  };
}
