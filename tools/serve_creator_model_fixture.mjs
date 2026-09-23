// Real CLI capture and real issue UI, using only an isolated database.
import {mkdirSync, writeFileSync, rmSync, copyFileSync} from 'node:fs';
import {spawn, spawnSync} from 'node:child_process';
import {resolve} from 'node:path';
const root = resolve('output/playwright/issue134/runtime'), binary = root + '/hey-boss';
mkdirSync(root + '/codex/sessions', {recursive:true});
copyFileSync(resolve('target/debug/hey-boss'), binary);
const env = {...process.env, HEY_BOSS_ISSUE_DB:root+'/issues.db', HEY_BOSS_FLEET_STATE:root, CODEX_HOME:root+'/codex', HEY_BOSS_INBOX_SOCKET:root+'/inbox.sock'};
for (const key of ['HEY_BOSS_ISSUE_HOST','HEY_BOSS_ISSUE_PROJECT','HEY_BOSS_AGENT_ID','CODEX_THREAD_ID']) delete env[key];
function issue(args, session) {
  const result = spawnSync(binary, ['issue','--project','Creator model QA','--json',...args], {env:{...env,...(session ? {CODEX_THREAD_ID:session} : {HEY_BOSS_AGENT_ID:'human:boss'})},encoding:'utf8'});
  if (result.status) throw Error(result.stderr || result.stdout);
  return JSON.parse(result.stdout);
}
let number = 0;
for (const [title,model] of [
  ['Preserve the model that discovered this issue','gpt-6-astra'],
  ['Review the reconnect implementation','gpt-6-sol'],
  ['A long custom model name stays readable','custom/'+ 'm'.repeat(220)],
  ['Older Codex issue without model metadata',null],
]) {
  const session = `aaaaaaaa-bbbb-cccc-dddd-${String(++number).padStart(12,'0')}`;
  writeFileSync(root+`/codex/sessions/rollout-${session}.jsonl`, JSON.stringify({type:'turn_context',payload:{model}})+'\n');
  const created = issue(['create','--title',title,'--body','The creating model is recorded automatically. Session details and conversation links remain available.'],session);
  if (created.issue.origin.model !== model) throw Error('Capture failed: '+JSON.stringify(created));
}
issue(['create','--title','Human-created issue retains the Boss attribution']);
const web = spawn(binary,['issue','web','--project','Creator model QA','--port','59734','--no-discovery','--json'],{env,stdio:['ignore','inherit','inherit']});
for (const signal of ['SIGINT','SIGTERM']) process.on(signal,()=>web.kill(signal));
web.on('exit',code=>{rmSync(root,{recursive:true,force:true});process.exit(code??0);});
