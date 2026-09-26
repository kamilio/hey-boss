import {DatabaseSync} from 'node:sqlite';
import {randomBytes,createHash,timingSafeEqual} from 'node:crypto';
export const hash = value => createHash('sha256').update(value).digest('hex');
export const token = () => randomBytes(32).toString('base64url');
export function equal(a,b){const x=Buffer.from(a??''),y=Buffer.from(b??'');return x.length===y.length&&timingSafeEqual(x,y);}
export class HubError extends Error{constructor(status,message){super(message);this.status=status;}}
const checkpointTables=['tasks','devices','pairing','outbox','metadata','issue_creations','artifact_requests'];
export class HubStore{
 constructor(path=':memory:'){
  this.db=new DatabaseSync(path);this.db.exec(`PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;
   CREATE TABLE IF NOT EXISTS tasks(id TEXT PRIMARY KEY, body TEXT NOT NULL, status TEXT NOT NULL, answer TEXT, actor TEXT, version INTEGER NOT NULL, delivered INTEGER NOT NULL DEFAULT 0);
   CREATE TABLE IF NOT EXISTS devices(id TEXT PRIMARY KEY, secret TEXT UNIQUE NOT NULL, subscription TEXT, created INTEGER NOT NULL);
   CREATE TABLE IF NOT EXISTS pairing(code TEXT PRIMARY KEY, expires INTEGER NOT NULL);
   CREATE TABLE IF NOT EXISTS outbox(id INTEGER PRIMARY KEY, device TEXT NOT NULL, body TEXT NOT NULL, attempts INTEGER NOT NULL DEFAULT 0, retry INTEGER NOT NULL DEFAULT 0);
   CREATE TABLE IF NOT EXISTS metadata(key TEXT PRIMARY KEY,value TEXT NOT NULL);
   CREATE TABLE IF NOT EXISTS issue_creations(id TEXT PRIMARY KEY,device TEXT NOT NULL,body TEXT NOT NULL,status TEXT NOT NULL DEFAULT 'pending',number INTEGER,error TEXT,created INTEGER NOT NULL);
   CREATE INDEX IF NOT EXISTS issue_creations_pending ON issue_creations(status,created,id);
   CREATE INDEX IF NOT EXISTS issue_creations_device ON issue_creations(device,created DESC);
   CREATE TABLE IF NOT EXISTS artifact_requests(id TEXT PRIMARY KEY,device TEXT NOT NULL,payload TEXT NOT NULL,status TEXT NOT NULL DEFAULT 'pending',result TEXT,created INTEGER NOT NULL);
   CREATE INDEX IF NOT EXISTS artifact_requests_pending ON artifact_requests(status,created);
   INSERT OR IGNORE INTO metadata VALUES('revision','0');`);
 }
 revision(){return Number(this.db.prepare("SELECT value FROM metadata WHERE key='revision'").get().value);}
 ensureRoom(extra){
  const {bytes}=this.db.prepare("SELECT coalesce((SELECT sum(length(cast(body AS BLOB))) FROM tasks),0)+coalesce((SELECT sum(length(cast(body AS BLOB))) FROM issue_creations),0)+coalesce((SELECT sum(length(cast(payload AS BLOB))+coalesce(length(cast(result AS BLOB)),0)) FROM artifact_requests),0) AS bytes").get();
  if(bytes+extra>32*1048576)throw new HubError(503,'The relay is full. Read pending notices and reconnect the supervisor before retrying.');
 }
 snapshot(){return Object.fromEntries(checkpointTables.map(table=>[table,this.db.prepare('SELECT * FROM '+table).all()]));}
 restore(snapshot){return this.transaction(()=>{
  if(!snapshot||checkpointTables.some(table=>!Array.isArray(snapshot[table])))throw new HubError(400,'Invalid supervisor checkpoint');
  for(const table of checkpointTables){
   const columns=this.db.prepare('PRAGMA table_info('+table+')').all().map(row=>row.name);
   this.db.exec('DELETE FROM '+table);
   const insert=this.db.prepare(`INSERT INTO ${table} (${columns.join(',')}) VALUES (${columns.map(()=>'?').join(',')})`);
   for(const row of snapshot[table]){
    if(Object.keys(row).length!==columns.length||columns.some(column=>!(column in row)))throw new HubError(400,'Invalid supervisor checkpoint row');
    insert.run(...columns.map(column=>row[column]));
   }
  }
 });}
 next(){this.db.exec("UPDATE metadata SET value=CAST(value AS INTEGER)+1 WHERE key='revision'");return this.revision();}
 transaction(fn){this.db.exec('BEGIN IMMEDIATE');try{const r=fn();this.db.exec('COMMIT');return r;}catch(e){this.db.exec('ROLLBACK');throw e;}}
 get(id){const r=this.db.prepare('SELECT * FROM tasks WHERE id=?').get(id);if(!r)throw new HubError(404,'This request is no longer available');return {...JSON.parse(r.body),status:r.status,result:r.answer,handledBy:r.actor,version:r.version};}
 list(){return this.db.prepare('SELECT id FROM tasks ORDER BY version DESC LIMIT 300').all().map(r=>this.get(r.id));}
 summaries(){return this.db.prepare("SELECT json_set(body,'$.description',substr(json_extract(body,'$.description'),1,4096),'$.question',substr(json_extract(body,'$.question'),1,4096)) AS body,status,answer,actor,version FROM tasks WHERE status='pending' OR id IN (SELECT id FROM tasks WHERE status!='pending' ORDER BY version DESC LIMIT 300) ORDER BY version DESC").all().map(r=>({...JSON.parse(r.body),status:r.status,result:r.answer,handledBy:r.actor,version:r.version}));}
 outcomes(){return this.db.prepare("SELECT id AS taskID,status,answer AS result,actor AS handledBy,version FROM tasks WHERE status!='pending' AND delivered=0 LIMIT 20").all();}
 clear(ids,actor='phone',rows=[]){return this.transaction(()=>{
  if(!Array.isArray(ids)||!ids.length||ids.length>10000||new Set(ids).size!==ids.length||ids.some(id=>typeof id!=='string'||!id.trim()||Buffer.byteLength(id)>256||/[\p{Cc}]/u.test(id)))throw new HubError(400,'Select 1–10000 unique notice IDs');
  const find=this.db.prepare('SELECT status FROM tasks WHERE id=?');
  // Seed only this explicit snapshot, in the same transaction as dismissal.
  // Existing outcomes win; there is no intermediate pending push notification.
  const selected=new Set(ids);
  if(new Set(rows.map(row=>row.taskID)).size!==rows.length||rows.some(row=>!selected.has(row.taskID)))throw new HubError(400,'Native notices must match the selected snapshot');
  for(const row of rows)if(!find.get(row.taskID))this.insertTask(row);
  for(const id of ids)if(!find.get(id))throw new HubError(404,'This request is no longer available');
  const update=this.db.prepare("UPDATE tasks SET status=CASE WHEN json_extract(body,'$.kind') IN ('approval','prompt') OR json_extract(body,'$.commentsEnabled')=1 THEN 'cancelled' ELSE 'ok' END,answer=NULL,actor=?,version=?,delivered=0,body=json_set(body,'$.completedAt',?) WHERE id=? AND status='pending'");
  let cleared=0;const completedAt=Date.now()/1000;
  for(const id of ids)if(find.get(id).status==='pending'){update.run(actor,this.next(),completedAt,id);cleared++;}
  return cleared;
 });}
 open(id){return this.transaction(()=>{
  const task=this.get(id);
  if(task.status!=='pending'||['approval','prompt'].includes(task.kind))return task;
  const version=this.next();this.db.prepare("UPDATE tasks SET status='ok',answer=NULL,actor='phone',version=?,delivered=0,body=json_set(body,'$.completedAt',?) WHERE id=? AND status='pending'").run(version,Date.now()/1000,id);
  return this.get(id);
 });}
 upsert(row){return this.transaction(()=>{
  const existing=this.db.prepare('SELECT status FROM tasks WHERE id=?').get(row.taskID);
  if(existing)return {task:this.get(row.taskID),created:false};
  this.insertTask(row);
  return {task:this.get(row.taskID),created:true};
 });}
 insertTask(row){
  const body=JSON.stringify(row);this.ensureRoom(Buffer.byteLength(body));
  this.db.prepare('INSERT INTO tasks(id,body,status,answer,actor,version,delivered) VALUES(?,?,?,?,?,?,1)').run(row.taskID,body,'pending',null,null,this.next());
 }
 resolve(id,result,actor,cancel=false){return this.transaction(()=>{
  const task=this.get(id);if(task.status!=='pending')throw new HubError(409,'Already handled on '+(task.handledBy==='mac'?'your Mac':'another device'));
  if(!cancel){
   if(task.kind==='approval'&&!task.options.includes(result))throw new HubError(400,'Choose one of the available answers');
   if(task.kind==='prompt'&&(typeof result!=='string'||!result.trim()||Buffer.byteLength(result)>16384))throw new HubError(400,'Enter an answer of up to 16 KiB');
   if(!['prompt','approval'].includes(task.kind)&&result!=null)throw new HubError(400,'This notification does not accept an answer');
  }
  const status=cancel&&(['prompt','approval'].includes(task.kind)||task.commentsEnabled)?'cancelled':'ok';
  const version=this.next();this.db.prepare('UPDATE tasks SET status=?,answer=?,actor=?,version=?,delivered=0,body=json_set(body,\'$.completedAt\',?) WHERE id=? AND status=\'pending\'').run(status,cancel?null:result??null,actor,version,Date.now()/1000,id);return this.get(id);
 });}
 terminal(){return this.db.prepare("SELECT id FROM tasks WHERE status!='pending' AND delivered=0 LIMIT 20").all().map(r=>this.get(r.id));}
 ack(id,version){this.db.prepare('UPDATE tasks SET delivered=1 WHERE id=? AND version=?').run(id,version);
  // Native history is authoritative; the phone already shows only 300 past rows.
  this.db.exec("DELETE FROM tasks WHERE status!='pending' AND delivered=1 AND id NOT IN (SELECT id FROM tasks WHERE status!='pending' ORDER BY version DESC LIMIT 300); DELETE FROM outbox WHERE json_extract(body,'$.id') NOT IN (SELECT id FROM tasks)");
 }
 pairing(){const code=randomBytes(5).toString('hex').toUpperCase();this.db.prepare('DELETE FROM pairing WHERE expires<?').run(Date.now());this.db.prepare('INSERT INTO pairing VALUES(?,?)').run(hash(code),Date.now()+300000);return code;}
 pair(code){return this.transaction(()=>{
  const r=this.db.prepare('DELETE FROM pairing WHERE code=? AND expires>? RETURNING code').get(hash(code.toUpperCase()),Date.now());if(!r)throw new HubError(401,'Pairing code expired or incorrect');
  const secret=token(),id=token();this.db.prepare('INSERT INTO devices VALUES(?,?,NULL,?)').run(id,hash(secret),Date.now());return {id,secret};
 });}
 device(secret){return this.db.prepare('SELECT * FROM devices WHERE secret=?').get(hash(secret));}
 enqueue(body,now=Date.now()){
  const data=JSON.stringify(body),eligible=this.preferences().mode==='automatic'?now+30000:now;
  this.db.prepare('INSERT INTO outbox(device,body,retry) SELECT id,?,? FROM devices WHERE subscription IS NOT NULL').run(data,eligible);
 }
 preferences(){const row=this.db.prepare("SELECT value FROM metadata WHERE key='notification_preferences'").get();return row?JSON.parse(row.value):{mode:'automatic',awayAfterSeconds:600};}
 setPreferences(value){if(!['automatic','always','off'].includes(value.mode)||![60,120,300,600,900].includes(value.awayAfterSeconds))throw new HubError(400,'Choose an away delay of 1, 2, 5, 10 or 15 minutes');this.db.prepare("INSERT OR REPLACE INTO metadata VALUES('notification_preferences',?)").run(JSON.stringify({mode:value.mode,awayAfterSeconds:value.awayAfterSeconds}));return this.preferences();}
 presence(value,now=Date.now()){
  if(typeof value.unavailable!=='boolean'||!Number.isFinite(value.idleSeconds)||value.idleSeconds<0||value.idleSeconds>604800)throw new HubError(400,'Invalid Mac presence');
  const raw=this.db.prepare("SELECT value FROM metadata WHERE key='mac_presence'").get(),previous=raw?JSON.parse(raw.value):null;
  const idleReliable=value.idleReliable===true;
  const confirming=idleReliable&&!value.unavailable&&value.idleSeconds>=this.preferences().awayAfterSeconds;
  const awaySince=confirming?(previous?.awaySince!=null&&now>=previous.seenAt&&now-previous.seenAt<=15000?previous.awaySince:now):null;
  this.db.prepare("INSERT OR REPLACE INTO metadata VALUES('mac_presence',?)").run(JSON.stringify({idleSeconds:value.idleSeconds,idleReliable,unavailable:value.unavailable,seenAt:now,awaySince}));
 }
 routing(now=Date.now()){
  const preferences=this.preferences();const raw=this.db.prepare("SELECT value FROM metadata WHERE key='mac_presence'").get();const presence=raw?JSON.parse(raw.value):null;
  const age=presence?now-presence.seenAt:Infinity,stale=age>120000||age<0;
  const reliable=presence?.idleReliable===true&&!stale;
  const confirmed=reliable&&presence.awaySince!=null&&presence.seenAt-presence.awaySince>=60000;
  const state=!presence?'unknown':stale?'offline':presence.unavailable?'locked':confirmed?'away':reliable&&presence.awaySince!=null?'confirming':'active';
  return {...preferences,macState:state,macIdleSeconds:reliable?presence.idleSeconds+Math.max(0,age)/1000:null,notifyPhone:preferences.mode==='always'||preferences.mode==='automatic'&&['away','locked','offline'].includes(state)};
 }
 issueProjects(){const row=this.db.prepare("SELECT value FROM metadata WHERE key='issue_projects'").get();return row?JSON.parse(row.value):[];}
 setIssueProjects(projects){
  if(!Array.isArray(projects)||projects.length>10000||projects.some(p=>typeof p.id!=='string'||!p.id.trim()||Buffer.byteLength(p.id)>8192||typeof p.name!=='string'||!p.name.trim()||Buffer.byteLength(p.name)>1024))throw new HubError(400,'Invalid project registry');
  const names=new Map();
  for(const {id,name} of projects){
   const key=name.toLowerCase();
   if(!names.has(key))names.set(key,{id,name});
  }
  this.db.prepare("INSERT OR REPLACE INTO metadata VALUES('issue_projects',?)").run(JSON.stringify([...names.values()]));
  this.db.prepare("INSERT OR REPLACE INTO metadata VALUES('issue_bridge_seen',?)").run(String(Date.now()));
 }
 issueConnected(){const row=this.db.prepare("SELECT value FROM metadata WHERE key='issue_bridge_seen'").get();return !!row&&Date.now()-Number(row.value)<30000;}
 issueCreation(row){return {...JSON.parse(row.body),status:row.status,number:row.number,error:row.error,created:row.created};}
 issueCreations(device){return this.db.prepare('SELECT * FROM issue_creations WHERE device=? ORDER BY created DESC LIMIT 100').all(device).map(row=>this.issueCreation(row));}
 issueSummaries(device){return this.db.prepare("SELECT json_remove(body,'$.body') AS body,status,number,error,created FROM issue_creations WHERE device=? ORDER BY created DESC LIMIT 100").all(device).map(row=>this.issueCreation(row));}
 getIssueCreation(device,id){const row=this.db.prepare('SELECT * FROM issue_creations WHERE device=? AND id=?').get(device,id);if(!row)throw new HubError(404,'This submission is no longer available');return this.issueCreation(row);}
 pendingIssues(){return this.db.prepare("SELECT * FROM issue_creations WHERE status='pending' ORDER BY created,id LIMIT 5").all().map(row=>this.issueCreation(row));}
 createIssue(device,value){return this.transaction(()=>{
  const identifier=(text,max)=>typeof text==='string'&&text.trim()&&Buffer.byteLength(text)<=max&&!/[\p{Cc}]/u.test(text);
  if(!value||typeof value.requestID!=='string'||!/^[-a-zA-Z0-9_]{1,128}$/.test(value.requestID))throw new HubError(400,'Invalid creation request ID; reload and try again');
  if(!identifier(value.title,512))throw new HubError(400,'Enter a title of up to 512 bytes without control characters');
  const body=value.body??'',labels=value.labels??[];
  if(typeof body!=='string'||Buffer.byteLength(body)>1048576)throw new HubError(400,'Description must be text up to 1 MiB');
  if(!Array.isArray(labels)||labels.length>50||labels.some(label=>!identifier(label,64)))throw new HubError(400,'Use up to 50 labels, each up to 64 bytes without control characters');
  const payload=JSON.stringify({requestID:value.requestID,project:value.project,title:value.title,body,labels});
  const existing=this.db.prepare('SELECT * FROM issue_creations WHERE id=?').get(value.requestID);
  // Accepted retries remain readable even if their project later disappears.
  if(existing){if(existing.device!==device||existing.body!==payload)throw new HubError(409,'This request ID already belongs to another submission');return this.issueCreation(existing);}
  this.ensureRoom(Buffer.byteLength(payload));
  if(!this.issueProjects().some(project=>project.id===value.project||typeof value.project==='string'&&project.name.toLowerCase()===value.project.toLowerCase()))throw new HubError(400,'Choose a registered project. Reconnect the supervisor to refresh projects.');
  if(this.db.prepare("SELECT COUNT(*) AS n FROM issue_creations WHERE status='pending'").get().n>=10000)throw new HubError(503,'The issue queue is full. Keep this draft and retry after the supervisor reconnects.');
  this.db.prepare('INSERT INTO issue_creations(id,device,body,created) VALUES(?,?,?,?)').run(value.requestID,device,payload,Date.now());this.next();
  return this.issueCreation(this.db.prepare('SELECT * FROM issue_creations WHERE id=?').get(value.requestID));
 });}
 finishIssue(id,result){
  if(!result||!['synced','error'].includes(result.status)||result.status==='synced'&&(!Number.isSafeInteger(result.number)||result.number<1)||result.status==='error'&&(typeof result.error!=='string'||!result.error.trim()||Buffer.byteLength(result.error)>4096))throw new HubError(400,'Invalid issue delivery result');
  if(!this.db.prepare('SELECT id FROM issue_creations WHERE id=?').get(id))throw new HubError(404,'Creation request not found');
  const r=this.db.prepare("UPDATE issue_creations SET status=?,number=?,error=? WHERE id=? AND status='pending'").run(result.status,result.number??null,result.error??null,id);if(r.changes)this.next();
 }
 close(){this.db.close();}
 artifactRequest(device,value){return this.transaction(()=>{
  const commands=['list','view','preview','create','edit','import','archive','delete','comment','resolve','link','unlink','links'];
  const attachment=value?.operation?.action==='attachment'&&['list','upload','download','remove'].includes(value.operation.operation?.command);
  const resourceRead=['view','status_view','status_history'].includes(value?.operation?.action)||value?.operation?.action==='mindmap'&&['view','show'].includes(value.operation.operation?.command);
  if(!value||!attachment&&!resourceRead&&(value.operation?.action!=='artifact'||!commands.includes(value.operation.operation?.command))||value.host)throw new HubError(400,'Only project artifact operations are accepted');
  const reading=resourceRead||['list','view','links','preview','download'].includes(value.operation.operation.command);
  if(!reading&&(typeof value.request_id!=='string'||!/^[-a-zA-Z0-9_]{1,128}$/.test(value.request_id)))throw new HubError(400,'An artifact mutation request ID is required');
  const id=reading?token():value.request_id;
  const payload=JSON.stringify({project:value.project,operation:value.operation});
  if(attachment&&value.operation.operation.command==='upload'){
   const data=value.operation.operation.data;
   if(typeof data!=='string'||data.length>Math.ceil(10*1048576/3)*4||Buffer.from(data,'base64').length>10*1048576)throw new HubError(400,'Attachments must be at most 10 MiB');
  }
  if(Buffer.byteLength(payload)>(attachment||value.operation.operation?.command==='import'?16:2)*1048576)throw new HubError(400,'Artifact request is too large');
  const existing=this.db.prepare('SELECT * FROM artifact_requests WHERE id=?').get(id);
  if(existing){if(existing.device!==device||existing.payload!==payload)throw new HubError(409,'ID belongs to another request');return this.artifactResult(device,id);}
  this.ensureRoom(Buffer.byteLength(payload));
  if(!this.issueProjects().some(p=>p.id===value.project))throw new HubError(400,'Choose a registered project; reconnect the supervisor to refresh projects');
  if(this.db.prepare("SELECT count(*) AS n FROM artifact_requests WHERE device=? AND status='pending'").get(device).n>=100)throw new HubError(503,'Too many pending requests; reconnect the supervisor');
  // This is a transport journal, never an authoritative artifact store.
  this.db.prepare("DELETE FROM artifact_requests WHERE status='done' AND created<?").run(Date.now()-7*86400000);
  this.db.prepare('INSERT INTO artifact_requests(id,device,payload,created) VALUES(?,?,?,?)').run(id,device,payload,Date.now());
  return this.artifactResult(device,id);
 });}
 artifactResult(device,id){const row=this.db.prepare('SELECT * FROM artifact_requests WHERE device=? AND id=?').get(device,id);if(!row)throw new HubError(404,'Artifact request not found');return {id:row.id,status:row.status,result:row.result?JSON.parse(row.result):null};}
 pendingArtifacts(){
  const result=[];let bytes=32;
  for(const row of this.db.prepare("SELECT id,length(cast(payload AS BLOB)) AS size FROM artifact_requests WHERE status='pending' ORDER BY created,id LIMIT 10").all()){
   if(bytes+row.size+256>16*1048576)break;
   const payload=this.db.prepare('SELECT payload FROM artifact_requests WHERE id=?').get(row.id).payload;
   result.push({id:row.id,...JSON.parse(payload)});bytes+=row.size+256;
  }
  return result;
 }
 finishArtifact(id,result){const data=JSON.stringify(result);if(typeof result?.ok!=='boolean'||Buffer.byteLength(data)>32*1048576)throw new HubError(400,'Invalid artifact transport result');const row=this.db.prepare('SELECT status FROM artifact_requests WHERE id=?').get(id);if(!row)throw new HubError(404,'Artifact request not found');if(row.status==='pending'){this.ensureRoom(Buffer.byteLength(data));this.db.prepare("UPDATE artifact_requests SET status='done',result=? WHERE id=? AND status='pending'").run(data,id);}}
}
