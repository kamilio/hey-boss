// Isolated full-corpus workload. No real provider or production data is used.
// node tools/issues_search_performance.mjs [path/to/release/hey-boss]
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import {spawn,spawnSync} from 'node:child_process';
import readline from 'node:readline';
import assert from 'node:assert/strict';
const root=fs.mkdtempSync(path.join(os.tmpdir(),'hb-search-perf-'));
const bin=path.join(root,'bin');fs.mkdirSync(bin);
const executable=path.join(bin,'hey-boss');fs.copyFileSync(process.argv[2]||'target/release/hey-boss',executable);fs.chmodSync(executable,0o755);
const env={...process.env,HEY_BOSS_ISSUE_DB:path.join(root,'issues.db'),HEY_BOSS_INBOX_SOCKET:path.join(root,'inbox.sock')};
const run=(command,args,opts={})=>{const r=spawnSync(command,args,{encoding:'utf8',...opts});if(r.status!==0)throw Error(r.stderr||r.stdout);return r.stdout;};
const sql=text=>run('sqlite3',[env.HEY_BOSS_ISSUE_DB],{input:'.timeout 5000\n'+text});
fs.writeFileSync(path.join(root,'mode'),'ok');
fs.writeFileSync(path.join(bin,'hey-proxy'),`#!/usr/bin/env node
const fs=require('fs'),path=require('path');const root=path.dirname(__dirname);
let input='';process.stdin.on('data',b=>input+=b);process.stdin.on('end',()=>{
 fs.appendFileSync(root+'/requests','1\\n');const mode=fs.readFileSync(root+'/mode','utf8');
 if(mode==='down')process.exit(1);if(mode==='slow'){setTimeout(()=>process.exit(1),30000);return;}
 const p=JSON.parse(input);process.stdout.write(JSON.stringify({data:p.input.map((_,index)=>({index,embedding:Array.from({length:512},(_,i)=>i===0?1:0)}))}));
});`,{mode:0o755});
let child;
try {
 run(executable,['issue','search-index','--project','named:SearchBench','--json'],{env});
 sql(`BEGIN;
 INSERT INTO projects(id,name,next_number) VALUES('named:SearchBench','SearchBench',9001);
 INSERT INTO agents VALUES('human:boss','{"id":"human:boss","kind":"human"}',1);
 WITH RECURSIVE n(x) AS(VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<9000)
 INSERT INTO issues(project_id,number,title,body,state,created_by,created_at,updated_at,version,labels,sort_order)
 SELECT 'named:SearchBench',x,'Network issue '||x,printf('Reconnect after sleep %d. ${'Network changes can interrupt a background connection. '.repeat(12)}',x),'open','human:boss',1,1,1,'[]',x FROM n;
 WITH RECURSIVE n(x) AS(VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<90000)
 INSERT INTO comments(project_id,issue_number,author,body,created_at)
 SELECT 'named:SearchBench',((x-1)%9000)+1,'human:boss',printf('Comment %d. ${'The connection should recover after waking and retry the queued update. '.repeat(8)}',x),1 FROM n;
 COMMIT;`);
 child=spawn(executable,['issue','web','--no-discovery','--port','0','--project','named:SearchBench','--json'],{env,stdio:['ignore','pipe','pipe']});
 child.stderr.on('data',b=>process.stderr.write(b));
 const ready=await new Promise((resolve,reject)=>{readline.createInterface({input:child.stdout}).once('line',line=>{try{resolve(JSON.parse(line))}catch(e){reject(e)}});child.once('exit',c=>reject(Error('web exited '+c)));});
 const boot=await (await fetch(ready.url+'api/bootstrap')).json();
 const api=async operation=>{const r=await fetch(ready.url+'api/action',{method:'POST',headers:{'Content-Type':'application/json','X-Hey-Boss-CSRF':boot.csrf,'Origin':ready.url.slice(0,-1)},body:JSON.stringify({project:'named:SearchBench',operation})});const v=await r.json();if(!v.ok)throw Error(JSON.stringify(v));return v;};
 const list={action:'list',state:'active',mine:false,unassigned:false,labels:[],search:null,limit:50,offset:0};
 const median=values=>{values.sort((a,b)=>a-b);return {median_ms:+values[Math.floor(values.length/2)].toFixed(2),p95_ms:+values[Math.floor(values.length*.95)].toFixed(2)};};
 const sample=async label=>{
   const reads=[],writes=[],sync=[];
   for(let i=0;i<30;i++){
     let start=performance.now();assert.equal((await api(list)).issues.length,50);reads.push(performance.now()-start);
     start=performance.now();await api({action:'edit',number:9000,title:'Network issue 9000',body:'Fresh foreground edit '+label+i,add_labels:[],remove_labels:[],if_version:null});writes.push(performance.now()-start);
     start=performance.now();sql(`BEGIN IMMEDIATE;UPDATE issues SET body='Replica batch ${label} ${i}' WHERE number BETWEEN 8900 AND 8924;COMMIT;`);sync.push(performance.now()-start);
     await new Promise(resolve=>setTimeout(resolve,100));
   }
   const result={phase:label,list:median(reads),save:median(writes),replica_25_updates:median(sync)};console.log(JSON.stringify(result));return result;
 };
 await sample('disabled');assert(!fs.existsSync(path.join(root,'requests')),'disabled search must not call provider');
 await api({action:'search_index',enabled:true});
 await sample('backfill');
 assert(fs.existsSync(path.join(root,'requests')),'backfill must request embeddings');
 fs.writeFileSync(path.join(root,'mode'),'down');
 await sample('provider_down');
 const start=performance.now();const fallback=await api({action:'search',query:'Network issue outage'});assert.equal(fallback.search.mode,'keywords');assert(fallback.issues.length>0);console.log(JSON.stringify({outage_search_ms:performance.now()-start}));
 // A slow query must not hold the issue read transaction or writer lease.
 sql('PRAGMA wal_checkpoint(PASSIVE);');
 run('sqlite3',[env.HEY_BOSS_ISSUE_DB+'.search.db',"DELETE FROM meta WHERE key='query_retry_after';"]);
 fs.writeFileSync(path.join(root,'mode'),'slow');
 const pending=api({action:'search',query:'Network issue slow'});
 const readStart=performance.now();await api(list);const readMs=performance.now()-readStart;
 const saveStart=performance.now();await api({action:'comment',number:1,body:'Comment while embeddings wait'});const saveMs=performance.now()-saveStart;
 await pending;console.log(JSON.stringify({during_slow_query:{list_ms:readMs,save_ms:saveMs}}));
 await api({action:'search_index',enabled:false});
 console.log(JSON.stringify({index:await api({action:'search_index'}),rss_kib:run('ps',['-o','rss=','-p',String(child.pid)]).trim()}));
 // Complete the derived fixture directly to measure retrieval at full scale.
 const vector=Buffer.alloc(512*4);vector.writeFloatLE(1,0);
 const compact=Buffer.alloc(512+4);compact[0]=127;compact.writeFloatLE(1/127,512);
 run('sqlite3',[env.HEY_BOSS_ISSUE_DB+'.search.db'],{input:`.timeout 5000
 ATTACH '${env.HEY_BOSS_ISSUE_DB}' AS source;
 BEGIN;
 DELETE FROM passages; DELETE FROM embeddings; DELETE FROM documents;
 INSERT INTO passages(project,number,field,source,part,text,hash) SELECT project_id,number,'title',0,0,title,'title-'||number FROM source.issues;
 INSERT INTO passages(project,number,field,source,part,text,hash) SELECT project_id,number,'description',0,0,body,'body-'||number FROM source.issues;
 INSERT INTO passages(project,number,field,source,part,text,hash) SELECT project_id,issue_number,'comment',id,0,body,'comment-'||id FROM source.comments;
 INSERT INTO documents(project,number,generation,complete) SELECT project,number,CAST(generation AS TEXT),1 FROM source.issue_search_dirty;
 INSERT INTO embeddings SELECT DISTINCT hash,X'${compact.toString('hex')}' FROM passages;
 INSERT OR REPLACE INTO queries VALUES('connection resume',X'${vector.toString('hex')}',0);
 INSERT OR REPLACE INTO meta VALUES('backfill_done','1');
 COMMIT;`});
 sql('UPDATE issue_search_dirty SET dirty=0;');
 fs.writeFileSync(path.join(root,'mode'),'ok');
 await api({action:'search_index',enabled:true});
 for(const semantic of [false,true]) {
   const times=[];
   for(let i=0;i<5;i++){const at=performance.now();const result=await api({action:'search',query:'connection resume',semantic});assert(result.issues.length>0);times.push(performance.now()-at);}
   console.log(JSON.stringify({full_index:108001,semantic,...median(times)}));
 }
 await api({action:'search_index',enabled:false});
} finally {
 if(child){child.kill('SIGKILL');await new Promise(resolve=>child.once('exit',resolve));}
 for(const line of run('ps',['-axo','pid,command']).split('\n')) {
   if(line.includes(root+'/bin/')) {const pid=Number(line.trim().split(/\s+/)[0]);if(pid&&pid!==process.pid){try{process.kill(pid,'SIGKILL')}catch{}}}
 }
 await new Promise(resolve=>setTimeout(resolve,250));
 fs.rmSync(root,{recursive:true,force:true,maxRetries:5,retryDelay:100});
}
