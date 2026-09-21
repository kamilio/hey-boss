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
  const projects=visible();return {...snapshot,signals:[],conflicts:[],events:[],machines:(snapshot?.machines||[]).map(m=>({host:m.host,hostname:m.hostname,state:m.state,heartbeat:m.heartbeat,workers:(m.workers||[]).map(w=>({id:w.id,pid:w.pid,runs:(w.runs||[]).filter(r=>projects.has(r.project_id))}))}))};
 };
 const page=fileURLToPath(new URL('../dist/agent-web/fleet.html',import.meta.url));
 for(const route of ['/agents','/agents/session'])app.get(route,auth,(req,res)=>res.sendFile(page));
 app.get('/api/agent-bootstrap',auth,(req,res)=>res.json({projects:store.issueProjects()}));
 app.get('/api/fleet/status',auth,(req,res)=>{if(!snapshot)return res.status(503).json({error:'Connect your supervisor to see agents.'});const value=filtered();if(now()-seen>15000)for(const m of value.machines)m.state='disconnected';res.json(value);});
 function requestAgent(req,res,action){
  const input=action==='takeover'?req.body:req.query;
  const cursor=Number(input.cursor??0),before=input.before==null?null:Number(input.before),at=input.at==null?null:Number(input.at),latest=input.latest==='1',host=input.host,run=input.run;
  if((before!==null&&(!Number.isSafeInteger(before)||before<0))||(req.query.latest!=null&&!['0','1'].includes(req.query.latest))||!Number.isSafeInteger(cursor)||cursor<0||typeof host!=='string'||typeof run!=='string')throw new HubError(400,'Invalid conversation request');
  if(at!==null&&(!Number.isSafeInteger(at)||at<0))throw new HubError(400,'Invalid invocation cursor');
  const entry=filtered().machines.find(m=>m.host===host||m.hostname===host)?.workers.flatMap(w=>w.runs).find(r=>r.id===run);
  // Historical origin references are validated by the authoritative supervisor.
  const project=entry?.project_id||(action==='conversation'&&visible().has(input.project)?input.project:null);
  if(!project)throw new HubError(404,'This conversation is no longer available');
  if(now()-seen>15000)throw new HubError(503,'Connect your supervisor to load this conversation.');
  if(pending.size>=32||[...pending.values()].filter(p=>p.device===req.device.id).length>=2)throw new HubError(429,'Wait for your current conversation to load.');
  const id=randomUUID();const timer=setTimeout(()=>{pending.delete(id);if(!res.destroyed)res.status(504).json({error:'The device did not respond. Try again when it reconnects.'});},20000);timer.unref();
  pending.set(id,{id,action,host,run,cursor,before,latest,at,project,device:req.device.id,res,timer});
  res.on('close',()=>{clearTimeout(timer);pending.delete(id);});
 }
 app.get('/api/fleet/conversation',auth,(req,res)=>requestAgent(req,res,'conversation'));
 app.post('/api/fleet/takeover',auth,(req,res)=>requestAgent(req,res,'takeover'));
 app.post('/api/bridge/agents/status',bridge,(req,res)=>{
  if(!Array.isArray(req.body.machines)||req.body.machines.length>100)throw new HubError(400,'Invalid agent snapshot');
  snapshot=req.body;seen=now();res.json({ok:true});
 });
 app.get('/api/bridge/agents',bridge,(req,res)=>res.json({requests:[...pending.values()].map(({id,action,host,run,cursor,before,latest,at,project})=>({id,action,host,run,cursor,before,latest,at,project}))}));
 app.post('/api/bridge/agents/:id/result',bridge,(req,res)=>{
  const request=pending.get(req.params.id);
  if(request){
   clearTimeout(request.timer);pending.delete(request.id);
   if(!visible().has(request.project))request.res.status(404).json({error:'This project is no longer available'});
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
