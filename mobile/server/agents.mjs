// Conversations cross the existing authenticated bridge only on demand.
import {fileURLToPath} from 'node:url';
import React from 'react';
import {renderToStaticMarkup} from 'react-dom/server';
import Markdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import {randomUUID} from 'node:crypto';
import {HubError} from './store.mjs';
export function agentRoutes(app,{auth,bridge,store,now}) {
 let snapshot=null,seen=0;const pending=new Map();
 const visible=()=>new Set(store.issueProjects().map(p=>p.id));
 const filtered=()=>{
  const projects=visible();return {...snapshot,signals:snapshot?.signals||[],conflicts:[],events:[],machines:(snapshot?.machines||[]).map(m=>({host:m.host,hostname:m.hostname,state:m.state,heartbeat:m.heartbeat,desired_revision:m.desired_revision,applied_revision:m.applied_revision,configuration_error:m.configuration_error,workers:(m.workers||[]).map(w=>({id:w.id,pid:w.pid,intent:w.intent,managed:w.managed,retiring:w.retiring,active:w.active,free:w.free,eligible:w.eligible,error:w.error,retry_at:w.retry_at,config:{name:w.config?.name,enabled:w.config?.enabled,concurrency:w.config?.concurrency,projects:w.config?.projects,directory:w.config?.directory,directories:w.config?.directories},chiefs:(w.chiefs||[]).filter(r=>projects.has(r.project_id)),runs:(w.runs||[]).filter(r=>projects.has(r.project_id))}))}))};
 };
 const page=fileURLToPath(new URL('../dist/agent-web/fleet.html',import.meta.url));
 for(const route of ['/agents','/agents/session','/workers'])app.get(route,auth,(req,res)=>res.sendFile(page));
 app.get('/api/agent-bootstrap',auth,(req,res)=>res.json({projects:store.issueProjects()}));
 app.get('/api/fleet/status',auth,(req,res)=>{if(!snapshot)return res.status(503).json({error:'Connect your supervisor to see agents.'});const value=filtered();if(now()-seen>15000)for(const m of value.machines)m.state='disconnected';res.json(value);});
 function requestAgent(req,res,action){
  const input=['conversation','assignment'].includes(action)?req.query:req.body;
  const issue=Number(input.issue),agent=input.agent;
  const {scope,text,request_id}=input;
  if(action==='steer'&&(!['session','issue','project'].includes(scope)||typeof text!=='string'||!text.trim()||Buffer.byteLength(text)>32000||typeof request_id!=='string'||!/^[a-zA-Z0-9_-]{1,128}$/.test(request_id)))throw new HubError(400,'Choose a scope and enter an instruction of up to 32000 bytes.');
  const cursor=Number(input.cursor??0),before=input.before==null?null:Number(input.before),at=input.at==null?null:Number(input.at),latest=input.latest==='1',host=input.host,run=input.run;
  if(action==='assignment'){
   if(!Number.isSafeInteger(issue)||issue<=0||typeof agent!=='string'||!agent.startsWith('codex:')||agent.length>128)throw new HubError(400,'Invalid assignment request');
  }else if((before!==null&&(!Number.isSafeInteger(before)||before<0))||(req.query.latest!=null&&!['0','1'].includes(req.query.latest))||!Number.isSafeInteger(cursor)||cursor<0||typeof host!=='string'||typeof run!=='string')throw new HubError(400,'Invalid conversation request');
  if(at!==null&&(!Number.isSafeInteger(at)||at<0))throw new HubError(400,'Invalid invocation cursor');
  const entry=filtered().machines.find(m=>m.host===host||m.hostname===host)?.workers.flatMap(w=>[...w.runs,...w.chiefs]).find(r=>r.id===run);
  if(entry?.kind==='chief'&&!['conversation','assignment'].includes(action))throw new HubError(400,'Chief conversations are read-only');
  // Historical origin references are validated by the authoritative supervisor.
  const project=entry?.project_id||(['conversation','assignment'].includes(action)&&visible().has(input.project)?input.project:null);
  if(!project)throw new HubError(404,'This conversation is no longer available');
  if(now()-seen>15000)throw new HubError(503,'Connect your supervisor to load this conversation.');
  if(pending.size>=32||[...pending.values()].filter(p=>p.device===req.device.id).length>=2)throw new HubError(429,'Wait for your current conversation to load.');
  const id=randomUUID();const timer=setTimeout(()=>{pending.delete(id);if(!res.destroyed)res.status(504).json({error:'The device did not respond. Try again when it reconnects.'});},20000);timer.unref();
  pending.set(id,{id,action,host,run,cursor,before,latest,at,project,issue,agent,scope,text,request_id,device:req.device.id,res,timer});
  res.on('close',()=>{clearTimeout(timer);pending.delete(id);});
 }
 function requestFleet(req,res,action){
  if(!snapshot||now()-seen>15000)throw new HubError(503,'Connect your supervisor to edit worker configuration.');
  const input=req.method==='GET'?{}:req.body;
  if(action==='configuration'&&req.method==='POST'&&((typeof input.text!=='string'&&!input.worker_update)||Buffer.byteLength(JSON.stringify(input))>1048576||typeof input.revision!=='string'||typeof input.save!=='boolean'))throw new HubError(400,'Provide YAML text, its revision, and whether to save.');
  if(action==='signal'&&(!['pause','resume','stop','restart'].includes(input.signal)||typeof input.host!=='string'||typeof input.worker!=='string'||typeof input.id!=='string'))throw new HubError(400,'Invalid worker action');
  if(pending.size>=32||[...pending.values()].filter(p=>p.device===req.device.id).length>=2)throw new HubError(429,'Wait for your current request to finish.');
  const id=randomUUID(),timer=setTimeout(()=>{pending.delete(id);if(!res.destroyed)res.status(504).json({error:'No confirmation from your supervisor. Reload to check whether the change was saved.'});},20000);timer.unref();
  pending.set(id,{id,action,text:input.text,worker_update:input.worker_update,revision:input.revision,save:input.save,host:input.host,worker:input.worker,signal:input.signal,signal_id:input.id,device:req.device.id,res,timer});
  res.on('close',()=>{clearTimeout(timer);pending.delete(id);});
 }
 app.get('/api/fleet/configuration',auth,(req,res)=>requestFleet(req,res,'configuration'));
 app.post('/api/fleet/configuration',auth,(req,res)=>requestFleet(req,res,'configuration'));
 app.post('/api/fleet',auth,(req,res)=>requestFleet(req,res,'signal'));
 app.get('/api/fleet/conversation',auth,(req,res)=>requestAgent(req,res,'conversation'));
 app.get('/api/fleet/assignment',auth,(req,res)=>requestAgent(req,res,'assignment'));
 app.post('/api/fleet/takeover',auth,(req,res)=>requestAgent(req,res,'takeover'));
 app.post('/api/fleet/steer',auth,(req,res)=>requestAgent(req,res,'steer'));
 app.post('/api/bridge/agents/status',bridge,(req,res)=>{
  if(!Array.isArray(req.body.machines)||req.body.machines.length>100)throw new HubError(400,'Invalid agent snapshot');
  snapshot=req.body;seen=now();res.json({ok:true});
 });
 app.get('/api/bridge/agents',bridge,(req,res)=>res.json({requests:[...pending.values()].map(({id,action,host,run,cursor,before,latest,at,project,issue,agent,scope,text,request_id,revision,save,worker,signal,signal_id,worker_update})=>({id,action,host,run,cursor,before,latest,at,project,issue,agent,scope,text,request_id,revision,save,worker,signal,signal_id,worker_update}))}));
 app.post('/api/bridge/agents/:id/result',bridge,(req,res)=>{
  const request=pending.get(req.params.id);
  if(request){
   clearTimeout(request.timer);pending.delete(request.id);
   if(!['configuration','signal'].includes(request.action)&&!visible().has(request.project))request.res.status(404).json({error:'This project is no longer available'});
   else{
    const result=req.body;
    if(result.ok&&Array.isArray(result.messages))for(const message of result.messages){delete message.html;if(message.role==='assistant')message.html=renderToStaticMarkup(React.createElement(Markdown,{remarkPlugins:[remarkGfm]},String(message.text??'')));}
    request.res.status(result.ok===false?503:200).json(result);
   }
  }
  res.json({ok:true});
 });
 return ()=>{for(const p of pending.values()){clearTimeout(p.timer);p.res.status(503).json({error:'Service reconnecting. Try again.'});}pending.clear();};
}
