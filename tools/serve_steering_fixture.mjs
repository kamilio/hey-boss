// Isolated end-to-end steering: real Rust workers/supervisor, synthetic Codex, web.
import {mkdirSync,mkdtempSync,writeFileSync} from 'node:fs';
import {spawn,spawnSync} from 'node:child_process';
import {resolve} from 'node:path';
mkdirSync(resolve('output/playwright/issue82'),{recursive:true});
const root=mkdtempSync(resolve('output/playwright/issue82/runtime-')),binary=resolve('target/debug/hey-boss');
mkdirSync(root,{recursive:true});
const env={...process.env,HEY_BOSS_ISSUE_DB:root+'/issues.db',HEY_BOSS_FLEET_STATE:root,HEY_BOSS_FLEET_CONFIG:root+'/inventory.json',HEY_BOSS_FLEET_DESIRED:root+'/desired.json',CODEX_HOME:root+'/codex',HEY_BOSS_CODEX:root+'/codex-mock',HEY_BOSS_TEST_CLI:binary,HEY_BOSS_INBOX_SOCKET:root+'/absent-inbox.sock'};
for(const key of ['HEY_BOSS_ISSUE_HOST','HEY_BOSS_ISSUE_PROJECT','HEY_BOSS_AGENT_ID','HEY_BOSS_FLEET_SUPERVISED'])delete env[key];
env.HEY_BOSS_AGENT_ID='human:qa';
writeFileSync(root+'/inventory.json','{"ssh_hosts":[]}');writeFileSync(root+'/desired.json','{"machines":{}}');
writeFileSync(root+'/codex-mock',`#!/usr/bin/env node
const fs=require('node:fs'),{spawnSync}=require('node:child_process'),{randomUUID}=require('node:crypto');
const session=randomUUID();let turn=0,issue;
const directory=process.env.CODEX_HOME+'/sessions/2026/09/21';fs.mkdirSync(directory,{recursive:true});const history=directory+'/rollout-'+session+'.jsonl';
const record=(role,text)=>fs.appendFileSync(history,JSON.stringify({type:'response_item',payload:{type:'message',role,content:[{type:'output_text',text}]}})+'\\n');
const send=value=>process.stdout.write(JSON.stringify(value)+'\\n');
require('node:readline').createInterface({input:process.stdin}).on('line',line=>{
 const v=JSON.parse(line);fs.appendFileSync('${root}/protocol.jsonl',line+'\\n');
 if(v.method==='initialize')send({id:v.id,result:{}});
 if(v.method==='thread/start')send({id:v.id,result:{thread:{id:session}}});
 if(v.method==='turn/start'){
  turn++;const text=v.params.input[0].text;
  if(!issue){issue=/issue view (\\d+)/.exec(text)?.[1];const claim=spawnSync(process.env.HEY_BOSS_TEST_CLI,['issue','--project','Steering Studio','--agent','codex:'+session,'claim',issue]);if(claim.status)throw Error(claim.stderr);}
  record('user',text);record('assistant','I’m checking the reconnect experience and keeping every conversation intact. You can add a reminder while I work.');
  send({id:v.id,result:{turn:{id:'turn-'+turn}}});send({method:'item/started',params:{threadId:session,item:{type:'agentMessage'}}});
 }
 if(v.method==='turn/steer'){
  const text=v.params.input[0].text;
  if(text.includes('REJECT')){send({id:v.id,error:{message:'Synthetic rejected instruction'}});return;}
  record('user',text);record('assistant','I received the extra instruction and will verify it before shipping.');
  send({id:v.id,result:{turnId:v.params.expectedTurnId}});
 }
});
`,{mode:0o755});
const cli=args=>{const r=spawnSync(binary,['issue','--project','Steering Studio',...args],{env,encoding:'utf8'});if(r.status)throw Error(r.stderr);return r;};
cli(['create','--title','Keep conversations intact when devices reconnect','--body','Preserve saved history.']);
cli(['create','--title','Polish the keyboard navigation','--body','Keep all actions accessible.']);
const children=[];const start=args=>{const child=spawn(binary,args,{env,stdio:['ignore','inherit','inherit']});children.push(child);return child;};
start(['worker','--project','Steering Studio','--directory',root,'--concurrency','2']);
start(['fleet','supervisor']);
let stopping=false;
function startWeb(){const web=start(['issue','web','--port','59682','--no-discovery','--project','Steering Studio','--json']);web.on('exit',()=>{if(!stopping)setTimeout(startWeb,500);});}
startWeb();
console.log(JSON.stringify({native:'http://127.0.0.1:59682'}));
for(const signal of ['SIGTERM','SIGINT'])process.on(signal,()=>{stopping=true;for(const child of children)child.kill();});
