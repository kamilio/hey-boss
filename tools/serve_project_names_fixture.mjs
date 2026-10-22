// Isolated project-name QA. Pass a CLI binary as the first argument if needed.
import {mkdirSync,mkdtempSync,rmSync} from 'node:fs';
import {spawn,spawnSync} from 'node:child_process';
import {resolve} from 'node:path';
const output=resolve('output/playwright/issue107');mkdirSync(output,{recursive:true});
const root=mkdtempSync(output+'/runtime-'),binary=resolve(process.argv[2]||'target/debug/hey-boss');
const env={...process.env,HEY_BOSS_ISSUE_DB:root+'/issues.db',HEY_BOSS_FLEET_STATE:root,HEY_BOSS_INBOX_SOCKET:root+'/inbox.sock',HEY_BOSS_AGENT_ID:'human:qa'};
for(const key of ['HEY_BOSS_ISSUE_HOST','HEY_BOSS_ISSUE_PROJECT','CODEX_THREAD_ID'])delete env[key];
const cli=args=>{const result=spawnSync(binary,['issue','--json',...args],{env,encoding:'utf8'});if(result.status)throw Error(result.stdout+result.stderr);return JSON.parse(result.stdout);};
cli(['--project','github.com/poe-internal/poe2','create','--title','Original destination']);
cli(['--project','github.com/other/poe2','create','--title','Reused destination']);
cli(['--project','named:poe2','create','--title','Named alias']);
for(const name of ['Design Team','A very long project name for narrow phone layouts','Hidden project'])cli(['--project',name,'create','--title','Fixture']);
cli(['--project','Hidden project','hide-project']);
const web=spawn(binary,['issue','--project','poe2','web','--port','59692','--no-discovery','--json'],{env,stdio:['ignore','inherit','inherit']});
let closing=false;
function close(){if(closing)return;closing=true;web.kill('SIGTERM');}
for(const signal of ['SIGTERM','SIGINT'])process.on(signal,close);
web.on('exit',()=>{close();rmSync(root,{recursive:true,force:true});});
console.log(JSON.stringify({fixture_pid:process.pid,native:'http://127.0.0.1:59692'}));
