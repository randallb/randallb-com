// Acceptance fixture wrapper around the same reusable embedding host mechanisms.
import {readFileSync} from 'node:fs';
import {dirname, resolve, join} from 'node:path';
import {pathToFileURL} from 'node:url';
import mechanisms from './embedding/host.cjs';

export async function createHost({module, WasmValues, directory}) {
  let engineFactory;
  if (process.env.LIBCWR_CORE_WASM) {
    const {Core} = await import(pathToFileURL(join(dirname(resolve(process.env.LIBCWR_CORE_WASM)), 'core.mjs')));
    const bytes = readFileSync(process.env.LIBCWR_CORE_WASM);
    engineFactory = () => new Core(bytes);
  } else {
    engineFactory = () => {
      const values = new WasmValues(module);
      return {
        transition: (state, event, context) => values.call([state, event, context]),
        close: () => values.destroy(),
      };
    };
  }
  return mechanisms.createSession({engineFactory, directory,
    environment: mechanisms.fixtureEnvironment(), fixtures: true});
}
