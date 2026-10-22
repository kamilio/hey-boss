// Isolated web QA; never uses the real issue store.
import {mkdirSync,mkdtempSync,copyFileSync} from 'node:fs';
import {spawn,spawnSync} from 'node:child_process';
import {resolve} from 'node:path';
const output=resolve('output/playwright/issue94');mkdirSync(output,{recursive:true});
const root=mkdtempSync(output+'/runtime-');
const binary=root+'/hey-boss';copyFileSync(resolve(process.argv[2]||'target/debug/hey-boss'),binary);
const project={id:'named:Status Dot QA',name:'Status Dot QA'};
const env={...process.env,HEY_BOSS_ISSUE_DB:root+'/issues.db',HEY_BOSS_FLEET_STATE:root,HEY_BOSS_INBOX_SOCKET:root+'/inbox.sock'};
for(const key of ['HEY_BOSS_ISSUE_HOST','HEY_BOSS_ISSUE_PROJECT','CODEX_THREAD_ID'])delete env[key];
const cli=args=>{const r=spawnSync(binary,['issue','--json','--project',project.id,'--agent','human:qa',...args],{env,encoding:'utf8'});if(r.status)throw Error(r.stdout+r.stderr);return JSON.parse(r.stdout);};
for(const [title,level,comment] of [
 ['Reconnect after waking from sleep','green','The fix passes tests. Checking the phone layout next.'],
 ['Install on the connected devices','orange','One device is offline. Retrying the installation.'],
 ['Restore the deployment connection','red','The server is unavailable. Waiting for the connection to return.'],
 ['Plan next week’s improvements',null,null],
 ['Review a long update','orange','A'.repeat(500)],
 ['Keep status text safe','green','<img src=x onerror=alert(1)> & "quoted" status stays plain text.'],
]){
 const number=cli(['create','--at-bottom','--title',title,'--body','Keep the work easy to follow.']).issue.number;
 if(level){cli(['claim',String(number)]);cli(['status',String(number),level,'--comment',comment]);}
}
const web=spawn(binary,['issue','--project',project.id,'web','--port','4798','--no-discovery','--json'],{env,stdio:['ignore','inherit','inherit']});
let closing=false;
function close(){if(closing)return;closing=true;web.kill('SIGTERM');}
for(const signal of ['SIGTERM','SIGINT'])process.on(signal,close);
web.on('exit',close);
console.log(JSON.stringify({fixture_pid:process.pid,native:'http://127.0.0.1:4798'}));
