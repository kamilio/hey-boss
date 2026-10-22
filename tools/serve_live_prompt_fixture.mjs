// Isolated web project settings review. No agents are launched.
import {mkdirSync,mkdtempSync,rmSync,copyFileSync} from 'node:fs';
import {spawn,spawnSync} from 'node:child_process';
import {resolve} from 'node:path';
const output=resolve('output/playwright/issue83');mkdirSync(output,{recursive:true});
const root=mkdtempSync(output+'/runtime-'),binary=root+'/hey-boss';
copyFileSync(resolve('target/debug/hey-boss'),binary);
const project={id:'named:Prompt Studio',name:'Prompt Studio'};
const env={...process.env,HEY_BOSS_ISSUE_DB:root+'/issues.db',HEY_BOSS_FLEET_STATE:root,HEY_BOSS_INBOX_SOCKET:root+'/absent.sock'};
for(const key of ['HEY_BOSS_ISSUE_HOST','HEY_BOSS_ISSUE_PROJECT','HEY_BOSS_AGENT_ID','CODEX_THREAD_ID'])delete env[key];
env.HEY_BOSS_AGENT_ID='human:qa';
const cli=args=>{
 const result=spawnSync(binary,['issue','--json','--project',project.id,'--agent','human:qa',...args],{env,cwd:root,encoding:'utf8'});
 if(result.status)throw Error(result.stdout+result.stderr);return JSON.parse(result.stdout);
};
cli(['create','--title','Keep running agents up to date','--body','Save instructions without restarting active work.']);
const web=spawn(binary,['issue','--project',project.id,'web','--port','4883','--no-discovery','--json'],{env,cwd:root,stdio:['ignore','inherit','inherit']});
let closing=false;
function close(){if(closing)return;closing=true;web.kill('SIGTERM');}
for(const signal of ['SIGTERM','SIGINT'])process.on(signal,close);
web.on('exit',()=>{close();rmSync(root,{recursive:true,force:true});});
console.log(JSON.stringify({fixture_pid:process.pid,native:'http://127.0.0.1:4883'}));
