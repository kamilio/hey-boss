import {DatabaseSync} from 'node:sqlite';
import {randomBytes,createHash,timingSafeEqual} from 'node:crypto';
export const hash = value => createHash('sha256').update(value).digest('hex');
export const token = () => randomBytes(32).toString('base64url');
export function equal(a,b){const x=Buffer.from(a??''),y=Buffer.from(b??'');return x.length===y.length&&timingSafeEqual(x,y);}
export class HubError extends Error{constructor(status,message){super(message);this.status=status;}}
export class HubStore{
 constructor(path=':memory:'){
  this.db=new DatabaseSync(path);this.db.exec(`PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;
   CREATE TABLE IF NOT EXISTS tasks(id TEXT PRIMARY KEY, body TEXT NOT NULL, status TEXT NOT NULL, answer TEXT, actor TEXT, version INTEGER NOT NULL, delivered INTEGER NOT NULL DEFAULT 0);
   CREATE TABLE IF NOT EXISTS devices(id TEXT PRIMARY KEY, secret TEXT UNIQUE NOT NULL, subscription TEXT, created INTEGER NOT NULL);
   CREATE TABLE IF NOT EXISTS pairing(code TEXT PRIMARY KEY, expires INTEGER NOT NULL);
   CREATE TABLE IF NOT EXISTS outbox(id INTEGER PRIMARY KEY, device TEXT NOT NULL, body TEXT NOT NULL, attempts INTEGER NOT NULL DEFAULT 0, retry INTEGER NOT NULL DEFAULT 0);
   CREATE TABLE IF NOT EXISTS metadata(key TEXT PRIMARY KEY,value TEXT NOT NULL);
   INSERT OR IGNORE INTO metadata VALUES('revision','0');`);
 }
 revision(){return Number(this.db.prepare("SELECT value FROM metadata WHERE key='revision'").get().value);}
 next(){this.db.exec("UPDATE metadata SET value=CAST(value AS INTEGER)+1 WHERE key='revision'");return this.revision();}
 transaction(fn){this.db.exec('BEGIN IMMEDIATE');try{const r=fn();this.db.exec('COMMIT');return r;}catch(e){this.db.exec('ROLLBACK');throw e;}}
 get(id){const r=this.db.prepare('SELECT * FROM tasks WHERE id=?').get(id);if(!r)throw new HubError(404,'This request is no longer available');return {...JSON.parse(r.body),status:r.status,result:r.answer,handledBy:r.actor,version:r.version};}
 list(){return this.db.prepare('SELECT id FROM tasks ORDER BY version DESC LIMIT 300').all().map(r=>this.get(r.id));}
 summaries(){return this.db.prepare("SELECT json_set(body,'$.description',substr(json_extract(body,'$.description'),1,4096),'$.question',substr(json_extract(body,'$.question'),1,4096)) AS body,status,answer,actor,version FROM tasks ORDER BY version DESC LIMIT 300").all().map(r=>({...JSON.parse(r.body),status:r.status,result:r.answer,handledBy:r.actor,version:r.version}));}
 outcomes(){return this.db.prepare("SELECT id AS taskID,status,answer AS result,actor AS handledBy,version FROM tasks WHERE status!='pending' AND delivered=0 LIMIT 20").all();}
 open(id){return this.transaction(()=>{
  const task=this.get(id);
  if(task.status!=='pending'||['approval','prompt'].includes(task.kind))return task;
  const version=this.next();this.db.prepare("UPDATE tasks SET status='ok',answer=NULL,actor='phone',version=?,delivered=0,body=json_set(body,'$.completedAt',?) WHERE id=? AND status='pending'").run(version,Date.now()/1000,id);
  return this.get(id);
 });}
 upsert(row){return this.transaction(()=>{
  const existing=this.db.prepare('SELECT status FROM tasks WHERE id=?').get(row.taskID);
  if(existing)return {task:this.get(row.taskID),created:false};
  const version=this.next();this.db.prepare('INSERT INTO tasks(id,body,status,answer,actor,version,delivered) VALUES(?,?,?,?,?,?,1)').run(row.taskID,JSON.stringify(row),'pending',null,null,version);
  return {task:this.get(row.taskID),created:true};
 });}
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
 ack(id,version){this.db.prepare('UPDATE tasks SET delivered=1 WHERE id=? AND version=?').run(id,version);}
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
 close(){this.db.close();}
}
