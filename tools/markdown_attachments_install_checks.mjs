// Real CLI, disk store, and embedded UI checks. --serve retains the isolated web
// server for browser checks; SIGTERM removes all fixture state.
import assert from 'node:assert/strict';
import {mkdtempSync,writeFileSync,rmSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {join,resolve} from 'node:path';
import {spawn} from 'node:child_process';
const binary=resolve(process.argv[2]),serve=process.argv.includes('--serve');
const root=mkdtempSync(join(tmpdir(),'hey-boss-markdown-'));
const project='named:Markdown attachments';
const env={...process.env,HEY_BOSS_AGENT_ID:'human:verification',HEY_BOSS_ISSUE_DB:join(root,'issues.db'),HEY_BOSS_FLEET_STATE:root,HEY_BOSS_INBOX_SOCKET:join(root,'inbox.sock')};
for(const name of ['HEY_BOSS_ISSUE_HOST','HEY_BOSS_ISSUE_PROJECT','HEY_BOSS_STATE_DIR'])delete env[name];
const run=args=>new Promise((resolve,reject)=>{const child=spawn(binary,args,{env,cwd:root,stdio:['ignore','pipe','pipe']});let out='',err='';child.stdout.on('data',v=>out+=v);child.stderr.on('data',v=>err+=v);child.on('error',reject);child.on('close',code=>resolve({code,out,err}));});
const command=async args=>{const result=await run([...args,'--project',project,'--json']);assert.equal(result.code,0,result.err||result.out);return JSON.parse(result.out);};
let web;
async function cleanup(){if(web&&web.exitCode===null&&web.signalCode===null){const ended=new Promise(r=>web.once('exit',r));web.kill();await ended;}rmSync(root,{recursive:true,force:true});}
try{
 web=spawn(binary,['issue','web','--port','0','--no-discovery','--project',project,'--json'],{env,cwd:root,stdio:['ignore','pipe','pipe']});
 const info=await new Promise((resolve,reject)=>{let out='',err='';const timer=setTimeout(()=>reject(Error('Web startup timed out: '+err)),60000);web.stderr.on('data',v=>err+=v);web.on('error',reject);web.on('exit',()=>{clearTimeout(timer);reject(Error(err));});web.stdout.on('data',v=>{out+=v;if(out.includes('\n')){clearTimeout(timer);resolve(JSON.parse(out.split('\n')[0]));}});});
 const base=info.url.replace(/\/$/,'');
 writeFileSync(join(root,'chart.svg'),'<svg xmlns="http://www.w3.org/2000/svg" width="720" height="280" viewBox="0 0 720 280"><rect width="720" height="280" rx="20" fill="#172b4d"/><text x="36" y="52" fill="#fff" font-family="sans-serif" font-size="24">Weekly build performance</text><rect x="40" y="106" width="470" height="34" rx="6" fill="#48c9b0"/><rect x="40" y="162" width="330" height="34" rx="6" fill="#70aaff"/><text x="40" y="242" fill="#fff" font-family="sans-serif" font-size="18">Hosted image · available on every device</text></svg>');
 writeFileSync(join(root,'data.csv'),'day,duration\nMonday,24\nTuesday,17\n');
 writeFileSync(join(root,'report.md'),'# Release evidence\n\nImages and downloads travel with this document.\n\n![Weekly build performance](chart.svg "Build duration")\n\n[Download measurements][data]\n\n[data]: data.csv\n\n`![example](missing.png)` remains code.\n');
 const args=['artifact','create','--title','Release evidence','--file',join(root,'report.md'),'--request-id','fixture-import'];
 const created=await command(args),id=created.artifact.id;
 assert.deepEqual(await command(args),created);
 assert.match(created.artifact.body,/\/attachments\/f-/);
 const files=await command(['attachment','list','--artifact',id]);assert.equal(files.attachments.length,2);
 const csv=files.attachments.find(file=>file.name==='data.csv');
 await command(['attachment','download',csv.id,'--output',join(root,'download.csv')]);
 const {readFileSync}=await import('node:fs');assert.equal(readFileSync(join(root,'download.csv'),'utf8'),readFileSync(join(root,'data.csv'),'utf8'));
 writeFileSync(join(root,'missing.md'),'![missing](absent.png)');
 const missing=await run(['artifact','create','--title','Missing','--file',join(root,'missing.md'),'--project',project]);assert.notEqual(missing.code,0);assert.match(missing.err,/absent.png/);
 const stale=await run(['artifact','edit',id,'--file',join(root,'report.md'),'--if-version','99','--project',project]);assert.notEqual(stale.code,0);
 assert.equal((await command(['attachment','list','--artifact',id])).attachments.length,2);
 for(const name of ['chart.svg','data.csv','report.md'])rmSync(join(root,name));
 await command(['attachment','download',csv.id,'--output',join(root,'after-source-removal.csv')]);
 assert.equal(readFileSync(join(root,'after-source-removal.csv'),'utf8'),readFileSync(join(root,'download.csv'),'utf8'));
 const page=await fetch(base+'/artifacts');assert.match(page.headers.get('content-security-policy'),/img-src[^;]*blob:/);
 const js=await (await fetch(base+'/attachments.js')).text();assert.match(js,/function hydrate/);
 console.log(JSON.stringify({status:'passed',base,url:base+'/artifacts#project='+encodeURIComponent(project)+'&artifact='+id,project,id,root,checks:['CLI import','request replay','hosted destinations','two attachments','byte-exact download','survives source removal','missing file rejection','stale edit rollback','embedded reader']}));
 if(serve){await new Promise(resolve=>{process.once('SIGTERM',resolve);process.once('SIGINT',resolve);});}
}finally{await cleanup();}
