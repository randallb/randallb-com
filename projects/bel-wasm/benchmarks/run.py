#!/usr/bin/env python3
"""Measure source-compiled rbel workloads. No language evaluator or timing pass gate."""
import argparse
from datetime import datetime, timezone
import hashlib
import json
import platform
from pathlib import Path
import statistics
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[1]
HERE = Path(__file__).resolve().parent
sys.path.insert(0,str(ROOT/'literate'))
from check import extract
CASES = ['tail-direct','tail-apply','allocation-churn','list-reverse','exact-arithmetic','utf8-roundtrip']
CONFIGURATION = dict(pairCells=32768,rootSlots=65536,hostSlots=4096,symbolSlots=1024,callLimit=128)


def digest(path): return hashlib.sha256(path.read_bytes()).hexdigest()

def run(command, **kwargs):
    start=time.perf_counter()
    result=subprocess.run(list(map(str,command)),timeout=600,**kwargs)
    if result.returncode:
        if result.stderr: print(result.stderr if isinstance(result.stderr,str) else result.stderr.decode(errors='replace'),file=sys.stderr)
        result.check_returncode()
    return result,(time.perf_counter()-start)*1000


def summarize(samples):
    times=[s['elapsed_ns']/s['iterations']/1e6 for s in samples]
    median=statistics.median(times)
    details=[]
    for sample in samples:
        before,after=sample['counters_before'],sample['counters_after']
        delta={name:after[name]-before[name] for name in ['live_cells','reclaimed_cells','collections','tail_transfers']}
        if any(delta[n]<0 for n in ['reclaimed_cells','collections','tail_transfers']):
            raise ValueError('runtime counter wrapped; reduce iterations')
        delta['allocated_cells']=delta['live_cells']+delta['reclaimed_cells']
        if delta['allocated_cells']<0: raise ValueError('inconsistent allocation counters')
        details.append({name:value/sample['iterations'] for name,value in delta.items()})
    return dict(median_ms=median,min_ms=min(times),max_ms=max(times),
        mad_ms=statistics.median(abs(t-median) for t in times),samples=len(times),
        median_per_operation={name:statistics.median(d[name] for d in details) for name in details[0]},
        instance_high_water={name:max(s['counters_after'][name] for s in samples)
            for name in ['max_roots','max_calls','total_bytes']})


def native_build(binary,work):
    tools=ROOT/'target/wabt-native-build';runtime=ROOT/'target/wabt-native-1.0.39/wasm2c'
    command=ROOT/'experiments/bel-rust-repl/wasm/command'
    _,translate=run([tools/'wasm2c',binary,'-n','bel','-o',work/'module.c'],capture_output=True)
    executable=work/'native-runner'
    flags=['-std=c99','-O2','-Wall','-Wextra','-Werror','-Wno-unused-function',
        '-Wno-unused-parameter','-Wno-unused-but-set-variable','-fno-optimize-sibling-calls',
        '-frounding-math','-DWASM_RT_USE_MMAP=0','-DWASM_RT_MEMCHECK_BOUNDS_CHECK=1',
        '-DWASM_RT_MEMCHECK_GUARD_PAGES=0','-DWASM_RT_STACK_DEPTH_COUNT=1',
        '-DWASM_RT_STACK_EXHAUSTION_HANDLER=0','-DWASM_RT_MAX_CALL_STACK_DEPTH=4096']
    _,compile_ms=run(['/usr/bin/clang',*flags,'-I',runtime,'-I',command,'-I',work,
        HERE/'native-runner.c',work/'module.c',runtime/'wasm-rt-impl.c',runtime/'wasm-rt-mem-impl.c',
        runtime/'wasm-rt-exceptions-impl.c','-lm','-pthread','-o',executable],capture_output=True)
    compiler=subprocess.check_output(['/usr/bin/clang','--version'],text=True).splitlines()[0]
    return executable,dict(translate_ms=translate,c_compile_ms=compile_ms,flags=flags,compiler=compiler)


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--rbel',type=Path,default=ROOT/'target/rbel-command/bin/rbel')
    parser.add_argument('--output',type=Path,default=ROOT/'target/benchmarks/baseline')
    parser.add_argument('--compare',type=Path,help='prior report.json, read only; incompatible runs are labelled')
    parser.add_argument('--warmup',type=int,default=2)
    parser.add_argument('--samples',type=int,default=7)
    parser.add_argument('--iterations',type=int,default=3)
    parser.add_argument('--native',action='store_true',help='also measure wasm2c on macOS ARM64')
    parser.add_argument('--case',action='append',choices=CASES)
    args=parser.parse_args()
    if not 0<=args.warmup<=100 or not 1<=args.samples<=100 or not 1<=args.iterations<=1000:
        parser.error('warmup must be 0..100; samples 1..100; iterations 1..1000')
    if args.native and (platform.system(),platform.machine())!=('Darwin','arm64'):
        parser.error('--native currently supports macOS ARM64')
    output=args.output.resolve()
    if not output.is_relative_to((ROOT/'target').resolve()): parser.error('output must be under project target')
    components=[HERE/'workloads.rbel',ROOT/'literate/integers.rbel.md',ROOT/'literate/utf8.rbel.md']
    compiler=args.rbel.resolve()
    if not (compiler.parent.parent/'libexec/rbel/wat2wasm').is_file():
        parser.error('compiler must be a complete relocatable rbel bundle')
    if output.is_relative_to(compiler.parent.parent): parser.error('output must be outside the compiler bundle')
    outputs=[output/name for name in ['report.json','report.md','workloads.rbel','workloads.wat',
        'workloads.wasm','request.json','module.c','module.h','native-runner']]
    protected=[*components,*HERE.rglob('*'),* (compiler.parent.parent).rglob('*')]
    protected=[p for p in protected if p.is_file()]
    if args.compare: protected.append(args.compare.resolve())
    def aliases(a,b): return a.resolve()==b.resolve() or a.exists() and b.exists() and a.samefile(b)
    for i,path in enumerate(outputs):
        if not path.resolve().is_relative_to(output) or any(aliases(path,p) for p in [*protected,*outputs[:i]]):
            parser.error('benchmark output aliases an input, tool, baseline or another output')
    baseline_bytes=args.compare.read_bytes() if args.compare else None
    baseline=json.loads(baseline_bytes) if baseline_bytes is not None else None
    if baseline is not None and (not isinstance(baseline,dict) or baseline.get('schema')!='rbel-benchmark-v1' or not isinstance(baseline.get('results'),list)):
        parser.error('comparison input must be a rbel-benchmark-v1 report')
    output.mkdir(parents=True,exist_ok=True)
    # Clear both human and machine success evidence after all alias validation.
    (output/'report.json').unlink(missing_ok=True)
    (output/'report.md').unlink(missing_ok=True)
    snapshots={p:p.read_bytes() for p in components}
    methods={name:(HERE/name).read_bytes() for name in ['run.py','run-wasm.mjs','native-runner.c']}
    source=output/'workloads.rbel'
    source.write_bytes(b'\n'.join(extract(data.decode())[0] if p.suffix=='.md' else data
                                  for p,data in snapshots.items()))
    wat,binary=output/'workloads.wat',output/'workloads.wasm'
    assembler=compiler.parent.parent/'libexec/rbel/wat2wasm'
    _,compile_ms=run([compiler,'emit',source,'-o',wat],capture_output=True)
    _,assemble_ms=run([assembler,wat,'-o',binary],capture_output=True)
    cases=list(dict.fromkeys(args.case or CASES))
    request=output/'request.json'
    request.write_text(json.dumps(dict(wasm=str(binary),configuration=CONFIGURATION,cases=cases,
        warmup=args.warmup,samples=args.samples,iterations=args.iterations)))
    execution,_=run(['node',HERE/'run-wasm.mjs',request],capture_output=True,text=True)
    wasm=json.loads(execution.stdout)
    environment={k:wasm[k] for k in ['engine','platform','arch','os_release','cpu','logical_cpus']}
    report=dict(schema='rbel-benchmark-v1',captured_at=datetime.now(timezone.utc).isoformat(),validated=True,configuration=CONFIGURATION,
        protocol=dict(warmup=args.warmup,samples=args.samples,iterations=args.iterations),environment=environment,
        sources={str(p.relative_to(ROOT)):hashlib.sha256(data).hexdigest() for p,data in snapshots.items()},
        cases=cases,workload_sha256=digest(HERE/'workloads.rbel'),
        harness_sha256={name:hashlib.sha256(data).hexdigest() for name,data in methods.items()},
        compiler_sha256=digest(compiler),wasm_sha256=digest(binary),source_sha256=digest(source),
        build=dict(source_to_wat_ms=compile_ms,assemble_ms=assemble_ms,module_compile_ms=wasm['module_compile_ms']),results=[])
    for result in wasm['results']:
        report['results'].append(dict(backend='wasm',**result,summary=summarize(result['samples'])))
    if args.native:
        executable,build=native_build(binary,output);report['build']['native']=build
        for name in cases:
            value,_=run([executable,name,args.warmup,args.samples,args.iterations],capture_output=True,text=True)
            result=json.loads(value.stdout)
            if result.get('validation') is not True: raise ValueError('native result not validated')
            report['results'].append(dict(backend='native',name=name,**result,summary=summarize(result['samples'])))
    if any((HERE/name).read_bytes()!=data for name,data in methods.items()): raise ValueError('benchmark harness changed during measurement')
    for p,data in snapshots.items():
        if p.read_bytes()!=data: raise ValueError('workload source changed during benchmark')
    if args.compare:
        if args.compare.read_bytes()!=baseline_bytes: raise ValueError('comparison baseline changed during measurement')
        report['comparison_baseline']=dict(sha256=hashlib.sha256(baseline_bytes).hexdigest(),
            compiler_sha256=baseline.get('compiler_sha256'),captured_at=baseline.get('captured_at'))
    for row in report['results']:
        row['comparison']=dict(status='no-baseline')
        if baseline:
            compatible=all(baseline.get(k)==report[k] for k in ['schema','configuration','protocol','environment','workload_sha256','harness_sha256','cases'])
            if row['backend']=='native': compatible=compatible and baseline.get('build',{}).get('native',{}).get('flags')==report['build']['native']['flags'] and baseline.get('build',{}).get('native',{}).get('compiler')==report['build']['native']['compiler']
            old=next((r for r in baseline.get('results',[]) if (r['backend'],r['name'])==(row['backend'],row['name'])),None)
            if compatible and old and old['summary']['median_ms']>0:
                row['comparison']=dict(status='comparable',median_change_percent=100*(row['summary']['median_ms']/old['summary']['median_ms']-1))
            else: row['comparison']=dict(status='incompatible')
    (output/'report.json').write_text(json.dumps(report,indent=2)+'\n')
    lines=['# rbel benchmark results','',f"Validated; {args.samples} samples × {args.iterations} calls after {args.warmup} warmups.",'',
        '| Backend | Workload | Median ms/call | MAD ms | Allocated cells/call | Collections/call | Change |',
        '| --- | --- | ---: | ---: | ---: | ---: | ---: |']
    for row in report['results']:
        s=row['summary'];c=row['comparison'];change=f"{c['median_change_percent']:+.1f}%" if c['status']=='comparable' else c['status']
        lines.append(f"| {row['backend']} | {row['name']} | {s['median_ms']:.3f} | {s['mad_ms']:.3f} | {s['median_per_operation']['allocated_cells']:.1f} | {s['median_per_operation']['collections']:.2f} | {change} |")
    lines += ['', 'Build wall times (single observations, not sample medians):', '',
        f'- Source to WAT: {compile_ms:.1f} ms', f'- Assembly: {assemble_ms:.1f} ms',
        f"- Wasm module compilation: {wasm['module_compile_ms']:.1f} ms",
        '', 'Execution includes automatic GC; status checks, validation, setup and preparation collection are excluded.',
        'See report.json for raw samples, configuration, phase times, counter snapshots and provenance.']
    (output/'report.md').write_text('\n'.join(lines)+'\n')
    print('\n'.join(lines));print(f'\nReport: {output / "report.json"}')

if __name__=='__main__':main()
