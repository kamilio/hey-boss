import express from 'express';
import {agentRoutes} from './agents.mjs';
import {webRoutes} from './web.mjs';
import {checkpointRoutes} from './checkpoint.mjs';
import webpush from 'web-push';
import {fileURLToPath} from 'node:url';
import {HubStore,HubError,equal,token} from './store.mjs';
import {pushContent,preview} from './markdown-text.mjs';
function validateTask(r){
 if(!r||typeof r.taskID!=='string'||r.taskID.length>128||!['alert','update','approval','prompt'].includes(r.kind)||typeof r.title!=='string'||typeof r.question!=='string'||!Array.isArray(r.options)||r.options.length>20||r.options.some(x=>typeof x!=='string'||x.length>512))throw new HubError(400,'Invalid task');
 if(typeof r.description!=='string'||Buffer.byteLength(r.description)>(r.kind==='update'?65536:1048576)||Buffer.byteLength(r.question)>(r.kind==='update'?1048576:65536))throw new HubError(400,'Document must be at most 1 MiB, with a summary of at most 64 KiB');
}
export function createApp({store=new HubStore(),hubToken,origin,secure=true,push=webpush,vapid,now=Date.now,checkpoint=false}={}){
 if(!hubToken||hubToken.length<32)throw Error('HUB_TOKEN must contain at least 32 characters');
 const app=express();app.disable('x-powered-by');app.use('/api/bridge/checkpoint',express.json({limit:'64mb'}));app.use('/api/bridge/artifacts',express.json({limit:'32mb'}));app.use('/api/artifact-requests',express.json({limit:'16mb'}));app.use(express.json({limit:'16mb'}));let bridgeSeen=0;const listeners=new Set();const attempts=new Map();
 if(vapid)push.setVapidDetails(vapid.subject,vapid.publicKey,vapid.privateKey);
 app.use((req,res,next)=>{res.set({'X-Content-Type-Options':'nosniff','Referrer-Policy':'no-referrer','Cache-Control':'no-store','Content-Security-Policy':"default-src 'self'; script-src 'self'; worker-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'"});if(req.method!=='GET'&&req.headers.origin&&req.headers.origin!==origin)return res.status(403).json({error:'Request origin is not allowed'});next();});
 const change=()=>{for(const res of listeners)res.write('data: '+JSON.stringify({revision:store.revision(),connected:Date.now()-bridgeSeen<30000})+'\n\n');};
 const auth=(req,res,next)=>{const secret=req.headers.cookie?.split(';').map(x=>x.trim()).find(x=>x.startsWith('hb_session='))?.slice(11);req.device=secret?store.device(secret):null;if(!req.device)return res.status(401).json({error:'Pair this device to continue'});next();};
 const bridge=(req,res,next)=>{if(!equal(req.headers.authorization,'Bearer '+hubToken))return res.status(401).json({error:'Invalid bridge credentials'});bridgeSeen=Date.now();next();};
 const closeCheckpoint=checkpointRoutes(app,{store,bridge,enabled:checkpoint,hubToken,onRestore:()=>{
  const keys=store.db.prepare("SELECT value FROM metadata WHERE key='vapid'").get();
  if(vapid&&keys){Object.assign(vapid,JSON.parse(keys.value));push.setVapidDetails(vapid.subject,vapid.publicKey,vapid.privateKey);}
 }});
 const closeAgents=agentRoutes(app,{auth,bridge,store,now});
 const closeWeb=webRoutes(app,{auth,bridge});
 app.use('/issue-web',auth,express.static(fileURLToPath(new URL('../dist/issue-web/',import.meta.url))));
 app.get('/healthz',(req,res)=>res.json({ok:true}));
 app.post('/api/bridge/pair-code',bridge,(req,res)=>res.json({code:store.pairing(),expiresIn:300}));
 app.post('/api/pair',(req,res)=>{
  const key=req.socket.remoteAddress;const now=Date.now();const entry=attempts.get(key)??{at:now,count:0};if(now-entry.at>60000){entry.at=now;entry.count=0;}entry.count++;attempts.set(key,entry);if(attempts.size>1000)attempts.delete(attempts.keys().next().value);if(entry.count>10)throw new HubError(429,'Wait a minute before trying again');
  if(typeof req.body.code!=='string'||req.body.code.length>32)throw new HubError(400,'Enter your pairing code');
  const device=store.pair(req.body.code);res.set('Set-Cookie',`hb_session=${device.secret}; HttpOnly; SameSite=Strict; Path=/; Max-Age=31536000${secure?'; Secure':''}`);res.json({paired:true});
 });
 const summary=task=>({...task,description:preview(task.description,240),question:preview(task.question,500)});
 const outcome=task=>({taskID:task.taskID,status:task.status,result:task.result,handledBy:task.handledBy,version:task.version});
 app.get('/api/tasks',auth,(req,res)=>res.json({tasks:store.summaries().map(summary),connected:Date.now()-bridgeSeen<30000,pushEnabled:!!req.device.subscription,vapidPublicKey:vapid?.publicKey??null,notifications:store.routing(now())}));
 app.get('/api/issues',auth,(req,res)=>res.json({projects:store.issueProjects(),creations:store.issueSummaries(req.device.id),connected:store.issueConnected()}));
 app.get('/api/issues/:id',auth,(req,res)=>res.json({creation:store.getIssueCreation(req.device.id,req.params.id)}));
 app.post('/api/issues',auth,(req,res)=>{const creation=store.createIssue(req.device.id,req.body);change();res.status(creation.status==='pending'?202:200).json({creation});});
 app.post('/api/bridge/issue-projects',bridge,(req,res)=>{store.setIssueProjects(req.body.projects);res.json({ok:true});});
 app.get('/api/bridge/issues',bridge,(req,res)=>res.json({creations:store.pendingIssues()}));
 app.post('/api/bridge/issues/:id/result',bridge,(req,res)=>{store.finishIssue(req.params.id,req.body);change();res.json({ok:true});});
 app.get('/project-resource',auth,(req,res)=>res.sendFile(fileURLToPath(new URL('../dist/artifact-web/resource.html',import.meta.url))));
 app.get('/artifacts',auth,(req,res)=>res.sendFile(fileURLToPath(new URL('../dist/issue-web/artifacts.html',import.meta.url))));
 app.use('/agent-web',auth,express.static(fileURLToPath(new URL('../dist/agent-web/',import.meta.url))));
 app.use('/artifact-web',auth,express.static(fileURLToPath(new URL('../dist/artifact-web/',import.meta.url))));
 app.get('/api/artifact-bootstrap',auth,(req,res)=>res.json({projects:store.issueProjects(),connected:store.issueConnected()}));
 app.post('/api/artifact-requests',auth,(req,res)=>res.status(202).json({request:store.artifactRequest(req.device.id,req.body)}));
 app.get('/api/artifact-requests/:id',auth,(req,res)=>res.json({request:store.artifactResult(req.device.id,req.params.id)}));
 app.get('/api/bridge/artifacts',bridge,(req,res)=>res.json({requests:store.pendingArtifacts()}));
 app.post('/api/bridge/artifacts/:id/result',bridge,(req,res)=>{store.finishArtifact(req.params.id,req.body);res.json({ok:true});});
 app.get('/api/tasks/:id',auth,(req,res)=>res.json({task:store.get(req.params.id)}));
 app.post('/api/tasks/clear',auth,(req,res)=>{const cleared=store.clear(req.body.taskIDs);change();res.json({cleared});});
 app.post('/api/tasks/:id/open',auth,(req,res)=>{const task=store.open(req.params.id);change();res.json({task:outcome(task)});});
 app.post('/api/notifications',auth,(req,res)=>{const preferences=store.setPreferences(req.body);change();res.json({notifications:{...preferences,...store.routing(now())}});});
 app.post('/api/bridge/presence',bridge,(req,res)=>{store.presence(req.body,now());res.json({notifications:store.routing(now())});});
 app.post('/api/tasks/:id/resolve',auth,(req,res)=>{const task=store.resolve(req.params.id,req.body.result,'phone',req.body.cancel===true);change();res.json({task});});
 app.post('/api/subscribe',auth,(req,res)=>{
  if(!vapid)throw new HubError(503,'Push has not been configured');const s=req.body;let url;try{url=new URL(s.endpoint);}catch{throw new HubError(400,'Invalid push endpoint');}
  if(url.protocol!=='https:'||!url.hostname.endsWith('.push.apple.com')||url.port||url.username||url.password)throw new HubError(400,'This service accepts Apple Web Push endpoints only');
  if(typeof s.keys?.p256dh!=='string'||typeof s.keys?.auth!=='string'||s.keys.p256dh.length>256||s.keys.auth.length>128)throw new HubError(400,'Invalid subscription keys');
  store.db.prepare('UPDATE devices SET subscription=? WHERE id=?').run(JSON.stringify(s),req.device.id);res.json({subscribed:true});
 });
 app.post('/api/logout',auth,(req,res)=>{store.db.prepare('DELETE FROM devices WHERE id=?').run(req.device.id);res.set('Set-Cookie','hb_session=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0'+(secure?'; Secure':''));res.json({ok:true});});
 app.get('/api/events',auth,(req,res)=>{if(listeners.size>=100)throw new HubError(503,'Connection limit reached');res.set('Content-Type','text/event-stream');res.flushHeaders();listeners.add(res);res.write('data: {}\n\n');req.on('close',()=>listeners.delete(res));});
 app.put('/api/bridge/tasks/:id',bridge,(req,res)=>{
  const r=req.body;validateTask(r);if(r.taskID!==req.params.id)throw new HubError(400,'Invalid task');
  const {task,created}=store.upsert(r);if(created){store.enqueue({id:task.taskID,...pushContent(task),kind:task.kind},now());change();}res.json({task:outcome(task)});
 });
 app.get('/api/bridge/tasks',bridge,(req,res)=>res.json({tasks:store.outcomes()}));
 app.post('/api/bridge/tasks/clear',bridge,(req,res)=>{
  const tasks=req.body.tasks??[];
  if(!Array.isArray(tasks)||tasks.length>100)throw new HubError(400,'Supply at most 100 native notices');
  tasks.forEach(validateTask);
  const cleared=store.clear(req.body.taskIDs,'mac',tasks);change();res.json({cleared,tasks:req.body.taskIDs.map(id=>outcome(store.get(id)))});
 });
 app.post('/api/bridge/tasks/:id/ack',bridge,(req,res)=>{store.ack(req.params.id,req.body.version);res.json({ok:true});});
 app.post('/api/bridge/tasks/:id/resolve',bridge,(req,res)=>{try{const task=store.resolve(req.params.id,req.body.result,'mac',req.body.cancel===true);change();res.json({task:outcome(task)});}catch(e){if(e.status===409)return res.status(409).json({error:e.message,task:outcome(store.get(req.params.id))});throw e;}});
 app.use(express.static(fileURLToPath(new URL('../dist/',import.meta.url)),{index:'index.html',setHeaders(res,path){if(path.includes('/assets/'))res.setHeader('Cache-Control','public, max-age=31536000, immutable');}}));
 app.use((error,req,res,next)=>{if(res.headersSent)return next(error);res.status(error.status??500).json({error:error.status?error.message:'The service could not complete this request'});});
 let pumping=false;
 const pump=async()=>{
  if(!vapid||pumping)return;
  pumping=true;
  try{
   const groups=new Map();
   for(const row of store.db.prepare('SELECT * FROM outbox WHERE retry<=? ORDER BY id LIMIT 100').all(now())){
    const device=store.db.prepare('SELECT * FROM devices WHERE id=?').get(row.device);
    const body=JSON.parse(row.body);const task=store.get(body.id);
    const age=now()-Number(task.createdAt??now()/1000)*1000;
    if(!device?.subscription||task.status!=='pending'||!['approval','prompt'].includes(task.kind)&&age>1800000){store.db.prepare('DELETE FROM outbox WHERE id=?').run(row.id);continue;}
    if(!store.routing(now()).notifyPhone)continue;
    if(!groups.has(row.device))groups.set(row.device,{device,rows:[]});
    groups.get(row.device).rows.push({row,body,task});
   }
   for(const {device,rows} of groups.values()){
    // A backlog becomes one useful summary rather than a burst of old banners.
    const batches=rows.length>=3?[rows]:rows.map(row=>[row]);
    for(const candidates of batches){
     if(!store.routing(now()).notifyPhone)break;
     const batch=candidates.filter(x=>store.get(x.task.taskID).status==='pending');
     if(!batch.length)continue;
     const decisions=batch.filter(x=>['approval','prompt'].includes(x.task.kind)).length;
     const body=batch.length===1?{...batch[0].body,...pushContent(batch[0].task)}:{id:'inbox',kind:'digest',taskIDs:batch.map(x=>x.task.taskID),title:`${batch.length} requests waiting`,body:decisions?`${decisions} ${decisions===1?'decision needs':'decisions need'} your answer. Open your inbox to catch up.`:'Your agents have updates ready. Open your inbox to catch up.'};
     try{
      await push.sendNotification(JSON.parse(device.subscription),JSON.stringify({...body,title:String(body.title??'Hey Boss').slice(0,160),body:String(body.body??'').slice(0,500)}),{TTL:decisions?3600:300,timeout:10000});
      for(const {row} of batch)store.db.prepare('DELETE FROM outbox WHERE id=?').run(row.id);
     }catch(e){
      console.warn(JSON.stringify({event:'push_delivery_failed',status:e.statusCode??null,code:e.code??null,attempt:Math.max(...batch.map(x=>x.row.attempts))+1}));
      if([404,410].includes(e.statusCode)){
       store.db.prepare('UPDATE devices SET subscription=NULL WHERE id=?').run(device.id);
       store.db.prepare('DELETE FROM outbox WHERE device=?').run(device.id);
       break;
      }
      for(const {row} of batch){
       if(row.attempts>=8)store.db.prepare('DELETE FROM outbox WHERE id=?').run(row.id);
       else store.db.prepare('UPDATE outbox SET attempts=attempts+1,retry=? WHERE id=?').run(now()+Math.min(300000,1000*2**row.attempts),row.id);
      }
     }
    }
   }
  }finally{pumping=false;}
 };
 app.locals.pump=pump;app.locals.close=()=>{closeCheckpoint();closeWeb();closeAgents();for(const res of listeners)res.end();};return app;
}
if(process.argv[1]===fileURLToPath(import.meta.url)){
 const store=new HubStore();
 let keys=store.db.prepare("SELECT value FROM metadata WHERE key='vapid'").get();if(!keys){keys={value:JSON.stringify(webpush.generateVAPIDKeys())};store.db.prepare("INSERT INTO metadata VALUES('vapid',?)").run(keys.value);}const vapid={...JSON.parse(keys.value),subject:process.env.VAPID_SUBJECT??process.env.PUBLIC_ORIGIN};
 const app=createApp({store,hubToken:process.env.HUB_TOKEN,origin:process.env.PUBLIC_ORIGIN,secure:process.env.INSECURE_LOCAL!=='1',vapid,checkpoint:true});const server=app.listen(Number(process.env.PORT??8787),'0.0.0.0');const timer=setInterval(()=>app.locals.pump().catch(()=>{}),2000);timer.unref();
 for(const signal of ['SIGTERM','SIGINT'])process.on(signal,()=>{clearInterval(timer);app.locals.close();server.close(()=>{store.close();process.exit(0);});setTimeout(()=>process.exit(1),10000).unref();});
}
