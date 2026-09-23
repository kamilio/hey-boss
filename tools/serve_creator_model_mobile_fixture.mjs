// Exercise the production paired-web relay against the isolated desktop fixture.
import {createApp} from '../mobile/server/index.mjs';
import {HubStore} from '../mobile/server/store.mjs';
const store=new HubStore();
store.setIssueProjects([{id:'named:Creator model QA',name:'Creator model QA'}]);
const key='synthetic-creator-model-qa-key'.repeat(3),base='http://127.0.0.1:59735',desktop='http://127.0.0.1:59734';
const app=createApp({store,hubToken:key,origin:base,secure:false});
app.get('/fixture-pairing',(_req,res)=>res.json({code:store.pairing()}));
const server=app.listen(59735,'127.0.0.1',()=>console.log(base));
const call=async(path,body)=>{
  const response=await fetch(base+path,{method:body?'POST':'GET',headers:{Authorization:'Bearer '+key,'Content-Type':'application/json'},body:body?JSON.stringify(body):undefined});
  return response.json();
};
let syncing=false;
const timer=setInterval(async()=>{
  if(syncing)return;
  syncing=true;
  try {
    const requests=(await call('/api/bridge/web')).requests;
    if(!requests.length)return;
    const bootstrap=await(await fetch(desktop+'/api/bootstrap')).json();
    for(const request of requests){
      const result=request.kind==='bootstrap'?bootstrap:request.kind==='inbox'?{ok:true,tasks:[],unread:0}:
        await(await fetch(desktop+'/api/'+request.kind,{method:'POST',headers:{'Content-Type':'application/json','X-Hey-Boss-CSRF':bootstrap.csrf},body:JSON.stringify(request.payload)})).json();
      await call('/api/bridge/web/'+request.id+'/result',result);
    }
  }catch(error){console.error(error.message);}finally{syncing=false;}
},100);
for(const signal of ['SIGTERM','SIGINT'])process.on(signal,()=>{
  clearInterval(timer);app.locals.close();server.close(()=>{store.close();process.exit(0);});
});
