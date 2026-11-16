// ABI2 value transport and explicit host roots. No language evaluation.
export class WasmValues {
  constructor(module, configuration = {}) {
    if (WebAssembly.Module.imports(module).length) throw Error('rbel modules must have no imports');
    this.instance = new WebAssembly.Instance(module);
    this.exports = this.instance.exports;
    this.tokens = new Set();
    for (const name of ['choose_args', 'join', 'intern', 'symbolname', 'memory', 'rbel_abi_version',
      'runtime_init', 'root_retain', 'root_release', 'pair_valid', 'value_kind', 'completion_status']) {
      if (!this.exports[name]) throw Error(`missing ABI2 Wasm export: ${name}`);
    }
    if (this.exports.rbel_abi_version() !== 2) throw Error('incompatible rbel value ABI');
    const {pairCells = 2097152, rootSlots = 262144, hostSlots = 65536,
      symbolSlots = 65536, callLimit = 512} = configuration;
    const status = this.exports.runtime_init(pairCells, rootSlots, hostSlots, symbolSlots, callLimit);
    if (status !== 0) throw Error(`rbel initialization failed (status ${status})`);
  }
  retain(value) {
    const kind = this.exports.value_kind(value);
    if (!kind) throw Error('invalid value to retain');
    // Immediate characters and nil cannot be collected; symbols can be weak.
    if (kind === 1 || kind === 3) {
      const handle = Object.freeze({owner:this, token:0n}); this.tokens.add(handle); return handle;
    }
    const token = this.exports.root_retain(value);
    if (token === 0n) throw Error('host root capacity exhausted or invalid value');
    const handle = Object.freeze({owner:this, token});
    this.tokens.add(handle);
    return handle;
  }
  release(handle) {
    if (!handle || handle.owner !== this) return false;
    if (!this.tokens.delete(handle)) return false;
    if (handle.token === 0n) return true;
    return this.exports.root_release(handle.token) === 1;
  }
  withRoots(values, operation) {
    const roots = [];
    try {
      for (const value of values) {
        const kind = this.exports.value_kind(value);
        if (kind !== 1 && kind !== 3) roots.push(this.retain(value));
      }
      const result = operation();
      if (result && typeof result.then === 'function') {
        throw Error('withRoots callback must be synchronous; retain across awaits explicitly');
      }
      return result;
    } finally {
      for (let i = roots.length - 1; i >= 0; --i) this.release(roots[i]);
    }
  }
  checkCompletion() {
    const status = this.exports.completion_status();
    if (status === 0) return;
    const diagnostic = {status, value:this.exports.error_value(), kind:this.exports.error_kind(),
      site:this.exports.error_site(), function:this.exports.error_function()};
    let payload = {raw:diagnostic.value};
    if (!this.formattingError) {
      this.formattingError = true;
      try { payload = this.decodeRooted(diagnostic.value,0,{nodes:0},true); } catch {}
      finally { this.formattingError = false; }
    }
    let source = '<ast>', name = String(diagnostic.function), line = 0;
    const text = (address,length) => {
      if (address + length > this.exports.memory.buffer.byteLength) return '<invalid diagnostic>';
      return new TextDecoder('utf-8',{fatal:true}).decode(new Uint8Array(this.exports.memory.buffer,address,length));
    };
    if (this.exports.diag_source_addr) {
      source = text(this.exports.diag_source_addr(diagnostic.function),this.exports.diag_source_len(diagnostic.function));
      name = text(this.exports.diag_function_addr(diagnostic.function),this.exports.diag_function_len(diagnostic.function));
      line = this.exports.diag_line(diagnostic.function);
    }
    const rendered = Array.isArray(payload) && payload.every(v => v && typeof v.char === 'string')
      ? payload.map(v=>v.char).join('') : JSON.stringify(payload);
    const error = Error(`${source}:${line || '?'}: ${name}: ${rendered} (form starts here; site ${diagnostic.site})`);
    error.rbel = {status:diagnostic.status,kind:diagnostic.kind,site:diagnostic.site,
      function:diagnostic.function,source,line,name,payload};
    throw error;
  }
  allocate(name, ...values) {
    const value = this.exports[name](...values);
    this.checkCompletion();
    return value;
  }
  choose(value) {
    return this.allocate('choose_args', this.list(value));
  }
  pairp(value) { return this.exports.pair_valid(value) === 1; }
  fields(value, limit = 32) {
    const fields = [];
    while (value !== 0) {
      if (fields.length >= limit || !this.pairp(value)) throw Error('invalid or bounded list');
      const view = new DataView(this.exports.memory.buffer);
      fields.push(view.getInt32(value, true));
      value = view.getInt32(value + 4, true);
    }
    return fields;
  }
  list(...values) { return this.listValues(values); }
  listValues(values) {
    return this.withRoots(values, () => {
      let tail = 0;
      for (let i = values.length - 1; i >= 0; --i) tail = this.allocate('join', values[i], tail);
      return tail;
    });
  }
  encode(value, depth = 0, budget = {nodes:0}) {
    if (++budget.nodes > 1048576 || depth > 128) throw Error('input resource bound');
    if (value === null) return 0;
    if (typeof value === 'string') {
      if (Array.from(value).length > 4096) throw Error('symbol name bound');
      return this.allocate('intern', this.encode(Array.from(value, char => ({char})), depth + 1, budget));
    }
    if (Array.isArray(value)) {
      const encoded = [], roots = [];
      try {
        for (const item of value) {
          const raw = this.encode(item, depth + 1, budget);
          const kind=this.exports.value_kind(raw);
          if (kind!==1 && kind!==3) roots.push(this.retain(raw));
          encoded.push(raw);
        }
        return this.listValues(encoded);
      } finally { for (const root of roots) this.release(root); }
    }
    if (value && typeof value === 'object' && Object.keys(value).length === 1) {
      if (typeof value.char === 'string' && Array.from(value.char).length === 1) {
        const cp = value.char.codePointAt(0);
        if (cp >= 0xd800 && cp <= 0xdfff) throw Error('invalid scalar');
        return (cp << 3) | 2;
      }
      if (Array.isArray(value.pair) && value.pair.length === 2) {
        const head = this.encode(value.pair[0], depth + 1, budget);
        return this.withRoots([head], () => this.allocate('join', head,
          this.encode(value.pair[1], depth + 1, budget)));
      }
    }
    throw Error('unsupported transport value');
  }
  decode(value, depth = 0, budget = {nodes:0}) {
    return this.withRoots([value], () => this.decodeRooted(value, depth, budget));
  }
  decodeRooted(value, depth, budget, diagnosticOnly = false) {
    if (++budget.nodes > 1048576 || depth > 128) throw Error('output resource bound');
    const kind = this.exports.value_kind(value);
    if (kind === 1) return null;
    if (kind === 3) return {char:String.fromCodePoint(value >>> 3)};
    if (kind === 2) {
      const name = diagnosticOnly ? this.exports.symbol_name_view(value) : this.allocate('symbolname', value);
      return this.fields(name, 4096).map(c => {
        if (this.exports.value_kind(c) !== 3) throw Error('invalid symbol transport');
        return String.fromCodePoint(c >>> 3);
      }).join('');
    }
    if (kind === 4) {
      const values = [];
      while (value !== 0) {
        if (++budget.nodes > 1048576) throw Error('output list resource bound');
        if (!this.pairp(value)) {
          return values.reduceRight((d,a) => ({pair:[a,d]}), this.decodeRooted(value, depth + 1, budget, diagnosticOnly));
        }
        const view = new DataView(this.exports.memory.buffer);
        const head = view.getInt32(value,true), tail = view.getInt32(value+4,true);
        values.push(this.decodeRooted(head,depth+1,budget,diagnosticOnly));
        value = tail;
      }
      return values;
    }
    throw Error('invalid Wasm value');
  }
  resume(continuation, reply) {
    return this.withRoots([continuation], () => {
      const tag = this.encode('resume');
      return this.withRoots([tag], () => {
        const answer = this.encode(reply);
        return this.choose(this.list(tag, continuation, answer));
      });
    });
  }
  call(value) { return this.decode(this.choose(this.encode(['call',value]))); }
  destroy() {
    if (this.exports) for (const root of [...this.tokens]) this.release(root);
    this.instance = null; this.exports = null;
  }
}
