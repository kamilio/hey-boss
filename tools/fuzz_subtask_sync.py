#!/usr/bin/env python3
"""Seeded actual-CLI graph edits across disconnected native SQLite replicas."""
import argparse,json,os,pathlib,random,sqlite3,subprocess,time
import fleet_hey_boss as fleet
p=argparse.ArgumentParser(description=__doc__);p.add_argument('--binary',type=pathlib.Path,required=True);p.add_argument('--output',type=pathlib.Path,required=True);p.add_argument('--seed',type=int,default=91427);p.add_argument('--operations',type=int,default=1000);a=p.parse_args();root=a.output.resolve();root.mkdir(exist_ok=False,parents=True);fleet.BINARY=a.binary.resolve();rng=random.Random(a.seed);project='named:Graph fuzz';start=time.monotonic();checks=conflicts=syncs=0;dbs=[]
def emit(event,**fields):
 v={'event':event,'elapsed_seconds':round(time.monotonic()-start,2),**fields};print(json.dumps(v),flush=True)
 with (root/'samples.jsonl').open('a') as f:f.write(json.dumps(v)+'\n')
def cli(path,args):
 env=os.environ.copy();env['HEY_BOSS_ISSUE_DB']=str(path);env.pop('HEY_BOSS_ISSUE_HOST',None)
 r=subprocess.run([str(fleet.BINARY),'issue','--project','Graph fuzz','--agent','human:fuzz','--json',*args],env=env,cwd=root,text=True,capture_output=True,timeout=20)
 if r.returncode not in (0,2,4):raise RuntimeError(r.stdout+r.stderr)
 v=json.loads(r.stdout)
 if r.returncode and v['error']['code'] not in ('conflict','invalid_input'):raise RuntimeError(str(v))
 return r.returncode,v
def graph(db):return {r['child_number']:r['parent_number'] for r in db.execute('SELECT * FROM issue_subtasks WHERE project_id=?',(project,))}
def invariant(db):
 global checks
 g=graph(db);counts={}
 for child,parent in g.items():
  seen={child};n=parent;depth=1
  while n in g:
   if n in seen:raise RuntimeError('Cycle')
   seen.add(n);depth+=1;n=g[n]
  if n in seen or depth>8:raise RuntimeError('Invalid graph depth')
  counts[parent]=counts.get(parent,0)+1
 if any(c>100 for c in counts.values()):raise RuntimeError('Invalid child capacity')
 if db.execute('PRAGMA integrity_check').fetchone()[0]!='ok' or db.execute('PRAGMA foreign_key_check').fetchall():raise RuntimeError('SQLite integrity')
 checks+=1
try:
 main_path=root/'controller.db';assert cli(main_path,['create','--title','Root fixture'])[0]==0
 main=fleet.connect_db(main_path);dbs.append(main);original=dict(main.execute('SELECT * FROM issues').fetchone())
 with main:
  for number in range(2,61):fleet.put_row(main,'issues',{**original,'number':number,'title':f'Issue {number}','sort_order':number})
  main.execute('UPDATE projects SET next_number=61')
 fleet.install_capture(main,'controller','controller')
 nodes=[]
 for node in ['a','b','c']:
  path=root/(node+'.db');db=fleet.connect_db(path);dbs.append(db)
  with main:fleet.allocate(main,node,[{'id':node,'config':{'projects':[project],'concurrency':2,'tags':[],'enabled':True}}])
  main.backup(db);db.execute('DELETE FROM fleet_outbox');db.commit();fleet.install_capture(db,'agent',node)
  with main:payload=fleet.export_snapshot(main,node)
  with db:fleet.apply_pull(db,node,payload,[])
  nodes.append((node,path,db))
 def sync(index,lost_ack=False):
  global syncs,conflicts
  node,path,db=nodes[index];changes=fleet.journal(db)
  with main:receipts=fleet.accept_changes(main,node,changes)
  conflicts+=sum(r['state']=='conflict' for r in receipts)
  if lost_ack:
   with main:replay=fleet.accept_changes(main,node,changes)
   if [(r['seq'],r['state']) for r in replay]!=[(r['seq'],r['state']) for r in receipts]:raise RuntimeError('Receipt outcome changed')
   receipts=replay
  with main:payload=fleet.export_snapshot(main,node)
  with db:fleet.apply_pull(db,node,payload,receipts)
  invariant(main);invariant(db);syncs+=1
 emit('started',seed=a.seed,operations=a.operations,nodes=3)
 for operation in range(a.operations):
  index=rng.randrange(4);node,path,db=nodes[index] if index<3 else ('controller',main_path,main)
  g=graph(db)
  if g and rng.random()<.45:
   child=rng.choice(list(g));args=['subtask','remove',str(g[child]),str(child)]
  else:
   parent,child=rng.sample(range(1,61),2);args=['subtask','add',str(parent),str(child)]
  code,result=cli(path,args)
  with (root/'operations.jsonl').open('a') as f:f.write(json.dumps({'operation':operation,'node':node,'args':args,'exit':code})+'\n')
  invariant(db)
  if rng.random()<.30:sync(rng.randrange(3),rng.random()<.30)
  if operation%50==0:emit('sample',operations=operation+1,checks=checks,syncs=syncs,retained_conflicts=conflicts)
 # Drain bounded frames, then full authoritative snapshots settle deferred edges.
 for _ in range(100):
  for index in range(3):sync(index,True)
  if all(not fleet.journal(db) for _,_,db in nodes):break
 else:raise RuntimeError('Outboxes did not drain')
 for index in range(3):sync(index)
 canonical=graph(main)
 for node,path,db in nodes:
  if graph(db)!=canonical or db.execute('SELECT count(*) FROM fleet_deferred_subtasks').fetchone()[0]:raise RuntimeError('Replica failed to converge: '+node)
 emit('passed',operations=a.operations,checks=checks,syncs=syncs,retained_conflicts=conflicts,final_edges=len(canonical),seed=a.seed)
except BaseException as e:emit('failed',error=str(e),checks=checks,syncs=syncs,retained_conflicts=conflicts);raise
finally:
 for db in dbs:db.close()
