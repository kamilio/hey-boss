// Isolated native-device project picker; no real Inbox or agents.
// First run cargo build.
import {mkdirSync,mkdtempSync,rmSync,copyFileSync} from 'node:fs';
import {spawn,spawnSync} from 'node:child_process';
import {resolve} from 'node:path';
const output=resolve('output/playwright/issue87');mkdirSync(output,{recursive:true});
const root=mkdtempSync(output+'/runtime-'),binary=root+'/hey-boss';
copyFileSync(resolve('target/debug/hey-boss'),binary);
const env={...process.env,HEY_BOSS_ISSUE_DB:root+'/issues.db',HEY_BOSS_FLEET_STATE:root,HEY_BOSS_INBOX_SOCKET:root+'/inbox.sock'};
for(const key of ['HEY_BOSS_ISSUE_HOST','HEY_BOSS_ISSUE_PROJECT','HEY_BOSS_AGENT_ID','CODEX_THREAD_ID'])delete env[key];
env.HEY_BOSS_AGENT_ID='human:qa';
const projects=Array.from({length:20},(_,i)=>({id:'named:Project '+String(i+1).padStart(2,'0'),name:'Project '+String(i+1).padStart(2,'0')}));
projects.push({id:'named:A very long project name for mobile layout testing',name:'A very long project name for mobile layout testing'});
const cli=args=>{const result=spawnSync(binary,['issue',...args],{env,encoding:'utf8'});if(result.status)throw Error(result.stdout+result.stderr);return result.stdout;};
for(const project of projects)cli(['--project',project.id,'create','--title','Review the mobile project menu']);
cli(['--project','Hidden project','create','--title','Hidden fixture']);
cli(['--project','Hidden project','hide-project']);
const web=spawn(binary,['issue','--project',projects[0].id,'web','--port','59687','--no-discovery','--json'],{env,stdio:['ignore','inherit','inherit']});
let closing=false;
function close(){if(closing)return;closing=true;web.kill('SIGTERM');}
for(const signal of ['SIGTERM','SIGINT'])process.on(signal,close);
web.on('exit',()=>{close();rmSync(root,{recursive:true,force:true});});
console.log(JSON.stringify({fixture_pid:process.pid,native:'http://127.0.0.1:59687'}));
