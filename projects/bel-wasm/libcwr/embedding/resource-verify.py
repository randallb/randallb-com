#!/usr/bin/env python3
"""Acceptance of a directly loaded ordinary rbel coworker program."""
import argparse, hashlib, json, os, re, select, shutil, subprocess, tempfile, time
from pathlib import Path
ROOT=Path(__file__).resolve().parents[2]
def record(value): return dict(value or [])
def text(value): return ''.join(c['char'] for c in value or [])
def chars(value): return [{'char':c} for c in value]

def main():
 p=argparse.ArgumentParser();p.add_argument('--executable',type=Path,default=ROOT/'target/libcwr-embedding/libcwr');p.add_argument('--rbel',type=Path,default=ROOT/'target/resource-command/bin/rbel');p.add_argument('--report',type=Path,default=ROOT/'target/resource-verification/results.json');a=p.parse_args()
 report={'passed':False,'groups':[],'transcripts':[],'executable_sha256':hashlib.sha256(a.executable.read_bytes()).hexdigest()}
 def check(name,condition):
  if not condition: raise AssertionError(name)
  report['groups'].append(name);print('PASS',name,flush=True)
 class Session:
  def __init__(self,source,state):
   self.process=subprocess.Popen([str(a.executable.resolve()),'workspace','--directory',str(state),'--resource',str(source),'--rbel',str(a.rbel.resolve())],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.STDOUT,bufsize=0);self.output=b'';self.offset=0
  def until(self,expected,timeout=150):
   marker=expected.encode();deadline=time.monotonic()+timeout
   while marker not in self.output[self.offset:]:
    if time.monotonic()>deadline: raise AssertionError(('timeout',expected,self.output[-7000:]))
    if select.select([self.process.stdout],[],[],.1)[0]:
     chunk=os.read(self.process.stdout.fileno(),65536);self.output+=chunk
     if not chunk and self.process.poll() is not None: raise AssertionError(('exit',self.process.returncode,self.output[-7000:]))
   end=self.output.index(marker,self.offset)+len(marker);out=self.output[self.offset:end];self.offset=end;return out.decode(errors='replace')
  def send(self,line): self.process.stdin.write((line+'\n').encode());self.process.stdin.flush()
  def command(self,line,prompt='you[demo]>'): self.send(line);return self.until(prompt)
  def close(self):
   if self.process.poll() is None:self.send('/exit')
   rest,_=self.process.communicate(timeout=20);self.output+=rest;report['transcripts'].append(self.output.decode(errors='replace'));assert self.process.returncode==0,self.output[-4000:]
 session=None
 try:
  with tempfile.TemporaryDirectory(prefix='rbel-resource-') as temp:
   work=Path(temp);source=work/'looper.rbel.md';original=(ROOT/'libcwr/coworkers/looper.rbel.md').read_text()
   # Preserve relative ordinary module import in an isolated editable copy.
   shutil.copytree(ROOT/'libcwr',work/'libcwr');source=work/'libcwr/coworkers/looper.rbel.md';state=work/'state'
   def snapshot():return record(json.loads((state/'state.json').read_text()))
   def invocations():return [record(x) for x in snapshot().get('invocations') or [] if not record(x).get('control')]
   def room():return next(record(x) for x in snapshot().get('conversations') or [] if record(x)['ref'].endswith('/demo'))
   def call(topic='Plan',notes='First step',expected='Brief: Plan'):
    session.send('/call looper draft--brief '+json.dumps({'topic':topic,'notes':notes}));out=session.until('looper [demo]: ');out+=session.until('you[demo]>');check('typed-result-displayed-'+str(len(report['groups'])),expected in out);return next(i for i in invocations() if record(i.get('result')).get('brief'))
   session=Session(source,state);session.until('workspace>')
   check('program-descriptor-loaded', '"draft--brief"' in session.command('/resource','workspace>'))
   check('program-action-discoverable','draft--brief' in session.command('/actions','workspace>'))
   session.command('/new demo');before=len(invocations());session.command('/invite looper');check('invitation-does-not-start-action',len(invocations())==before)
   check('member-identity',len(room()['participants'])==2)
   before=len(invocations());out=session.command('/call looper draft--brief '+json.dumps({'topic':'','notes':'x'}));check('empty-required-text-rejected','invalid' in out and len(invocations())==before)
   for invalid in [{'topic':'x'},{'topic':'x','notes':'y','extra':'z'},{'topic':[],'notes':'y'}]:
    out=session.command('/call looper draft--brief '+json.dumps(invalid));check('interface-rejects-'+str(len(report['groups'])),'invalid' in out and len(invocations())==before)
   first=call();identity=first['target'];check('exact-target-from-resource',identity[3]=='draft--brief' and '/coworkers/resource-' in identity[1]);check('typed-input-and-output-retained',text(record(first['input'])['topic'])=='Plan' and text(record(first['result'])['brief'])=='Brief: Plan\nFirst step')
   messages=[record(x) for x in room()['messages']];reply=next(m for m in messages if m.get('invocation')==first['ref'] and m.get('reply'));request=next(m for m in messages if m['ref']==reply['reply']);check('authorship-and-causal-links',request['author']!=reply['author'] and reply['author']==identity[1]);check('reply-retains-typed-value',record(reply.get('value')).get('brief')==record(first['result'])['brief'])
   session.send('/call looper draft--brief {"topic":"Queue1","notes":"One"}');session.send('/call looper draft--brief {"topic":"Queue2","notes":"Two"}');session.send('/status');session.until('Brief: Queue2');session.until('you[demo]>');check('queued-actions-survive-control-binding-switch',all(i.get('phase')=='completed' for i in invocations()))
   session.send('@uppercase still works');session.until('uppercase [demo]: STILL WORKS');session.until('you[demo]>');check('builtin-coexists-with-resource',True)
   source.write_text(original.replace('Brief: ','Updated: '));out=session.command('/check');check('check-does-not-activate','Source checks passed' in out);call(expected='Brief: Plan')
   out=session.command('/reload');check('valid-reload-activated','Reloaded coworker source' in out);updated=call(expected='Updated: Plan');check('identity-stable-after-edit',updated['target']==identity)
   source.write_text(original.replace('(id looper)', '(id replacement)'));out=session.command('/reload');check('changed-identity-candidate-rejected','failed' in out);call(expected='Updated: Plan')
   source.write_text(original+"\n```rbel example=broken\n(looper '(describe))\n=>\nnil\n```\n");out=session.command('/reload');check('failing-example-preserves-active','failed' in out);call(expected='Updated: Plan')
   source.write_text(original);session.close();session=None
   # Move entire authored tree, leaving relative imports valid.
   moved=work/'moved';shutil.move(work/'libcwr',moved);source=moved/'coworkers/looper.rbel.md';session=Session(source,state);session.until('workspace>');session.command('/open demo');reopened=call();check('identity-and-membership-survive-move-restart',reopened['target']==identity and identity[1] in room()['participants']);check('history-preserved',any(i['ref']==first['ref'] for i in invocations()));session.close();session=None
   environment=record(json.loads((state/'context.json').read_text()));row=next(record(x) for x in environment['workspace-actions'] if record(x).get('name')=='draft--brief')
   trusted=work/'trusted.json'
   def raw(events,allowed=True):
    env=dict(environment,allow=[row['target']] if allowed else [],binding=list(row.items()));trusted.write_text(json.dumps(list(env.items())))
    launch=[str(a.executable.resolve()),'--directory',str(state),'--context',str(trusted),'--resource',str(source),'--rbel',str(a.rbel.resolve())]
    run=subprocess.run(launch,input=''.join(json.dumps(list(e.items()))+'\n' for e in events),text=True,capture_output=True,timeout=150)
    report['transcripts'].append({'events':events,'stdout':run.stdout,'stderr':run.stderr,'exit':run.returncode});assert run.returncode==0,run.stderr
    return [json.loads(line) for line in run.stdout.splitlines()]
   event={'op':'admit','ref':environment['context']+'/invocations/raw','origin':environment['context']+'/requests/raw','conversation':room()['ref'],'message':environment['context']+'/messages/raw','response-message':environment['context']+'/replies/raw','target':row['target'],'input':[['topic',chars('Raw')],['notes',chars('Caller')]],'text':chars('raw request')}
   before=(state/'state.json').read_bytes();answers=raw([event],False);check('denied-raw-caller-no-execution',answers[0][0]=='denied' and before==(state/'state.json').read_bytes())
   answers=raw([event,{'op':'run','ref':event['ref']}]);check('authorized-raw-structured-execution',answers[0][0]=='admitted' and answers[1][0]=='completed' and text(record(record(answers[1][1])['result'])['brief'])=='Brief: Raw\nCaller')
   before=(state/'state.json').read_bytes();answers=raw([{'op':'run','ref':event['ref']}]);check('completed-raw-replay-retained',answers[0][0]=='completed' and before==(state/'state.json').read_bytes())
   # Ordinary programs may supply their own entry and lie about handler output.
   # The trusted runtime must still enforce the retained interface.
   malicious=re.sub(r'```rbel example=.*?```', '', original, flags=re.S)
   malicious=malicious.replace('(def looper (request) (resource-program (looper-definition) request))', '(def looper (request) (if (id (car request) \'describe) (resource-program (looper-definition) request) \'(ok ((wrong "bad")))))')
   source.write_text(malicious)
   bad=dict(event,ref=environment['context']+'/invocations/invalid-output',origin=environment['context']+'/requests/invalid-output',message=environment['context']+'/messages/invalid-output',**{'response-message':environment['context']+'/replies/invalid-output'})
   execution_path=state/'executions.jsonl';prior_executions=execution_path.read_bytes() if execution_path.exists() else b''
   answers=raw([bad,{'op':'run','ref':bad['ref']}]);failed=next(i for i in invocations() if i['ref']==bad['ref']);check('raw-invalid-output-retains-failure',answers[1][0]=='reconciliation-required' and failed['phase']=='reconciliation-required' and failed.get('error'));check('invalid-output-no-execution-receipt',execution_path.read_bytes()==prior_executions)
   session=Session(source,state);session.until('workspace>');session.command('/open demo');session.send('/call looper draft--brief {"topic":"Bad","notes":"Output"}');out=session.until('reconciliation-required');session.until('you[demo]>');check('terminal-invalid-output-ends-active-attempt','reconciliation-required' in out);session.close();session=None;source.write_text(original)
   # Reopening and reloading must never restore an exact target grant removed by the owner.
   revoked=dict(environment,allow=[x for x in environment['allow'] if x!=row['target']]);(state/'context.json').write_text(json.dumps(list(revoked.items())))
   session=Session(source,state);session.until('workspace>');session.command('/open demo');session.command('/reload');before=len(invocations());out=session.command('/call looper draft--brief {"topic":"Denied","notes":"No grant"}');check('revoked-grant-survives-reload-restart','denied' in out and len(invocations())==before);session.close();session=None
   check('source-loader-no-toml',not list(work.rglob('*.toml')))
  report['passed']=True
 finally:
  if session and session.process.poll() is None:session.process.kill();session.process.wait()
  a.report.parent.mkdir(parents=True,exist_ok=True);a.report.write_text(json.dumps(report,indent=2)+'\n')
if __name__=='__main__':main()
