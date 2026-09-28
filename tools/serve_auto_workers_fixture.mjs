// Isolated browser fixture: no fleet processes or real configuration are touched.
import {createServer} from 'node:http';
import {readFileSync} from 'node:fs';
const source=new URL('../src/issues/web/',import.meta.url);
let revision='1',text='machines:\n  local:\n    workers:\n      - id: poe-code\n        intent: running\n        config:\n          concurrency: 2\n          projects: [github.com/poe-platform/poe-code]\n          directory: /Users/kamil/Workspace/poe-code\n  devbox:\n    workers: []\n';
const config={name:'Poe Code',concurrency:2,enabled:true,projects:['github.com/poe-platform/poe-code'],directory:'/Users/kamil/Workspace/poe-code'};
const run={id:'run-1',project_id:config.projects[0],project_name:'Poe Code',number:688,title:'Keep workers synchronized',state:'running',started_at:Date.now()-240000,finished_at:null,last_event:'Checking recovery after a disconnected machine returns.'};
const snapshot=()=>({ok:true,configuration:{revision,source:'~/.hey-boss/fleet.yaml',error:null},machines:[
 {host:'local',hostname:'MacBook Pro',state:'connected',heartbeat:Date.now()/1000,desired_revision:revision,applied_revision:revision,workers:[{id:'poe-code',managed:true,intent:'running',pid:12,config,active:1,free:1,eligible:2,runs:[run],chiefs:[]},{id:'tools',managed:true,intent:'running',pid:13,config:{...config,name:'Tools',concurrency:1},active:0,free:1,eligible:0,runs:[],chiefs:[]},{id:'independent',managed:false,pid:14,config,runs:[]}]},
 {host:'devbox',hostname:'Devbox',state:'disconnected',heartbeat:Date.now()/1000-180,desired_revision:revision,applied_revision:'old',workers:[{id:'remote',managed:true,intent:'running',pid:42,config:{...config,name:'Remote worker',directory:'/home/kamil/Workspace/poe-code'},runs:[],chiefs:[]}]},
 {host:'studio',hostname:'Studio Mac',state:'connected',heartbeat:Date.now()/1000,desired_revision:revision,applied_revision:revision,workers:[{id:'paused',managed:true,intent:'pause',pid:43,config:{...config,name:'Review queue',enabled:false},runs:[],chiefs:[]}]}
],signals:[],conflicts:[]});
const server=createServer(async(req,res)=>{
 const path=new URL(req.url,'http://localhost').pathname;const json=(value,status=200)=>{res.writeHead(status,{'Content-Type':'application/json'});res.end(JSON.stringify(value));};
 if(path==='/api/bootstrap'||path==='/api/agent-bootstrap')return json({csrf:'fixture',project:{id:config.projects[0],name:'Poe Code'},projects:[{id:config.projects[0],name:'Poe Code'}]});
 if(path==='/api/fleet/status')return json(snapshot());
 if(path==='/api/fleet/events'){res.writeHead(200,{'Content-Type':'text/event-stream'});res.write('event: connected\ndata: {}\n\n');return;}
 if(path==='/fixture/change'){revision=String(Number(revision)+1);return json({ok:true});}
 if(path==='/api/fleet/configuration'){
  if(req.method==='POST'){
   let body='';for await(const chunk of req)body+=chunk;
   const input=JSON.parse(body);
   if(input.revision!==revision)return json({ok:false,error:'Configuration changed since you opened it. Reload before saving.'},409);
   if(input.text.includes('concurrency: 0'))return json({ok:false,error:'Worker concurrency must be between 1 and 1024'},400);
   if(!input.save)return json({ok:true,valid:true,revision,changes:input.text===text?[]:[{host:'local',worker:'poe-code',action:'update'}]});
   text=input.text;revision=String(Number(revision)+1);
  }
  return json({ok:true,text,revision,source:'~/.hey-boss/fleet.yaml',error:null});
 }
 if(path==='/api/fleet')return json({ok:true});
 if(path.startsWith('/api/'))return json({ok:true});
 const file=['/agents','/workers'].includes(path)?'fleet.html':path.slice(1);
 if(!['fleet.html','fleet.js','fleet.css','components.js','components.css','quick-issue.js','routes.js','origin.css','icon.png'].includes(file)){res.writeHead(404);return res.end();}
 let content=readFileSync(new URL(file,source));
 if(file==='fleet.html')content=Buffer.from(content.toString().replace('<!--app-shell-->',readFileSync(new URL('app-shell.html',source),'utf8')));
 if(file==='routes.js')content=Buffer.from(content.toString().replace('/* ROUTE_DEFINITIONS */ []',readFileSync(new URL('routes.json',source),'utf8')));
 res.writeHead(200,{'Content-Type':file.endsWith('.js')?'text/javascript':file.endsWith('.css')?'text/css':file.endsWith('.png')?'image/png':'text/html'});res.end(content);
});
server.listen(59688,'127.0.0.1',()=>console.log('Worker browser fixture: http://127.0.0.1:59688/agents#workers'));
for(const signal of ['SIGTERM','SIGINT'])process.on(signal,()=>{server.closeAllConnections();server.close();});
