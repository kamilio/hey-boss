// Real isolated issue database and HTTP API, plus the paired mobile relay.
import {mkdtempSync,rmSync,readFileSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {resolve,join} from 'node:path';
import {spawn,spawnSync} from 'node:child_process';
import {DatabaseSync} from 'node:sqlite';
import {createServer,request} from 'node:http';
import {createApp} from '../mobile/server/index.mjs';
import {HubStore} from '../mobile/server/store.mjs';
const root=mkdtempSync(join(tmpdir(),'hb-timeline-visual-'));
const binary=resolve('target/debug/hey-boss'),project={id:'named:Timeline QA',name:'Timeline QA'};
const env={...process.env,HEY_BOSS_ISSUE_DB:join(root,'issues.db'),HEY_BOSS_FLEET_STATE:root,HEY_BOSS_FLEET_DESIRED:join(root,'absent.json'),HEY_BOSS_INBOX_SOCKET:join(root,'absent.sock')};
for(const k of ['HEY_BOSS_ISSUE_HOST','HEY_BOSS_ISSUE_PROJECT','HEY_BOSS_AGENT_ID'])delete env[k];
const web=spawn(binary,['issue','--project',project.id,'web','--port','48947','--no-discovery','--json'],{env,stdio:['ignore','inherit','inherit']});
let proxy,mobile,store,app,timer,closing=false;
function close(){if(closing)return;closing=true;clearInterval(timer);app?.locals.close();mobile?.close();proxy?.close();store?.close();web.kill('SIGTERM');}
for(const signal of ['SIGINT','SIGTERM'])process.on(signal,close);
process.on('exit',()=>{web.kill('SIGTERM');rmSync(root,{recursive:true,force:true});});
web.on('exit',close);
for(let n=0;;n++){try{if((await fetch('http://127.0.0.1:48947/')).ok)break;}catch{}if(n>100)throw Error('Fixture startup timed out');await new Promise(r=>setTimeout(r,100));}
function cli(args){const r=spawnSync(binary,['issue','--project',project.id,'--agent','human:boss','--json',...args],{env,encoding:'utf8'});if(r.status)throw Error(r.stderr+r.stdout);return JSON.parse(r.stdout);}
cli(['create','--title','Preserve the conversation when reconnecting','--body','Keep the conversation and unsent draft when a connection drops.\n\n- [x] Restore the session\n- [ ] Verify the phone layout','--label','enhancement']);
cli(['create','--title','A quiet issue','--body','No discussion yet.']);
cli(['create','--title','Archived conversation']);cli(['delete','3']);
const db=new DatabaseSync(env.HEY_BOSS_ISSUE_DB);db.exec('PRAGMA busy_timeout=5000');
const at=Date.now()-3600000;
db.prepare("UPDATE events SET created_at=? WHERE issue_number=1 AND action='created'").run(at-1000);
db.prepare('UPDATE issues SET created_at=? WHERE number=1').run(at-1000);
const actor={id:'codex:timeline',kind:'codex',model:'gpt-6-astra',machine:'test',host:'MacBook Pro',cwd:root,session_id:'timeline',source:'fixture'};
db.prepare('INSERT INTO agents VALUES(?,?,?)').run(actor.id,JSON.stringify(actor),at);
db.prepare('INSERT INTO agents VALUES(?,?,?)').run('watcher:github',JSON.stringify({...actor,id:'watcher:github',kind:'system'}),at);
const event=db.prepare('INSERT INTO events(project_id,issue_number,actor,action,created_at,data) VALUES(?,1,?,?,?,?)');
const add=(time,who,action,data)=>event.run(project.id,who,action,at+time,JSON.stringify({...data,...(who===actor.id?{actor_model:actor.model}:{})}));
for(let n=0;n<36;n++)add(n*1000,'human:boss','edited',{before:{labels:[]},after:{labels:['triage-'+n]}});
add(60000,'human:boss','assigned',{target:'machine:devbox',previous_assignee:null,assignee:null});
add(120000,actor.id,'claimed',{previous_assignee:null,assignee:actor.id});
const comment=db.prepare('INSERT INTO comments(project_id,issue_number,author,body,created_at) VALUES(?,1,?,?,?)');
const first=comment.run(project.id,actor.id,'The draft now survives reconnecting. **Tests pass.**\n\nChecking the narrow layout next.',at+180000).lastInsertRowid;
add(180000,actor.id,'commented',{comment_id:Number(first),body:'The draft now survives reconnecting.'});
add(240000,actor.id,'edited',{before:{labels:['enhancement']},after:{labels:['bug','needs-review']}});
add(300000,actor.id,'assigned',{target:'github',previous_assignee:actor.id,assignee:'watcher:github'});
add(360000,'watcher:github','unassigned',{previous_assignee:'watcher:github'});
const last=comment.run(project.id,'human:boss','Looks good on the phone. Keep the activity readable when labels or issue titles are long.',at+420000).lastInsertRowid;
add(420000,'human:boss','commented',{comment_id:Number(last)});
add(480000,'human:boss','assigned',{target:'boss',previous_assignee:null,assignee:'human:boss'});
add(540000,'human:boss','edited',{before:{labels:[],title:'Previous title'},after:{labels:['a-long-label-that-must-wrap-without-making-the-phone-scroll-sideways','<img src=x onerror=alert(1)>'],title:'Preserve the conversation when reconnecting'}});
db.close();
proxy=createServer((req,res)=>{
 const file=req.url.split('?')[0].slice(1);
 if(['app.js','app.css'].includes(file)){res.setHeader('Content-Type',file.endsWith('.js')?'text/javascript':'text/css');return res.end(readFileSync(resolve('src/issues/web',file)));}
 const headers={...req.headers,host:'127.0.0.1:48947'};delete headers['sec-fetch-site'];for(const k of ['origin','referer'])if(headers[k])headers[k]=headers[k].replace('48948','48947');
 const upstream=request({hostname:'127.0.0.1',port:48947,path:req.url,method:req.method,headers},r=>{res.writeHead(r.statusCode,r.headers);r.pipe(res);});upstream.on('error',()=>{res.writeHead(502);res.end();});req.pipe(upstream);
});proxy.listen(48948,'127.0.0.1');
store=new HubStore();store.setIssueProjects([project]);const hubToken='synthetic-timeline-fixture-'.repeat(4);
app=createApp({store,hubToken,secure:false,origin:'http://127.0.0.1:52947'});app.get('/fixture/pairing',(_,res)=>res.json({code:store.pairing()}));mobile=app.listen(52947,'127.0.0.1');
let pumping=false;
timer=setInterval(async()=>{if(pumping||closing)return;pumping=true;const headers={Authorization:'Bearer '+hubToken,'Content-Type':'application/json'};try{
 const {requests}=await(await fetch('http://127.0.0.1:52947/api/bridge/web',{headers})).json();
 await Promise.all(requests.map(async row=>{const bootstrap=await(await fetch('http://127.0.0.1:48947/api/bootstrap')).json();const result=row.kind==='bootstrap'?bootstrap:await(await fetch('http://127.0.0.1:48947/api/'+row.kind,{method:'POST',headers:{'Content-Type':'application/json','X-Hey-Boss-CSRF':bootstrap.csrf},body:JSON.stringify(row.payload)})).json();await fetch('http://127.0.0.1:52947/api/bridge/web/'+row.id+'/result',{method:'POST',headers,body:JSON.stringify(result)});}));
}catch(e){if(!closing)console.error(e.message);}finally{pumping=false;}},50);
console.log('Timeline fixtures: desktop http://127.0.0.1:48948, paired phone http://127.0.0.1:52947/issues');
