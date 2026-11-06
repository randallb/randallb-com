// Public embedding adapter. Protocol decisions execute only inside core.wasm.
import {WasmValues} from '../../literate/wasm-values.mjs';

export class Core {
  constructor(bytes) {
    this.module = bytes instanceof WebAssembly.Module ? bytes : new WebAssembly.Module(bytes);
    if (WebAssembly.Module.imports(this.module).length) throw Error('libcwr core must have no imports');
  }
  transition(state, event, environment) {
    if (!this.module) throw Error('core is closed');
    // Values cross calls as data, never arena pointers. Recreate the bounded
    // instance each transition so a long-lived embedding does not fill its arena.
    const engine = new WasmValues(this.module);
    try {
      const result = engine.call([state, event, environment]);
      if (!Array.isArray(result) || result.length !== 3) throw Error('invalid libcwr transition result');
      return result;
    } finally {
      engine.destroy();
    }
  }
  close() { this.module = null; }
}
