// Paired production UI backed exclusively by a disposable native fixture.
import {createApp} from '../mobile/server/index.mjs';
import {HubStore} from '../mobile/server/store.mjs';
const native=process.argv[2];
if(!/^http:\/\/127\.0\.0\.1:\d+$/.test(native||''))throw Error('Supply the isolated native fixture base URL');
let boot=await(await fetch(native+'/api/bootstrap')).json();
const store=new HubStore();store.setIssueProjects(boot.projects);
const key='synthetic-markdown-attachment-fixture'.repeat(2);
const app=createApp({store,hubToken:key,origin:'http://127.0.0.1:52063',secure:false});
app.get('/fixture-pairing',(req,res)=>res.json({code:store.pairing()}));
const server=app.listen(52063,'127.0.0.1',()=>console.log(JSON.stringify({base:'http://127.0.0.1:'+server.address().port})));
let busy=false;
const timer=setInterval(async()=>{
 if(busy)return;busy=true;
 try{
  const headers={'Content-Type':'application/json',Authorization:'Bearer '+key};
  const queue=await(await fetch('http://127.0.0.1:52063/api/bridge/web',{headers})).json();
  for(const request of queue.requests){
   const response=await fetch(native+'/api/'+(request.kind==='bootstrap'?'bootstrap':request.kind),request.kind==='bootstrap'?{}:{method:'POST',headers:{'Content-Type':'application/json','X-Hey-Boss-CSRF':boot.csrf},body:JSON.stringify(request.payload)});
   await fetch('http://127.0.0.1:52063/api/bridge/web/'+request.id+'/result',{method:'POST',headers,body:JSON.stringify(await response.json().then(result=>{if(request.kind==='bootstrap')boot=result;return result;}))});
  }
  for(const request of store.pendingArtifacts()){
   const response=await fetch(native+'/api/action',{method:'POST',headers:{'Content-Type':'application/json','X-Hey-Boss-CSRF':boot.csrf},body:JSON.stringify({project:request.project,operation:request.operation,request_id:request.id})});
   store.finishArtifact(request.id,await response.json());
  }
 }catch(e){console.error(e.message);}finally{busy=false;}
},100);
for(const signal of ['SIGTERM','SIGINT'])process.on(signal,()=>{clearInterval(timer);app.locals.close();server.closeAllConnections();server.close(()=>{store.close();process.exit(0);});});
