#!/usr/bin/env python3
"""Stress independent CLI workers against one isolated durable issue queue."""
import argparse,json,os,pathlib,shutil,signal,sqlite3,subprocess,time
p=argparse.ArgumentParser(description=__doc__);p.add_argument('--cli',type=pathlib.Path,required=True);p.add_argument('--output',type=pathlib.Path,required=True);p.add_argument('--workers',type=int,default=10);p.add_argument('--issues',type=int,default=120);a=p.parse_args()
os.umask(0o077);root=a.output.resolve();root.mkdir(exist_ok=False,parents=True);cli=a.cli.resolve();fixture=root/'codex-fixture.mjs';shutil.copy2('tests/fixtures/codex-worker.mjs',fixture);fixture.chmod(0o700)
env=dict(os.environ,HEY_BOSS_ISSUE_DB=str(root/'issues.db'),HEY_BOSS_STATE_DIR=str(root/'state'),HEY_BOSS_FLEET_STATE=str(root/'fleet'),HEY_BOSS_CODEX=str(fixture),HEY_BOSS_TEST_CLI=str(cli));env.pop('HEY_BOSS_ISSUE_HOST',None)
(root/'mode.txt').write_text('completed');workers=[];handles=[];peak=0;peaks={};started=time.monotonic()
def issue(*args):
 r=subprocess.run([str(cli),'issue','--project','Worker fixture','--agent','human:contention','--json',*args],env=env,cwd=root,text=True,capture_output=True,timeout=30)
 if r.returncode:raise RuntimeError(r.stderr+' '+r.stdout)
 return json.loads(r.stdout)
try:
 for n in range(a.issues):issue('create','--title','Ready '+str(n),'--body','Synthetic contention test','--label','ready')
 parked=issue('create','--title','Tag control','--body','Remain open','--label','parked')['issue']['number']
 boss=issue('create','--title','Boss control','--body','Remain open','--label','ready')['issue']['number'];issue('assign-to-boss',str(boss))
 issue('settings','set','--prompt','/goal Assign and implement `{{issue_command}}`. {{commit_instruction}}')
 for n in range(a.workers):
  h=(root/('worker-'+str(n)+'.log')).open('w');handles.append(h)
  workers.append(subprocess.Popen([str(cli),'worker','--project','Worker fixture','--concurrency','2','--tag','ready','--name','Contention '+str(n),'--json'],env=env,cwd=root,stdout=h,stderr=h))
 deadline=time.monotonic()+180
 with sqlite3.connect(root/'issues.db') as db:
  while True:
   assert all(w.poll() is None for w in workers),'Worker unexpectedly exited'
   active=dict(db.execute('SELECT worker_id,count(*) FROM worker_runs WHERE finished_at IS NULL GROUP BY worker_id'));peak=max(peak,sum(active.values()))
   for key,value in active.items():peaks[key]=max(peaks.get(key,0),value);assert value<=2,active
   closed=db.execute("SELECT count(*) FROM issues WHERE state='closed'").fetchone()[0]
   if closed==a.issues:break
   if time.monotonic()>deadline:raise RuntimeError('Timed out with '+str(closed)+' completed issues')
   time.sleep(.1)
  assert not db.execute('SELECT issue_number,count(*) FROM worker_runs GROUP BY issue_number HAVING count(*)<>1').fetchall(),'Duplicate reservations'
  assert db.execute('SELECT count(*) FROM worker_runs').fetchone()[0]==a.issues
  assert not db.execute("SELECT 1 FROM worker_runs WHERE state<>'completed' OR claimed_at IS NULL OR session_id IS NULL").fetchall()
  assert db.execute("SELECT state FROM issues WHERE number=?",(parked,)).fetchone()[0]=='open'
  assert db.execute("SELECT state,assignee FROM issues WHERE number=?",(boss,)).fetchone()==('open','human:boss')
  assert db.execute('PRAGMA integrity_check').fetchone()[0]=='ok';assert not db.execute('PRAGMA foreign_key_check').fetchall()
 result={'passed':True,'workers':a.workers,'capacity_per_worker':2,'issues_completed':a.issues,'peak_simultaneous_slots':peak,'worker_peaks':peaks,'elapsed_seconds':round(time.monotonic()-started,2),'duplicate_reservations':0,'manual_claims_verified':a.issues,'goal_activation_verified':sum('thread/goal/set' in line and 'active' in line for line in (root/'protocol.jsonl').read_text().splitlines())}
 assert result['goal_activation_verified']==a.issues,result
 (root/'result.json').write_text(json.dumps(result,indent=2)+'\n');print(json.dumps(result),flush=True)
finally:
 for worker in workers:
  if worker.poll() is None:worker.send_signal(signal.SIGTERM)
 for worker in workers:
  try:worker.wait(timeout=10)
  except subprocess.TimeoutExpired:worker.kill();worker.wait()
 for handle in handles:handle.close()
