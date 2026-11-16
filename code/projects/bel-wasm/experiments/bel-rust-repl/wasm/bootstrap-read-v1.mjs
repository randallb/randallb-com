// Frozen ABI1 syntax transport for the pinned bootstrap reader. No evaluation.
import {readFileSync} from 'node:fs';
const [binary,path] = process.argv.slice(2);
try {
  const module = new WebAssembly.Module(readFileSync(binary));
  if (WebAssembly.Module.imports(module).length) throw Error('bootstrap reader must have no imports');
  const e = new WebAssembly.Instance(module).exports;
  const bytes = readFileSync(path);
  if (bytes.length > 524288) throw Error('bootstrap source exceeds512KiB');
  const source = new TextDecoder('utf-8',{fatal:true}).decode(bytes);
  let chars = 0;
  for (const c of Array.from(source).reverse()) chars = e.join((c.codePointAt(0)<<3)|2,chars);
  const result = e.choose(chars);
  let nodes = 0;
  const pair = v => v >=4096 && (v&7)===0 && v+8<=e.memory.buffer.byteLength;
  function decode(v,depth=0) {
    if (++nodes>1048576 || depth>256) throw Error('bootstrap syntax resource bound');
    if (v===0) return null;
    if ((v&7)===2) return {char:String.fromCodePoint(v>>>3)};
    if ((v&7)===1) {
      let out='', p=e.symbolname(v);
      while(p) {
        if (++nodes>1048576 || !pair(p)) throw Error('invalid bootstrap symbol');
        const view=new DataView(e.memory.buffer), c=view.getInt32(p,true);
        if((c&7)!==2) throw Error('invalid bootstrap symbol character');
        out+=String.fromCodePoint(c>>>3);p=view.getInt32(p+4,true);
      }
      return out;
    }
    const values=[];
    while(v!==0) {
      if (++nodes>1048576) throw Error('bootstrap list bound');
      if(!pair(v)) return values.reduceRight((d,a)=>({pair:[a,d]}),decode(v,depth+1));
      const view=new DataView(e.memory.buffer);
      const a=view.getInt32(v,true),d=view.getInt32(v+4,true);
      values.push(decode(a,depth+1));v=d;
    }
    return values;
  }
  process.stdout.write(JSON.stringify(decode(result))+'\n');
} catch(error) {console.error(`bootstrap reader: ${error.message}`);process.exitCode=1;}
