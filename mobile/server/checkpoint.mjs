import {randomUUID} from 'node:crypto';
import {HubError,equal} from './store.mjs';

// Fly has no writable user-data volume. A successful mutation is acknowledged
// only after the supervisor has fsynced its private checkpoint on the Mac.
export function checkpointRoutes(app,{store,bridge,enabled,hubToken,onRestore}){
 const epoch=randomUUID();let ready=!enabled,version=0,saved=0;
 const waiting=new Set();
 app.use('/api',(req,res,next)=>{
  if(!enabled||req.path.startsWith('/bridge/checkpoint'))return next();
  const durable=req.method!=='GET'&&!['/action','/mm','/preview','/inbox'].includes(req.path)&&(!req.path.startsWith('/bridge/')||/^\/bridge\/(tasks(?:\/|$)|issues\/[^/]+\/result|artifacts\/[^/]+\/result)/.test(req.path))&&!req.path.endsWith('/ack');
  if(!ready&&(!req.path.startsWith('/bridge/')||durable))return res.status(503).json({error:'Supervisor reconnecting. Retry when connected.'});
  if(!durable)return next();
  // Failed authentication must never reserve a checkpoint waiter.
  const secret=req.headers.cookie?.split(';').map(x=>x.trim()).find(x=>x.startsWith('hb_session='))?.slice(11);
  if(req.path.startsWith('/bridge/')?!equal(req.headers.authorization,'Bearer '+hubToken):req.path!=='/pair'&&!store.device(secret??''))return next();
  if(waiting.size>=128)return res.status(503).json({error:'Too many pending actions. Retry shortly.'});
  const json=res.json.bind(res);
  res.json=value=>{
   const entry={version:++version,res,value,json};
   const timer=setTimeout(()=>{waiting.delete(entry);if(!res.destroyed)json.call(res.status(503),{error:'Supervisor unavailable. This action is not yet acknowledged. Reconnect and retry.'});},8000);timer.unref();
   entry.timer=timer;waiting.add(entry);
   // A disconnected caller may still have changed state; checkpoint that state
   // so an identical retry sees the same accepted answer.
   res.on('close',()=>{clearTimeout(timer);waiting.delete(entry);});
   return res;
  };
  next();
 });
 app.get('/api/bridge/checkpoint',bridge,(req,res)=>res.json({epoch,ready,version,...(ready&&version>saved?{snapshot:store.snapshot()}: {})}));
 app.post('/api/bridge/checkpoint/restore',bridge,(req,res)=>{
  if(ready)throw new HubError(409,'The live hub cannot be restored again');
  if(req.body.snapshot)store.restore(req.body.snapshot);
  onRestore?.();
  ready=true;res.json({ok:true});
 });
 app.post('/api/bridge/checkpoint/ack',bridge,(req,res)=>{
  if(req.body.epoch===epoch&&Number.isSafeInteger(req.body.version)&&req.body.version>=saved&&req.body.version<=version){
   saved=req.body.version;
   for(const entry of waiting)if(entry.version<=saved){clearTimeout(entry.timer);waiting.delete(entry);if(!entry.res.destroyed)entry.json(entry.value);}
  }
  res.json({ok:true});
 });
 return ()=>{for(const entry of waiting){clearTimeout(entry.timer);if(!entry.res.destroyed)entry.json.call(entry.res.status(503),{error:'Service restarting. Retry when connected.'});}waiting.clear();};
}
