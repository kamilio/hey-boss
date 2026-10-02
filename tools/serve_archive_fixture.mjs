// Real desktop and phone archive fixture. Stop with Ctrl+C to remove private data.
import {mkdtempSync, rmSync, existsSync, realpathSync, copyFileSync} from 'node:fs';
import {DatabaseSync} from 'node:sqlite';
import {spawn,spawnSync} from 'node:child_process';
import {tmpdir} from 'node:os';
import {resolve, join} from 'node:path';
import {createHash} from 'node:crypto';
import {createApp} from '../mobile/server/index.mjs';
import {HubStore} from '../mobile/server/store.mjs';

const root = realpathSync(mkdtempSync(join(tmpdir(), 'hb-archive-ui-data-')));
const database = join(root, 'issues.db');
const env = {...process.env, HEY_BOSS_ISSUE_DB:database, HEY_BOSS_FLEET_STATE:root,
  HEY_BOSS_INBOX_SOCKET:join(root, 'inbox.sock'), HEY_BOSS_AGENT_ID:'human:boss'};
for (const key of ['HEY_BOSS_ISSUE_HOST','HEY_BOSS_ISSUE_PROJECT','CODEX_THREAD_ID']) delete env[key];
// Keep concurrent checkout builds from restarting the fixture midway through a check.
const binary = join(root, 'hey-boss');
copyFileSync(resolve(process.argv[2] || 'target/debug/hey-boss'), binary);
const children = [];
const start = args => {
  const child = spawn(binary, args, {env, stdio:['ignore','inherit','inherit']});
  children.push(child);
  return child;
};
const owner = start(['fleet','companion']);
const identity = createHash('sha256').update(database).digest('hex').slice(0,24);
const socketBase = `/tmp/hey-boss-db-${process.getuid()}/${identity}`;
let timer, app, server, store, closing = false;
async function close() {
  if (closing) return;
  closing = true;
  clearTimeout(timer);
  app?.locals.close();
  server?.closeAllConnections();
  server?.close();
  await Promise.all(children.map(child => new Promise(resolve => {
    if (child.exitCode !== null || child.signalCode !== null) return resolve();
    child.once('exit', resolve);
    child.kill('SIGTERM');
  })));
  store?.close();
  rmSync(root, {recursive:true,force:true});
  for (const suffix of ['.sock','.lock','.startup','.log']) rmSync(socketBase+suffix,{force:true});
  console.log('COMPLETE: fixture processes stopped and temporary data removed');
}
for (const signal of ['SIGTERM','SIGINT']) process.on(signal, close);
try {
  for (let attempt=0; !existsSync(socketBase+'.sock'); attempt++) {
    if (attempt === 200 || owner.exitCode !== null) throw Error('Fixture database did not start');
    await new Promise(resolve=>setTimeout(resolve,50));
  }
  start(['issue','--project','Archive QA','web','--port','59662','--no-discovery','--json']);
  const native='http://127.0.0.1:59662', paired='http://127.0.0.1:52062';
  let boot;
  for (let attempt=0; !boot; attempt++) {
    if (attempt === 200) throw Error('Fixture web server did not start');
    try {boot=await (await fetch(native+'/api/bootstrap')).json();}
    catch {await new Promise(resolve=>setTimeout(resolve,50));}
  }

  function cli(args){const result=spawnSync(binary,['issue','--project','Archive QA','--agent','human:boss','--json',...args],{env,encoding:'utf8'});if(result.status)throw Error(result.stderr+result.stdout);return JSON.parse(result.stdout);}
  for(let n=1;n<=4;n++) {
    cli(['create','--title',n===3?'Deleted archive conversation':n===4?'Recent closed issue':'Closed archive conversation '+n,'--body','Historical body: cold-search-needle 🦀. **Formatting survives.**']);
    cli(['comment',String(n),'--body','Saved discussion **survives** archival.']);
    if(n===3)cli(['delete',String(n)]);else cli(['close',String(n)]);
  }
  const db=new DatabaseSync(database);db.exec('PRAGMA busy_timeout=10000');
  const old=Date.now()-6*86400000;
  db.prepare('UPDATE issues SET created_at=?,updated_at=?,closed_at=CASE WHEN closed_at IS NULL THEN NULL ELSE ? END,deleted_at=CASE WHEN deleted_at IS NULL THEN NULL ELSE ? END WHERE number<=3').run(old,old,old,old);
  for(const table of ['events','comments','issue_status_updates'])db.prepare('UPDATE '+table+' SET created_at=? WHERE issue_number<=3').run(old);
  db.prepare('INSERT INTO events(project_id,issue_number,actor,action,created_at,data) VALUES(?,?,?,?,?,?)').run('named:Archive QA',1,'human:boss','edited',old,'{}');
  const add=db.prepare('INSERT INTO events(project_id,issue_number,actor,action,created_at,data) VALUES(?,?,?,?,?,?)');
  for(let n=0;n<48;n++)add.run('named:Archive QA',1,'human:boss','edited',old+n+1,JSON.stringify({before:{title:'Earlier title '+n},after:{title:'Later title '+n}}));

  const job={id:'archive-ui-run',project:{id:'named:Archive QA',name:'Archive QA'},issue:{number:2,title:'Closed archive conversation 2',body:'Saved job context'},comments:[],config:{cwd:root,prompt:'Saved prompt',enabled:false},actor:{id:'human:boss',kind:'human',session_id:null,machine:'fixture',host:'fixture',pid:null,process_start:null,cwd:root,source:'fixture',model:null},owner_pid:1,owner_start:'finished',machine:'fixture'};
  db.prepare('INSERT INTO worker_runs(id,project_id,issue_number,job,actor_id,state,owner_pid,owner_start,machine,started_at,updated_at,finished_at,expanded_prompt,last_event) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?)').run(job.id,job.project.id,2,JSON.stringify(job),'human:boss','completed',1,'finished','fixture',old,old,old,'Saved expanded prompt','Archived worker event 28');
  db.prepare('INSERT INTO issue_workers(id,kind,config,version,updated_at) VALUES(?,?,?,?,?)').run('archive-ui-worker','managed',JSON.stringify({name:'Archive fixture',directory:root,projects:[job.project.id],enabled:false}),1,old);
  db.prepare('UPDATE worker_runs SET worker_id=? WHERE id=?').run('archive-ui-worker',job.id);
  const workerEvent=db.prepare('INSERT INTO worker_events(run_id,created_at,text) VALUES(?,?,?)');
  for(let n=1;n<=28;n++)workerEvent.run(job.id,old,'Archived worker event '+n);
  console.log(JSON.stringify({fixture_root:root,waiting_for_archive:true}));
  for(let n=0;;n++) {
    const archived=db.prepare('SELECT count(*) n FROM issues WHERE archive_key IS NOT NULL AND archive_cleanup=0').get().n;
    const runArchived=db.prepare('SELECT archive_key IS NOT NULL AND archive_cleanup=0 done FROM worker_runs WHERE id=?').get(job.id).done;
    if(archived===3&&runArchived)break;
    if(n>180)throw Error('Background archive did not finish');
    await new Promise(r=>setTimeout(r,1000));
  }
  if(db.prepare('SELECT count(*) n FROM comments WHERE issue_number<=3').get().n!==0)throw Error('Fixture history is still hot');
  if(db.prepare('SELECT count(*) n FROM worker_events').get().n!==1)throw Error('Fixture logs are still hot');
  db.close();
  console.log('ARCHIVE READY: three cold issues and a recent hot control');
  store=new HubStore();store.setIssueProjects(boot.projects);
  const headers={'Content-Type':'application/json',Authorization:'Bearer '+'synthetic-archive-fixture'.repeat(2)};
  app=createApp({store,hubToken:'synthetic-archive-fixture'.repeat(2),origin:paired,secure:false});
  app.get('/fixture-pairing',(_req,res)=>res.json({code:store.pairing()}));
  server=app.listen(52062,'127.0.0.1');
  async function relay() {
    try {
      const queue=await (await fetch(paired+'/api/bridge/web',{headers})).json();
      for (const request of queue.requests) {
        const response=request.kind==='inbox'?{ok:true,tasks:[]}:
          await (await fetch(native+'/api/'+request.kind,request.kind==='bootstrap'?{}:{method:'POST',
            headers:{'Content-Type':'application/json','X-Hey-Boss-CSRF':boot.csrf},body:JSON.stringify(request.payload)})).json();
        await fetch(paired+'/api/bridge/web/'+request.id+'/result',{method:'POST',headers,body:JSON.stringify(response)});
      }
    } catch(error) {if(!closing) console.error(error);}
    if(!closing) timer=setTimeout(relay,50);
  }
  relay();
  console.log(JSON.stringify({fixture_pid:process.pid,owner_pid:owner.pid,native,paired:paired+'/issues'}));
} catch(error) {console.error(error);process.exitCode=1;await close();}
