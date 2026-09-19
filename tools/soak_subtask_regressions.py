#!/usr/bin/env python3
"""Repeat isolated graph/reconnect and real fake-Codex worker checks to a deadline."""
import argparse,datetime,json,os,pathlib,re,subprocess,time
p=argparse.ArgumentParser(description=__doc__);p.add_argument('--source',type=pathlib.Path,required=True);p.add_argument('--binary',type=pathlib.Path,required=True);p.add_argument('--worker-tests',type=pathlib.Path,required=True);p.add_argument('--output',type=pathlib.Path,required=True);p.add_argument('--deadline',required=True);a=p.parse_args();root=a.output.resolve();root.mkdir(exist_ok=False,parents=True)
deadline=datetime.datetime.fromisoformat(a.deadline.replace('Z','+00:00'));started=time.monotonic();rounds=checks=0
def emit(event,**fields):
 v={'timestamp':datetime.datetime.now(datetime.timezone.utc).isoformat(),'event':event,**fields};print(json.dumps(v),flush=True)
 with (root/'samples.jsonl').open('a') as f:f.write(json.dumps(v)+'\n')
env=os.environ.copy();env['HEY_BOSS_TEST_BINARY']=str(a.binary.resolve());env.pop('HEY_BOSS_ISSUE_HOST',None)
emit('started',deadline=deadline.isoformat(),source=str(a.source.resolve()),binary=str(a.binary.resolve()))
try:
 while datetime.datetime.now(datetime.timezone.utc)<deadline:
  t=time.monotonic();rounds+=1
  for name,command in [('fleet',['python3','-m','unittest','discover','-s','tools','-p','test_fleet_subtasks.py']),('upgrade',['python3','-m','unittest','discover','-s','tools','-p','test_upgrade_hey_boss.py']),('worker',[str(a.worker_tests.resolve()),'parallel_worker_refreshes_subtask_readiness'])]:
   r=subprocess.run(command,cwd=a.source.resolve(),env=env,text=True,capture_output=True,timeout=180);(root/(name+'-'+str(rounds)+'.log')).write_text(r.stdout+r.stderr)
   if r.returncode:raise RuntimeError(name+' failed in round '+str(rounds)+': '+(r.stdout+r.stderr)[-3500:])
   count=re.search(r'Ran (\d+) tests',r.stderr) if name!='worker' else re.search(r'test result: ok\. (\d+) passed',r.stdout)
   if not count:raise RuntimeError('Missing test result: '+name)
   checks+=int(count.group(1))
  emit('sample',rounds=rounds,assertions=checks,failures=0,elapsed_seconds=round(time.monotonic()-started,2))
  remaining=(deadline-datetime.datetime.now(datetime.timezone.utc)).total_seconds()
  time.sleep(max(0,min(30-(time.monotonic()-t),remaining)))
 emit('passed',rounds=rounds,assertions=checks,failures=0,elapsed_seconds=round(time.monotonic()-started,2),deadline_reached=True)
except BaseException as e:emit('failed',error=str(e),rounds=rounds,assertions=checks,elapsed_seconds=round(time.monotonic()-started,2));raise
