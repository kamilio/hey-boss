// Synthetic paired web fixture, bridged to serve_agents_fixture.mjs.
import {createApp} from '../mobile/server/index.mjs';
import {HubStore} from '../mobile/server/store.mjs';
const store=new HubStore();store.setIssueProjects([{id:'named:Atlas',name:'Atlas'},{id:'named:Hey Boss',name:'Hey Boss'},{id:'named:Studio',name:'Studio'}]);
const key='synthetic-agent-visual-fixture-key'.repeat(2),base='http://127.0.0.1:59642';
const app=createApp({store,hubToken:key,origin:base,secure:false});
app.get('/fixture-pairing',(req,res)=>res.json({code:store.pairing()}));
const server=app.listen(59642,'127.0.0.1',()=>console.log(JSON.stringify({url:base,code:store.pairing()})));
const call=async(path,body)=>{const response=await fetch(base+path,{method:body?'POST':'GET',headers:{Authorization:'Bearer '+key,'Content-Type':'application/json'},body:body?JSON.stringify(body):undefined});return response.json();};
let syncing=false;
const timer=setInterval(async()=>{if(syncing)return;syncing=true;try{
 const status=await(await fetch('http://127.0.0.1:59641/api/fleet/status')).json();await call('/api/bridge/agents/status',status);
 for(const request of (await call('/api/bridge/agents')).requests){const response=await fetch('http://127.0.0.1:59641/api/fleet/conversation?'+new URLSearchParams({host:request.host,run:request.run,cursor:request.cursor,...(request.latest?{latest:1}:{}),...(request.before==null?{}:{before:request.before})}));await call('/api/bridge/agents/'+request.id+'/result',await response.json());}
}catch{}finally{syncing=false;}},500);
for(const signal of ['SIGTERM','SIGINT'])process.on(signal,()=>{clearInterval(timer);app.locals.close();server.close(()=>process.exit(0));});
