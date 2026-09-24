// A paired production mobile app with a synthetic store and native artifact bridge.
import {createApp} from '../mobile/server/index.mjs';
import {HubStore} from '../mobile/server/store.mjs';
const native='http://127.0.0.1:59487',base='http://127.0.0.1:59489';
const boot=await(await fetch(native+'/api/bootstrap')).json();
const store=new HubStore();store.setIssueProjects(boot.projects);
const key='synthetic-artifact-browser-fixture'.repeat(2);
const app=createApp({store,hubToken:key,origin:base,secure:false});
app.get('/fixture-pairing',(req,res)=>res.json({code:store.pairing()}));
const server=app.listen(59489,'127.0.0.1',error=>{if(error)throw error;console.log('Synthetic paired artifact app '+base);});
let syncing=false;
const timer=setInterval(async()=>{if(syncing)return;syncing=true;try{
 // The current /artifacts page uses the shared, transient web relay. Keep the
 // legacy artifact queue below for referring-resource pages and recovery tests.
 const headers={'Content-Type':'application/json',Authorization:'Bearer '+key};
 const queue=await(await fetch(base+'/api/bridge/web',{headers})).json();
 for(const request of queue.requests){
  const response=await fetch(native+'/api/'+(request.kind==='bootstrap'?'bootstrap':request.kind),request.kind==='bootstrap'?{}:{method:'POST',headers:{'Content-Type':'application/json','X-Hey-Boss-CSRF':boot.csrf},body:JSON.stringify(request.payload)});
  await fetch(base+'/api/bridge/web/'+request.id+'/result',{method:'POST',headers,body:JSON.stringify(await response.json())});
 }
 for(const request of store.pendingArtifacts()){
  const reading=request.operation.action!=='artifact'||['list','view','links','preview'].includes(request.operation.operation.command);
  const response=await fetch(native+'/api/action',{method:'POST',headers:{'Content-Type':'application/json','X-Hey-Boss-CSRF':boot.csrf},body:JSON.stringify({project:request.project,operation:request.operation,request_id:reading?null:request.id})});
  const result=await response.json();store.finishArtifact(request.id,result);
 }
}catch(e){console.error(e.message);}finally{syncing=false;}},100);
for(const signal of ['SIGTERM','SIGINT'])process.on(signal,()=>{clearInterval(timer);app.locals.close();server.closeAllConnections();server.close(()=>{store.close();process.exit(0);});});
