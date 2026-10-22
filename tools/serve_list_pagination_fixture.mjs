// Isolated native issue UI. Stop with Ctrl+C to remove the fixture.
import {mkdtempSync, rmSync, existsSync, realpathSync, copyFileSync} from 'node:fs';
import {spawn,spawnSync} from 'node:child_process';
import {tmpdir} from 'node:os';
import {resolve, join} from 'node:path';
import {createHash} from 'node:crypto';

const root = realpathSync(mkdtempSync(join(tmpdir(), 'hey-boss-pagination-')));
const database = join(root, 'issues.db');
const env = {...process.env, HEY_BOSS_ISSUE_DB:database, HEY_BOSS_FLEET_STATE:root,
  HEY_BOSS_INBOX_SOCKET:join(root, 'inbox.sock'), HEY_BOSS_AGENT_ID:'human:boss'};
for (const key of ['HEY_BOSS_ISSUE_HOST','HEY_BOSS_ISSUE_PROJECT','CODEX_THREAD_ID']) delete env[key];
const binary = join(root, 'hey-boss');
copyFileSync(resolve('target/debug/hey-boss'), binary);
const children = [];
const start = args => {
  const child = spawn(binary, args, {env, stdio:['ignore','inherit','inherit']});
  children.push(child);
  return child;
};
const owner = start(['fleet','companion']);
const identity = createHash('sha256').update(database).digest('hex').slice(0,24);
const socketBase = `/tmp/hey-boss-db-${process.getuid()}/${identity}`;
let closing = false;
async function close() {
  if (closing) return;
  closing = true;
  await Promise.all(children.map(child => new Promise(resolve => {
    if (child.exitCode !== null || child.signalCode !== null) return resolve();
    const deadline=setTimeout(()=>child.kill('SIGKILL'),3000);
    child.once('exit',()=>{clearTimeout(deadline);resolve();});
    child.kill('SIGINT');
  })));
  rmSync(root, {recursive:true,force:true});
  for (const suffix of ['.sock','.lock','.startup','.log']) rmSync(socketBase+suffix,{force:true});
  console.log('COMPLETE: fixture processes stopped and temporary data removed');
}
for (const signal of ['SIGTERM','SIGINT']) process.on(signal,()=>close().then(()=>process.exit(0)));
try {
  for (let attempt=0; !existsSync(socketBase+'.sock'); attempt++) {
    if (attempt === 200 || owner.exitCode !== null) throw Error('Fixture database did not start');
    await new Promise(resolve=>setTimeout(resolve,50));
  }
  const init=spawnSync(binary,['project','init','--project','Pagination QA','--prs','false','--worktree','false','--yes'],{env,cwd:root,encoding:'utf8'});
  if(init.status!==0)throw Error(init.stderr || init.stdout);
  start(['issue','--project','Pagination QA','web','--port','59671','--no-discovery','--json']);
  const native='http://127.0.0.1:59671';
  let boot;
  for (let attempt=0; !boot; attempt++) {
    if (attempt === 200) throw Error('Fixture web server did not start');
    try {boot=await (await fetch(native+'/api/bootstrap')).json();}
    catch {await new Promise(resolve=>setTimeout(resolve,50));}
  }
  console.log(JSON.stringify({fixture_pid:process.pid,owner_pid:owner.pid,native}));
} catch(error) {console.error(error);process.exitCode=1;await close();}
