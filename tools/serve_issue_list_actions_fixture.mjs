// Isolated native issue UI. Stop with Ctrl+C to remove the fixture.
import {mkdtempSync, rmSync, existsSync, realpathSync} from 'node:fs';
import {spawn, execFileSync} from 'node:child_process';
import {tmpdir} from 'node:os';
import {resolve, join} from 'node:path';
import {createHash} from 'node:crypto';
import {createServer} from 'node:http';
import {DatabaseSync} from 'node:sqlite';

const root = realpathSync(mkdtempSync(join(tmpdir(), 'hey-boss-issue139-')));
const database = join(root, 'issues.db');
const env = {...process.env, HEY_BOSS_ISSUE_DB:database, HEY_BOSS_FLEET_STATE:root,
  HEY_BOSS_INBOX_SOCKET:join(root, 'inbox.sock'), HEY_BOSS_AGENT_ID:'human:boss'};
for (const key of ['HEY_BOSS_ISSUE_HOST','HEY_BOSS_ISSUE_PROJECT','CODEX_THREAD_ID']) delete env[key];
const binary = resolve('target/debug/hey-boss');
const initProject = name => execFileSync(binary, ['project','init','--project',name,'--prs','false','--worktree','false','--yes','--json'], {env,cwd:root,stdio:'pipe'});
const children = [];
const start = args => {
  const child = spawn(binary, args, {env, stdio:['ignore','inherit','inherit']});
  children.push(child);
  return child;
};
const owner = start(['fleet','companion']);
const identity = createHash('sha256').update(database).digest('hex').slice(0,24);
const socketBase = `/tmp/hey-boss-db-${process.getuid()}/${identity}`;
let server, closing = false;
async function close() {
  if (closing) return;
  closing = true;
  server?.closeAllConnections();
  server?.close();
  await Promise.all(children.map(child => new Promise(resolve => {
    if (child.exitCode !== null || child.signalCode !== null) return resolve();
    const timeout = setTimeout(() => child.kill('SIGKILL'), 5000);
    child.once('exit', () => {clearTimeout(timeout);resolve();});
    child.kill('SIGTERM');
  })));
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
  initProject('Quick action QA');
  start(['issue','--project','Quick action QA','web','--port','59639','--no-discovery','--json']);
  const native='http://127.0.0.1:59639';
  let boot;
  for (let attempt=0; !boot; attempt++) {
    if (attempt === 200) throw Error('Fixture web server did not start');
    try {boot=await (await fetch(native+'/api/bootstrap')).json();}
    catch {await new Promise(resolve=>setTimeout(resolve,50));}
  }
  server=createServer(async(req,res)=>{
    const send=(status,value)=>{res.writeHead(status,{'Content-Type':'application/json'});res.end(JSON.stringify(value));};
    try{
      if(req.method==='POST'&&req.url==='/fixture-project'){
        const name='Quick actions '+Date.now();initProject(name);return send(200,{project:'named:'+name});
      }
      if(req.method==='POST'&&req.url==='/fixture-hold'){
        let raw='';for await(const chunk of req)raw+=chunk;
        const body=JSON.parse(raw),db=new DatabaseSync(database);
        try{db.exec('PRAGMA busy_timeout=5000');db.prepare("UPDATE issues SET manual_blocked=1,version=version+1 WHERE project_id=? AND number=7 AND state='blocked'").run(body.project);return send(200,{ok:true});}finally{db.close();}
      }
      send(404,{ok:false});
    }catch(error){send(500,{error:error.message});}
  });
  server.listen(52039,'127.0.0.1');
  console.log(JSON.stringify({fixture_pid:process.pid,owner_pid:owner.pid,native}));
} catch(error) {console.error(error);process.exitCode=1;await close();}
