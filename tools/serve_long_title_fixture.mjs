// Isolated native and paired issue UI. Stop with Ctrl+C to remove the fixture.
import {mkdtempSync, rmSync, existsSync, realpathSync} from 'node:fs';
import {spawn} from 'node:child_process';
import {tmpdir} from 'node:os';
import {resolve, join} from 'node:path';
import {createHash} from 'node:crypto';
import {createApp} from '../mobile/server/index.mjs';
import {HubStore} from '../mobile/server/store.mjs';

const root = realpathSync(mkdtempSync(join(tmpdir(), 'hey-boss-issue253-')));
const database = join(root, 'issues.db');
const env = {...process.env, HEY_BOSS_ISSUE_DB:database, HEY_BOSS_FLEET_STATE:root,
  HEY_BOSS_INBOX_SOCKET:join(root, 'inbox.sock'), HEY_BOSS_AGENT_ID:'human:boss'};
for (const key of ['HEY_BOSS_ISSUE_HOST','HEY_BOSS_ISSUE_PROJECT','CODEX_THREAD_ID']) delete env[key];
const binary = resolve(process.argv[2] || 'target/debug/hey-boss');
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
  start(['issue','--project','Long title QA','web','--port','59653','--no-discovery','--json']);
  const native='http://127.0.0.1:59653', paired='http://127.0.0.1:52053';
  let boot;
  for (let attempt=0; !boot; attempt++) {
    if (attempt === 200) throw Error('Fixture web server did not start');
    try {boot=await (await fetch(native+'/api/bootstrap')).json();}
    catch {await new Promise(resolve=>setTimeout(resolve,50));}
  }
  store=new HubStore();store.setIssueProjects(boot.projects);
  const headers={'Content-Type':'application/json',Authorization:'Bearer '+'synthetic-long-title-fixture'.repeat(2)};
  app=createApp({store,hubToken:'synthetic-long-title-fixture'.repeat(2),origin:paired,secure:false});
  app.get('/fixture-pairing',(_req,res)=>res.json({code:store.pairing()}));
  server=app.listen(52053,'127.0.0.1');
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
