// HTTP responses and payloads exist only while a paired request is in flight.
import {randomUUID} from 'node:crypto';
import {fileURLToPath} from 'node:url';
import {HubError} from './store.mjs';

export function webRoutes(app,{auth,bridge}){
 const pending=new Map();let bytes=0;
 function remove(id){const request=pending.get(id);if(request){bytes-=request.bytes;pending.delete(id);}return request;}
 function relay(req,res,kind){
  const payload=kind==='bootstrap'?null:req.body;
  if(payload?.host)throw new HubError(400,'Use the supervisor for mobile issue access');
  if(pending.size>=64||[...pending.values()].filter(p=>p.device===req.device.id).length>=8)throw new HubError(429,'Wait for the current requests to finish');
  const size=Buffer.byteLength(JSON.stringify(payload));
  if(bytes+size>32*1048576)throw new HubError(503,'Wait for the current document transfers to finish');
  const id=randomUUID();
  const timer=setTimeout(()=>{remove(id);if(!res.destroyed)res.status(504).json({ok:false,error:{code:'offline',message:'Supervisor unavailable. Reconnect and retry; your draft is preserved.'}});},25000);timer.unref();
  pending.set(id,{id,kind,payload,device:req.device.id,res,timer,bytes:size});bytes+=size;
  res.on('close',()=>{clearTimeout(timer);remove(id);});
 }
 for(const route of ['/issues','/mm'])app.get(route,auth,(req,res)=>res.sendFile(fileURLToPath(new URL(`../dist/issue-web/${route==='/mm'?'mindmap':'index'}.html`,import.meta.url))));
 app.get('/api/bootstrap',auth,(req,res)=>relay(req,res,'bootstrap'));
 for(const [route,kind] of [['action','action'],['mm','action'],['preview','preview'],['inbox','inbox']])app.post('/api/'+route,auth,(req,res)=>relay(req,res,kind));
 app.get('/api/bridge/web',bridge,(req,res)=>res.json({requests:[...pending.values()].map(({id,kind,payload})=>({id,kind,payload}))}));
 app.post('/api/bridge/web/:id/result',bridge,(req,res)=>{
  const request=remove(req.params.id);
  if(request){clearTimeout(request.timer);request.res.json(req.body);}
  res.json({ok:true});
 });
 return ()=>{for(const p of pending.values()){clearTimeout(p.timer);p.res.status(503).json({ok:false,error:{message:'Service restarting. Retry after reconnecting.'}});}pending.clear();bytes=0;};
}
