// Verify installed Rust history reader and embedded UI in disposable private state.
import {mkdtempSync,mkdirSync,writeFileSync,rmSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {join,resolve} from 'node:path';
import {spawn,spawnSync} from 'node:child_process';
const binary=resolve(process.argv[2]),expected=process.argv[3]??'';
const run=(args,options={})=>{const r=spawnSync(binary,args,{encoding:'utf8',...options});if(r.status!==0)throw Error(r.stderr||r.stdout);return r.stdout;};
const version=run(['--version']).trim();if(!version.includes(expected))throw Error('Installed build mismatch');
const root=mkdtempSync(join(tmpdir(),'hey-boss-agent-check-'));let web;
try{
 const env={...process.env,HEY_BOSS_ISSUE_DB:join(root,'issues.db'),HEY_BOSS_FLEET_STATE:join(root,'fleet'),CODEX_HOME:join(root,'codex')};
 for(const key of ['HEY_BOSS_ISSUE_HOST','HEY_BOSS_ISSUE_PROJECT','HEY_BOSS_STATE_DIR'])delete env[key];
 run(['issue','--project','Agents verification','create','--title','Saved conversation'],{env,cwd:root});
 const session='aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee',path=join(root,'codex/sessions/2026/09/19/rollout-test-'+session+'.jsonl');mkdirSync(resolve(path,'..'),{recursive:true});
 writeFileSync(path,[['user','Original request'],['assistant','Verified reply']].map(([role,text])=>JSON.stringify({type:'response_item',payload:{type:'message',role,content:[{type:'output_text',text}]}})+'\n').join(''));
 const sql="INSERT INTO worker_runs(id,project_id,issue_number,job,actor_id,session_id,state,owner_pid,owner_start,machine,started_at,updated_at,expanded_prompt) SELECT 'verify-run',project_id,number,'{\"issue\":{\"title\":\"Saved conversation\"}}',created_by,?,'completed',1,'start','fixture',1,1,'Original request' FROM issues LIMIT 1";
 const db=run(['fleet','database','--path',env.HEY_BOSS_ISSUE_DB],{input:JSON.stringify({sql,args:[session]})+'\n',env,cwd:root});if(!JSON.parse(db).ok)throw Error(db);
 const first=JSON.parse(run(['fleet','conversation'],{env,cwd:root,input:JSON.stringify({run:'verify-run',latest:true})}));
 if(!first.ok||first.messages.map(m=>m.text).join('|')!=='Original request|Verified reply')throw Error('Saved history mismatch');
 const next=JSON.parse(run(['fleet','conversation'],{env,cwd:root,input:JSON.stringify({run:'verify-run',cursor:first.cursor})}));if(next.messages.length)throw Error('Duplicate incremental history');
 web=spawn(binary,['issue','web','--port','0','--no-discovery','--project','Agents verification','--json'],{env,cwd:root,stdio:['ignore','pipe','pipe']});
 const info=await new Promise((yes,no)=>{let line='';const timer=setTimeout(()=>no(Error('Web startup timed out')),20000);web.stdout.on('data',chunk=>{line+=chunk;if(line.includes('\n')){clearTimeout(timer);try{yes(JSON.parse(line.split('\n')[0]));}catch(e){no(e);}}});web.on('error',no);web.on('exit',()=>no(Error('Web exited')));});
 if(!info.ok)throw Error('Web startup failed');const base=info.url.replace(/\/$/,'');
 for(const path of ['/agents','/agents/session','/workers']){const r=await fetch(base+path);const html=await r.text();if(!r.ok||!html.includes('Agents · Hey Boss')||!html.includes('id="conversation"')||html.includes('coordinates workers'))throw Error('Installed UI mismatch: '+path);}
 const js=await(await fetch(base+'/fleet.js')).text();if(!js.includes('pageshow')||!js.includes('projectView')||!js.includes('conversation-end'))throw Error('Installed interaction code mismatch');
 console.log(JSON.stringify({status:'passed',version,installed_assets:true,full_saved_history:true,incremental_history:true,rust_reader:true}));
}finally{if(web&&web.exitCode===null){const ended=new Promise(r=>web.once('exit',r));web.kill();await ended;}rmSync(root,{recursive:true,force:true});}
