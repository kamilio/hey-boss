import test from 'node:test';
import assert from 'node:assert/strict';
import {HubStore} from './store.mjs';
import {createApp} from './index.mjs';
test('agent history requires pairing, registered projects, bounded requests and authenticated bridge',async t=>{
 const store=new HubStore();store.setIssueProjects([{id:'named:Atlas',name:'Atlas'}]);const key='x'.repeat(64);
 const app=createApp({store,hubToken:key,secure:false,origin:'http://localhost'});const server=app.listen(0,'127.0.0.1');await new Promise(r=>server.once('listening',r));
 t.after(()=>{app.locals.close();server.close();store.close();});const base='http://127.0.0.1:'+server.address().port;
 const call=(path,body,headers={})=>fetch(base+path,{method:body?'POST':'GET',headers:{'Content-Type':'application/json',...headers},body:body?JSON.stringify(body):undefined});
 assert.equal((await call('/agents')).status,401);assert.equal((await call('/api/fleet/status')).status,401);
 assert.equal((await call('/api/bridge/agents')).status,401);
 const pair=await call('/api/pair',{code:store.pairing()});const headers={Cookie:pair.headers.get('set-cookie').split(';')[0]},bridge={Authorization:'Bearer '+key};
 assert.equal((await call('/api/fleet/status',null,headers)).status,503);
 const snapshot={ok:true,machines:[{host:'local',state:'connected',heartbeat:Date.now()/1000,workers:[{pid:1,runs:[{id:'run',project_id:'named:Atlas',title:'Fix reconnect'},{id:'private',project_id:'named:Hidden'}]}]}]};
 assert.equal((await call('/api/bridge/agents/status',snapshot,bridge)).status,200);
 const status=await(await call('/api/fleet/status',null,headers)).json();assert.equal(status.machines[0].workers[0].runs.length,1);
 assert.equal((await call('/api/fleet/conversation?host=local&run=private',null,headers)).status,404);
 assert.equal((await call('/api/fleet/assignment?project=named%3AAtlas&issue=4&agent=codex%3Aexact')).status,401);
 assert.equal((await call('/api/fleet/assignment?project=named%3AHidden&issue=4&agent=codex%3Aexact',null,headers)).status,404);
 assert.equal((await call('/api/fleet/assignment?project=named%3AAtlas&issue=0&agent=codex%3Aexact',null,headers)).status,400);
 const assignment=call('/api/fleet/assignment?project=named%3AAtlas&issue=4&agent=codex%3Aexact',null,headers);
 let assignmentRequests;
 for(let i=0;i<20;i++){assignmentRequests=(await(await call('/api/bridge/agents',null,bridge)).json()).requests;if(assignmentRequests.length)break;await new Promise(r=>setTimeout(r,10));}
 assert.equal(assignmentRequests[0].action,'assignment');
 assert.equal(assignmentRequests[0].issue,4);
 assert.equal(assignmentRequests[0].agent,'codex:exact');
 await call('/api/bridge/agents/'+assignmentRequests[0].id+'/result',{ok:true,machine:{host:'local'},run:{id:'session:exact',project_id:'named:Atlas',number:4,standalone:true}},bridge);
 assert.equal((await(await assignment).json()).run.id,'session:exact');
 assert.equal((await call('/api/fleet/conversation?host=local&run=run&cursor=-1',null,headers)).status,400);
 const reading=call('/api/fleet/conversation?host=local&run=run&cursor=0&latest=1&before=100',null,headers);
 let pending;for(let i=0;i<20;i++){pending=(await(await call('/api/bridge/agents',null,bridge)).json()).requests;if(pending.length)break;await new Promise(r=>setTimeout(r,10));}
 assert.equal(pending.length,1);assert.equal(pending[0].run,'run');assert.equal(pending[0].latest,true);assert.equal(pending[0].before,100);
 assert.equal((await call('/api/fleet/conversation?host=local&run=run&before=-1',null,headers)).status,400);
 await call('/api/bridge/agents/'+pending[0].id+'/result',{ok:true,messages:[{id:'0',role:'assistant',text:'**Done** <script>alert(1)</script>'}],cursor:100,has_more:false,availability:'available'},bridge);
 const result=await(await reading).json();assert.match(result.messages[0].html,/<strong>Done<\/strong>/);assert.ok(!result.messages[0].html.includes('<script>'));
 assert.equal((await call('/api/fleet/takeover',{host:'local',run:'run'})).status,401);
 assert.equal((await call('/api/fleet/takeover',{host:'local',run:'private'},headers)).status,404);
 assert.equal((await call('/api/fleet/takeover',{host:'local',run:'run'},{...headers,Origin:'https://evil.example'})).status,403);
 const takeover=call('/api/fleet/takeover',{host:'local',run:'run'},headers);
 for(let i=0;i<20;i++){pending=(await(await call('/api/bridge/agents',null,bridge)).json()).requests;if(pending.length)break;await new Promise(r=>setTimeout(r,10));}
 assert.equal(pending.length,1);assert.equal(pending[0].action,'takeover');assert.equal(pending[0].project,'named:Atlas');
 await call('/api/bridge/agents/'+pending[0].id+'/result',{ok:true,stopped:true,resume_command:'cd /repo && codex resume session'},bridge);
 assert.equal((await(await takeover).json()).resume_command,'cd /repo && codex resume session');
 const instruction={host:'local',run:'run',scope:'issue',text:'Preserve keyboard navigation',request_id:'steer-test'};
 assert.equal((await call('/api/fleet/steer',instruction)).status,401);
 assert.equal((await call('/api/fleet/steer',{...instruction,run:'private'},headers)).status,404);
 assert.equal((await call('/api/fleet/steer',{...instruction,run:'old',project:'named:Atlas'},headers)).status,404);
 assert.equal((await call('/api/fleet/steer',instruction,{...headers,Origin:'https://evil.example'})).status,403);
 for(const change of [{scope:'all'},{text:' '},{text:'🔥'.repeat(8001)},{request_id:'invalid id'}])assert.equal((await call('/api/fleet/steer',{...instruction,...change},headers)).status,400);
 const steering=call('/api/fleet/steer',instruction,headers);
 for(let i=0;i<20;i++){pending=(await(await call('/api/bridge/agents',null,bridge)).json()).requests;if(pending.length)break;await new Promise(r=>setTimeout(r,10));}
 assert.equal(pending[0].action,'steer');assert.equal(pending[0].scope,'issue');assert.equal(pending[0].text,instruction.text);assert.equal(pending[0].request_id,'steer-test');
 await call('/api/bridge/agents/'+pending[0].id+'/result',{ok:true,state:'queued'},bridge);
 assert.equal((await(await steering).json()).state,'queued');
 assert.equal((await call('/api/fleet/conversation?host=local&run=old&project=named%3AAtlas&at=-1',null,headers)).status,400);
 assert.equal((await call('/api/fleet/takeover',{host:'local',run:'old',project:'named:Atlas'},headers)).status,404,'Historical fallback never authorizes takeover');
 const historical=call('/api/fleet/conversation?host=local&run=old&project=named%3AAtlas&at=123',null,headers);
 for(let i=0;i<20;i++){pending=(await(await call('/api/bridge/agents',null,bridge)).json()).requests;if(pending.length)break;await new Promise(r=>setTimeout(r,10));}
 assert.equal(pending[0].run,'old');assert.equal(pending[0].at,123);assert.equal(pending[0].project,'named:Atlas');
 await call('/api/bridge/agents/'+pending[0].id+'/result',{ok:false,error:'This origin is not recorded'},bridge);
 assert.equal((await historical).status,503,'The supervisor must confirm the saved origin before serving history');
 store.setIssueProjects([]);assert.equal((await call('/api/fleet/conversation?host=local&run=run',null,headers)).status,404);
});
