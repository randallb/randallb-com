// Bounded data transport for the existing rbel value ABI. No evaluation.
export class WasmValues {
  constructor(module) {
    if (WebAssembly.Module.imports(module).length) throw Error('test modules must have no imports');
    this.instance = new WebAssembly.Instance(module);
    this.exports = this.instance.exports;
    for (const name of ['choose', 'join', 'intern', 'symbolname', 'memory']) {
      if (!this.exports[name]) throw Error(`missing Wasm export: ${name}`);
    }
  }
  pairp(value) {
    return value >= 4096 && (value & 7) === 0 && value + 8 <= this.exports.memory.buffer.byteLength;
  }
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
  list(...values) {
    return values.reduceRight((tail, value) => this.exports.join(value, tail), 0);
  }
  encode(value, depth = 0, budget = {nodes: 0}) {
    if (++budget.nodes > 262144 || depth > 128) throw Error('input resource bound');
    if (value === null) return 0;
    if (typeof value === 'string') {
      if (Array.from(value).length > 4096) throw Error('symbol name bound');
      return this.exports.intern(this.encode(Array.from(value, char => ({char})), depth + 1, budget));
    }
    if (Array.isArray(value)) {
      return this.list(...value.map(v => this.encode(v, depth + 1, budget)));
    }
    if (value && typeof value === 'object' && Object.keys(value).length === 1) {
      if (typeof value.char === 'string' && Array.from(value.char).length === 1) {
        const cp = value.char.codePointAt(0);
        if (cp >= 0xd800 && cp <= 0xdfff) throw Error('invalid scalar');
        return (cp << 3) | 2;
      }
      if (Array.isArray(value.pair) && value.pair.length === 2) {
        return this.exports.join(...value.pair.map(v => this.encode(v, depth + 1, budget)));
      }
    }
    throw Error('unsupported transport value');
  }
  decode(value, depth = 0, budget = {nodes: 0}) {
    if (++budget.nodes > 262144 || depth > 128) throw Error('output resource bound');
    if (value === 0) return null;
    if (value === 1) return 't';
    if ((value & 7) === 2) {
      const cp = value >>> 3;
      if (cp > 0x10ffff || cp >= 0xd800 && cp <= 0xdfff) throw Error('invalid output scalar');
      return {char: String.fromCodePoint(cp)};
    }
    if ((value & 7) === 1) {
      const chars = this.fields(this.exports.symbolname(value), 4096);
      return chars.map(c => {
        if ((c & 7) !== 2) throw Error('invalid symbol transport');
        return String.fromCodePoint(c >>> 3);
      }).join('');
    }
    if (this.pairp(value)) {
      const values = [];
      while (value !== 0) {
        if (++budget.nodes > 262144) throw Error('output list resource bound');
        if (!this.pairp(value)) {
          return values.reduceRight((d, a) => ({pair: [a, d]}), this.decode(value, depth + 1, budget));
        }
        const view = new DataView(this.exports.memory.buffer);
        const head = view.getInt32(value, true);
        const tail = view.getInt32(value + 4, true);
        values.push(this.decode(head, depth + 1, budget));
        value = tail;
      }
      return values;
    }
    throw Error('invalid Wasm value');
  }
  call(value) {
    return this.decode(this.exports.choose(this.encode(['call', value])));
  }
  destroy() {
    this.instance = null;
    this.exports = null;
  }
}
