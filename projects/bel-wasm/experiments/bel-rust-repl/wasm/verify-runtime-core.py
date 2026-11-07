#!/usr/bin/env python3
"""Exercise the emitted ABI2 runtime independently of compiler lowering.

The source function is a balanced tree of literal WAT fragments. This test
concatenates only those fragments; it never evaluates language expressions.
Compiler/self-hosting acceptance separately executes the actual emitter.
"""
import argparse
import json
from pathlib import Path
import re
import subprocess
import tempfile

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]


def literal_runtime(source):
    # Recognize just strings and the fixed sc-l2 concatenation skeleton. Escapes
    # follow the rbel reader: a backslash quotes the next character literally.
    chars, strings, outside = [], [], []
    index = 0
    while index < len(source):
        if source[index] == ';':
            index = source.find('\n', index)
            if index < 0:
                break
        elif source[index] == '"':
            index += 1
            chars = []
            while index < len(source) and source[index] != '"':
                if source[index] == '\\':
                    index += 1
                chars.append(source[index])
                index += 1
            if index == len(source):
                raise ValueError('unterminated runtime fragment')
            strings.append(''.join(chars))
            outside.append(' STRING ')
            index += 1
        else:
            outside.append(source[index])
            index += 1
    skeleton = ''.join(outside)
    if re.sub(r'[()\s]|def|sc-runtime-part-[0-9]+|sc-runtime|sc-l2|STRING', '', skeleton):
        raise ValueError('runtime source is not a literal concatenation tree')
    return ''.join(strings)


FIXTURE = r'''
(global $probe-code (mut i32) (i32.const 0))
(global $probe-fn (mut i32) (i32.const 0))
(global $probe-fail-code (mut i32) (i32.const 0))
(global $probe-fail-fn (mut i32) (i32.const 0))
(func $generated-static-end (result i32) (i32.const 4096))
(func $generated-roots (call $mark (global.get $probe-code)) (call $mark (global.get $probe-fn)) (call $mark (global.get $probe-fail-code)) (call $mark (global.get $probe-fail-fn)))
(func $generated-init (result i32)
 (global.set $probe-code (call $join (i32.const 0) (i32.const 0)))
 (global.set $probe-fn (call $closure (global.get $probe-code) (i32.const 0)))
 (global.set $probe-fail-code (call $join (i32.const 0) (i32.const 0)))
 (global.set $probe-fail-fn (call $closure (global.get $probe-fail-code) (i32.const 0)))
 (if (result i32) (global.get $error-pending) (then (i32.const 2)) (else (i32.const 0))))
(func $dispatch (param $code i32) (param $env i32) (param $args i32) (result i32)
 (local $base i32) (local $xs i32) (local $next i32) (local $status i32)
 (if (i32.eq (local.get $code) (global.get $probe-fail-code)) (then
  (global.set $active-function (i32.const 99)) (global.set $active-site (i32.const 77))
  (drop (call $raise (i32.const 1) (global.get $error-user))) (return (i32.const 2))))
 (if (local.get $env) (then
  (global.set $active-function (i32.const 88)) (global.set $active-site (i32.const 66))
  (return (call $invoke (global.get $probe-fail-fn) (i32.const 0)))))
 (local.set $base (call $frame-enter (i32.const 2)))
 (if (i32.lt_s (local.get $base) (i32.const 0)) (then (return (i32.const 2))))
 (call $root-set (local.get $base) (i32.const 0) (local.get $args))
 (local.set $xs (call $car (local.get $args)))
 (call $root-set (local.get $base) (i32.const 1) (local.get $xs))
 (if (local.get $xs) (then
   (local.set $next (call $join (call $cdr (local.get $xs)) (i32.const 0)))
   (global.set $transfer-fn (global.get $probe-fn)) (global.set $transfer-args (local.get $next))
   (local.set $status (i32.const 1)))
  (else (global.set $output (i32.const 1)) (local.set $status (i32.const 0))))
 (call $frame-leave (local.get $base))
 (if (result i32) (global.get $error-pending) (then (i32.const 2)) (else (local.get $status))))
(func (export "probe_trace") (result i32)
 (drop (call $public-begin))
 (drop (call $invoke (call $closure (global.get $probe-code) (i32.const 1)) (i32.const 0)))
 (call $public-end (i32.const 0)))
(func (export "probe_literal") (param $pointer i32) (param $length i32) (result i32)
 (drop (call $public-begin)) (call $public-end (call $literal-text (local.get $pointer) (local.get $length))))
(func (export "probe_fn") (result i32) (global.get $probe-fn))
(func (export "probe_error") (param $x i32) (result i32)
 (drop (call $public-begin)) (drop (call $err (local.get $x))) (call $public-end (i32.const 0)))
(func (export "probe_roots") (result i32)
 (drop (call $public-begin)) (drop (call $frame-enter (i32.const 2147483647)))
 (call $public-end (i32.const 0)))
(func (export "probe_fp_add") (param $a i32) (param $b i32) (result i32)
 (drop (call $public-begin)) (call $public-end (call $fp-add (local.get $a) (local.get $b))))
(func (export "probe_char_code") (param $x i32) (result i32)
 (drop (call $public-begin)) (call $public-end (call $char-code (local.get $x))))
(func (export "probe_code_char") (param $x i32) (result i32)
 (drop (call $public-begin)) (call $public-end (call $code-char (local.get $x))))
'''

SCRIPT = r'''
import {readFileSync} from 'node:fs';
import assert from 'node:assert/strict';
const module = new WebAssembly.Module(readFileSync(process.argv[2]));
assert.deepEqual(WebAssembly.Module.imports(module), []);
const checks=[];
const pass=name=>{ checks.push(name); console.log('PASS '+name); };
function instance(pairs=4096,roots=1024,hosts=128,symbols=128,calls=32){
 const e=new WebAssembly.Instance(module).exports;
 assert.equal(e.rbel_abi_version(),2);
 assert.equal(e.runtime_init(pairs,roots,hosts,symbols,calls),0);
 return e;
}
function retain(e,v){const t=e.root_retain(v);assert.notEqual(t,0n);return t;}
function pair(e,a,d){const v=e.join(a,d);assert.equal(e.completion_status(),0);return v;}
function text(e,s){let out=0;for(const c of Array.from(s).reverse())out=pair(e,(c.codePointAt(0)<<3)|2,out);return out;}
function sym(e,s){const v=e.intern(text(e,s));assert.equal(e.completion_status(),0);return v;}
function encode(e,value){
 if(value===null)return 0;
 if(typeof value==='string')return sym(e,value);
 if(value.char)return (value.char.codePointAt(0)<<3)|2;
 const values=[],tokens=[];
 try{
  for(const item of value){const v=encode(e,item);values.push(v);tokens.push(retain(e,v));}
  let out=0;for(const v of values.reverse())out=pair(e,v,out);return out;
 }finally{for(const t of tokens)e.root_release(t);}
}
function chars(e,value){let s='',p=value,n=0;while(p){assert.equal(e.pair_valid(p),1);assert.ok(++n<10000);const d=new DataView(e.memory.buffer);s+=String.fromCodePoint(d.getInt32(p,true)>>>3);p=d.getInt32(p+4,true);}return s;}
function fields(e,value){const out=[];let p=value;while(p){assert.equal(e.pair_valid(p),1);const d=new DataView(e.memory.buffer);out.push(d.getInt32(p,true));p=d.getInt32(p+4,true);}return out;}
{
 const e=instance();const initial=e.stat_live_cells();
 const held=pair(e,1,0),t=retain(e,held);
 for(let i=0;i<30000;i++)pair(e,1,0);
 assert.ok(e.stat_collections()>0);assert.ok(e.stat_reclaimed_cells()>20000);
 assert.equal(e.pair_valid(held),1);assert.equal(e.root_release(t),1);assert.equal(e.root_release(t),0);
 pair(e,0,0);e.collect();assert.equal(e.pair_valid(held),0);assert.ok(e.stat_live_cells()<initial+10);
 assert.equal(e.stat_root_top(),0);pass('bounded-heap-collection-and-host-retention');
}
{
 const e=instance();const live=sym(e,'retained-symbol'), t=retain(e,live);
 const dead=sym(e,'unreferenced-symbol');
 for(let i=0;i<2000;i++)sym(e,'temporary-'+i);
 e.collect();assert.equal(sym(e,'retained-symbol'),live);assert.equal(e.value_kind(dead),0);
 assert.ok(e.stat_live_symbols()<40);assert.equal(e.root_release(t),1);
 pass('weak-symbol-descriptors-preserve-only-live-identities');
}
{
 const e=instance();const x=pair(e,1,0),t=retain(e,x);
 new DataView(e.memory.buffer).setInt32(x+4,x,true);
 e.collect();assert.equal(e.pair_valid(x),1);assert.equal(e.root_release(t),1);
 pair(e,0,0);e.collect();assert.equal(e.pair_valid(x),0);pass('iterative-cyclic-marking');
}
{
 const e=instance(4096,1024,8192);const ts=[];
 while(true){const x=e.join(1,0);if(e.completion_status()===2)break;ts.push(retain(e,x));}
 assert.equal(e.error_value(),65);assert.ok(ts.length>3000);assert.equal(e.stat_root_top(),0);
 for(const t of ts)e.root_release(t);assert.ok(pair(e,1,0));
 e.probe_roots();assert.equal(e.completion_status(),2);assert.equal(e.error_value(),73);assert.equal(e.stat_root_top(),0);
 assert.ok(pair(e,1,0));pass('heap-and-root-exhaustion-are-recoverable');
}
{
 const e=instance(4096,1024,2);const x=pair(e,1,0),t=retain(e,x);
 const y=pair(e,0,0),u=retain(e,y);e.probe_error(x);assert.equal(e.completion_status(),2);
 assert.equal(e.root_retain(1),0n);assert.equal(e.error_value(),x);assert.equal(e.completion_status(),2);
 assert.equal(e.root_release(t),1);assert.equal(e.error_value(),x);assert.equal(e.root_release(t),0);
 assert.equal(e.root_release(u),1);assert.ok(pair(e,1,0));pass('root-management-preserves-language-errors');
}
{
 const e=instance(65536,1024,128,128,8);let xs=0;
 for(let i=0;i<50000;i++)xs=pair(e,1,xs);
 const t=retain(e,xs),args=pair(e,xs,0),u=retain(e,args);
 assert.equal(e.invoke_value(e.probe_fn(),args),1);assert.equal(e.completion_status(),0);
 assert.equal(e.stat_max_calls(),1);assert.equal(e.stat_tail_transfers(),50000);assert.equal(e.stat_root_top(),0);
 assert.ok(e.stat_collections()>0);e.root_release(t);e.root_release(u);pass('tail-trampoline-bounded-call-and-root-depth');
}
{
 const e=instance();const code=e.probe_char_code((0x1f642<<3)|2),t=retain(e,code);
 const f=fields(e,code);assert.equal(chars(e,f[3]),'128578');assert.equal(chars(e,f[4]),'1');
 assert.equal(e.probe_code_char(code),(0x1f642<<3)|2);assert.equal(e.completion_status(),0);
 const surrogate=encode(e,['lit','num','+',Array.from('55296',char=>({char})),[{char:'1'}]]);
 e.probe_code_char(surrogate);assert.equal(e.completion_status(),2);assert.equal(e.error_value(),97);
 e.root_release(t);pass('unicode-scalar-and-exact-integer-conversion');
}
{
 const e=instance();const a=encode(e,['lit','float',Array.from('0011111111110000000000000000000000000000000000000000000000000000',char=>({char}))]);const ta=retain(e,a);
 const b=encode(e,['lit','float',Array.from('0100000000000000000000000000000000000000000000000000000000000000',char=>({char}))]);const tb=retain(e,b);
 const answer=e.probe_fp_add(a,b);assert.equal(e.completion_status(),0);
 assert.equal(chars(e,fields(e,answer)[2]),'0100000000001000000000000000000000000000000000000000000000000000');
 e.root_release(ta);e.root_release(tb);pass('binary64-primitive-preserves-bit-pair-contract');
}
{
 const e=instance();e.probe_trace();assert.equal(e.completion_status(),2);
 assert.equal(e.error_function(),99);assert.equal(e.error_site(),77);assert.equal(e.error_frame_count(),2);
 assert.equal(e.error_frame_function(0),88);assert.equal(e.error_frame_site(0),66);
 assert.equal(e.error_frame_function(1),99);assert.equal(e.error_frame_site(1),77);
 assert.equal(e.stat_root_top(),0);pair(e,1,0);assert.equal(e.error_frame_count(),0);
 e.invoke_value(e.probe_fn(),1);assert.equal(e.completion_status(),2);assert.equal(e.error_value(),57);
 assert.equal(e.stat_root_top(),0);pass('error-origin-trace-and-malformed-invocation-recovery');
}
{
 const e=instance();const good='AΩ世界🙂';let bytes=new TextEncoder().encode(good);
 new Uint8Array(e.memory.buffer,2048,bytes.length).set(bytes);
 assert.equal(chars(e,e.probe_literal(2048,bytes.length)),good);assert.equal(e.completion_status(),0);
 const old=sym(e,'format-error');e.probe_error(old);const before=[e.completion_status(),e.error_value(),e.error_kind()];
 assert.equal(chars(e,e.symbol_name_view(old)),'format-error');assert.deepEqual([e.completion_status(),e.error_value(),e.error_kind()],before);
 for(const bad of [[0x80],[0xc0,0x80],[0xc2],[0xe0,0x80,0x80],[0xed,0xa0,0x80],
                   [0xf0,0x80,0x80,0x80],[0xf4,0x90,0x80,0x80],[0xf5,0x80,0x80,0x80],[0xe2,0x82],[0xc2,0xa0,0x80]]){
  new Uint8Array(e.memory.buffer,2048,bad.length).set(bad);e.probe_literal(2048,bad.length);
  assert.equal(e.completion_status(),2);assert.equal(e.error_value(),97);assert.equal(e.stat_root_top(),0);
 }
 e.probe_literal(0xffffffff,8);assert.equal(e.completion_status(),2);assert.equal(e.error_value(),97);
 for(let i=0;i<3000;i++)pair(e,1,0);const gc=e.stat_collections();bytes=new TextEncoder().encode('x'.repeat(2500));
 new Uint8Array(e.memory.buffer,0,bytes.length).set(bytes);
 assert.equal(chars(e,e.probe_literal(0,bytes.length)),'x'.repeat(2500));assert.ok(e.stat_collections()>gc);
 assert.equal(e.stat_root_top(),0);pass('static-utf8-literals-rooted-and-error-name-view-nonallocating');
}
{
 const e=new WebAssembly.Instance(module).exports;
 assert.equal(e.runtime_init(0xffffffff,1024,128,128,8),2);assert.notEqual(e.management_status(),0);
 const f=instance();assert.ok(f.stat_total_bytes()<67108864);assert.equal(f.runtime_init(4096,1024,128,128,8),2);
 assert.equal(f.pair_valid(4096),0);assert.equal(f.value_kind(0x110000*8+2),0);pass('initialization-and-value-bounds');
}

{
 for(const count of [1025,4095]){
  const e=instance(count);const x=pair(e,1,0),t=retain(e,x);
  assert.equal(x&7,0);e.collect();assert.equal(e.pair_valid(x),1);
  assert.equal(e.root_release(t),1);assert.equal(e.stat_root_top(),0);
 }
 for(const calls of [513,2048,4096]){
  const e=new WebAssembly.Instance(module).exports;
  assert.equal(e.runtime_init(4096,1024,128,128,calls),2);
  assert.notEqual(e.management_status(),0);
 }
 pass('odd-heap-capacities-align-and-unsafe-call-limits-rejected');
}
{
 const e=instance();
 // Simulate the final generation without four billion retain/release calls.
 // This fixture's generated-static-end is4096 and root capacity is1024.
 const hostBase=4096+4*1024;
 new DataView(e.memory.buffer).setUint32(hostBase+4,0xffffffff,true);
 const t=retain(e,1);assert.equal(BigInt.asUintN(64,t)>>32n,0xffffffffn);
 assert.equal(e.root_release(t),1);assert.equal(e.root_release(t),0);
 const next=retain(e,1);assert.equal(BigInt.asUintN(64,next)&0xffffffffn,2n);
 assert.equal(e.root_release(next),1);
 pass('generation-wrap-retires-slot');
}
{
 const e=instance(1024);e.collect();let pressure=0;
 while(e.stat_live_cells()<1024-25)pressure=pair(e,1,pressure);
 const held=retain(e,pressure),name=text(e,'abcdefghijklmno'),named=retain(e,name);
 const before=e.stat_live_symbols();e.intern(name);
 assert.equal(e.completion_status(),2);assert.equal(e.error_value(),65);
 assert.equal(e.stat_root_top(),0);assert.equal(e.stat_live_symbols(),before);
 assert.equal(e.root_release(held),1);e.collect();const id=e.intern(name);
 assert.equal(e.completion_status(),0);assert.equal(chars(e,e.symbol_name_view(id)),'abcdefghijklmno');
 assert.equal(e.stat_live_symbols(),before+1);assert.equal(e.root_release(named),1);
 pass('interrupted-intern-copy-releases-partials-and-descriptor');
}

console.log(JSON.stringify({passed:true,checks}));
'''


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--wat2wasm', type=Path, default=ROOT/'target/bel-wasm-tools/node_modules/.bin/wat2wasm')
    parser.add_argument('--report', type=Path, default=ROOT/'target/runtime-core/results.json')
    args = parser.parse_args()
    body = literal_runtime((HERE/'core-runtime.bel').read_text())
    args.report.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='rbel-runtime-') as directory:
        work = Path(directory)
        source, binary, script = work/'test.wat', work/'test.wasm', work/'test.mjs'
        source.write_text('(module\n(memory (export \"memory\") 1 1024)\n'+body+FIXTURE+'\n)')
        script.write_text(SCRIPT)
        subprocess.run([str(args.wat2wasm.resolve()), str(source), '-o', str(binary)], check=True)
        run = subprocess.run(['node', str(script), str(binary)], text=True, capture_output=True)
        print(run.stdout, end='')
        if run.returncode:
            print(run.stderr, end='')
            raise SystemExit(run.returncode)
        args.report.write_text(json.dumps(json.loads(run.stdout.splitlines()[-1]),indent=2)+'\n')


if __name__ == '__main__':
    main()
