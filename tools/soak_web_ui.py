#!/usr/bin/env python3
"""Repeat real-browser UI flows against isolated stores until a measured deadline."""
import argparse,datetime,hashlib,json,os,pathlib,re,subprocess,time

def main():
 p=argparse.ArgumentParser(description=__doc__);p.add_argument('--wrapper',type=pathlib.Path,required=True);p.add_argument('--url',action='append',required=True);p.add_argument('--browser',action='append',required=True);p.add_argument('--output',type=pathlib.Path,required=True);p.add_argument('--deadline',required=True);p.add_argument('--script',action='append');p.add_argument('--min-checks',type=int,default=10);p.add_argument('--session-prefix');a=p.parse_args()
 if len(a.url)!=len(a.browser):raise RuntimeError('One browser for each isolated URL is required')
 os.umask(0o077);root=a.output.resolve();root.mkdir(parents=True,exist_ok=False)
 deadline=datetime.datetime.fromisoformat(a.deadline.replace('Z','+00:00'));start=time.monotonic();rounds=0;checks=0;sessions=[]
 def emit(event,**fields):
  item={'timestamp':datetime.datetime.now(datetime.timezone.utc).isoformat(),'event':event,**fields};line=json.dumps(item);print(line,flush=True)
  with (root/'samples.jsonl').open('a') as f:f.write(line+'\n')
 def command(session,*args):
  r=subprocess.run([str(a.wrapper),'-s='+session,*args],text=True,capture_output=True,timeout=180)
  if r.returncode or '### Error' in r.stdout:raise RuntimeError(r.stdout[-3000:]+r.stderr[-1500:])
  return r.stdout
 templates=a.script or ['issues_global_profile_checks','issues_creation_feedback_checks','issues_order_browser_checks']
 prepared=[];last_suite={}
 try:
  for idx,(url,browser) in enumerate(zip(a.url,a.browser)):
   session=(a.session_prefix or 'soak-'+root.parent.name+'-'+root.name)+'-'+str(idx);sessions.append(session)
   command(session,'open','about:blank','--browser',browser)
   command(session,'goto',url)
   scripts=[]
   for name in templates:
    source=(pathlib.Path('tools')/(name+'.js')).read_text().strip().rstrip(';');source=source.replace('http://127.0.0.1:4782/',url.rstrip('/')+'/')
    source='async page => {page.removeAllListeners("pageerror");page.removeAllListeners("dialog");page.on("dialog",d=>d.accept().catch(()=>{}));return await ('+source+')(page);}'
    path=root/(pathlib.Path(name).name+'-'+str(idx)+'.js');path.write_text(source);scripts.append(path)
   prepared.append(scripts)
  emit('started',deadline=deadline.isoformat(),browsers=a.browser,urls=a.url,scripts={str(s):hashlib.sha256(s.read_bytes()).hexdigest() for group in prepared for s in group})
  while datetime.datetime.now(datetime.timezone.utc)<deadline:
   for idx,scripts in enumerate(prepared):
    for script in scripts:
     last_suite={'browser':a.browser[idx],'script':script.name,'round':rounds+1}
     (root/'running.json').write_text(json.dumps(last_suite)+'\n')
     output=command(sessions[idx],'run-code','--filename',str(script));m=re.search(r'### Result\n([^\n]+)',output)
     if not m:raise RuntimeError('No browser result: '+output[:1500])
     value=json.loads(m.group(1));passed=value.get('passed',0)
     if passed<a.min_checks:raise RuntimeError('Incomplete browser assertions: '+str(value))
     checks+=passed
     with (root/'results.jsonl').open('a') as f:f.write(json.dumps({'at':datetime.datetime.now(datetime.timezone.utc).isoformat(),'browser':a.browser[idx],'script':script.name,'result':value})+'\n')
   rounds+=1;emit('sample',rounds=rounds,assertions=checks,failures=0,elapsed_seconds=round(time.monotonic()-start,2))
  emit('passed',rounds=rounds,assertions=checks,failures=0,elapsed_seconds=round(time.monotonic()-start,2),deadline_reached=datetime.datetime.now(datetime.timezone.utc)>=deadline)
 except Exception as e:
  emit('failed',error=str(e),rounds=rounds,assertions=checks,elapsed_seconds=round(time.monotonic()-start,2),**last_suite)
  for idx,session in enumerate(sessions):
   try:
    state=command(session,'run-code','async page => ({url:page.url(),html:await page.locator("body").innerHTML(),state:await page.evaluate(()=>({ready:document.readyState,model:typeof model!=="undefined"?{route:model.route,project:model.project,error:model.error}:null,active:document.activeElement?.outerHTML,code:[...document.querySelectorAll("#comment-rendered pre")].map(e=>({scrollLeft:e.scrollLeft,scrollWidth:e.scrollWidth,clientWidth:e.clientWidth,tabIndex:e.tabIndex,rect:e.getBoundingClientRect().toJSON()}))}))})')
    (root/('failure-state-'+str(idx)+'.log')).write_text(state)
   except Exception as diagnostic:emit('diagnostic_error',session=session,error=str(diagnostic))
  raise
 finally:
  for session in sessions:
   try:command(session,'close')
   except Exception as error:emit('cleanup_error',session=session,error=str(error))
if __name__=='__main__':main()
