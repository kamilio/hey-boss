// Real settings markup and controller, isolated from personal state.
import {createServer} from 'node:http';
import {readFileSync} from 'node:fs';
const source=new URL('../src/issues/web/',import.meta.url);
const dialog=readFileSync(new URL('index.html',source),'utf8').match(/<dialog id="global-settings-dialog"[^]*?<\/dialog>/)[0];
const server=createServer((req,res)=>{
 if(/^\/[\w-]+\.(css|js)$/.test(req.url)){
  res.setHeader('Content-Type',req.url.endsWith('.css')?'text/css':'text/javascript');
  try{res.end(readFileSync(new URL(req.url.slice(1),source)));}catch{res.writeHead(404).end();}return;
 }
 res.setHeader('Content-Type','text/html');
 res.end(`<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><link rel="stylesheet" href="/components.css"><link rel="stylesheet" href="/app.css">
 <div class="profile-control"><button id="self-avatar">Profile</button><div id="profile-menu" hidden><span id="profile-name"></span><button id="global-settings-trigger">Settings</button></div></div>${dialog}
 <script>
 const $=s=>document.querySelector(s),model={boss:{name:'Boss'},project:{id:'fixture'},route:{}},detailCache=new Map();
 const closeProjectMenu=()=>{},renderRoute=async()=>{},toast=()=>{},initials=()=> 'B',HeyBossUI={requestId:()=>crypto.randomUUID()};
 window.saved=[];window.failure=null;window.fixture={boss_name:'Boss',auto_close_merged_prs:true,version:1,quiet_hours:{enabled:true,end:'07:00',start:'22:00',time_zone:'America/Chicago'}};
 async function api(operation){if(operation.action==='global_settings')return structuredClone(fixture);if(failure){const error=Error(failure);error.code='conflict';throw error;}saved.push(operation);Object.assign(fixture,operation,{version:fixture.version+1});return structuredClone(fixture);}
 </script><script src="/global-settings.js"></script><script>initGlobalSettings();updateProfile();</script></html>`);
});
server.listen(59651,'127.0.0.1',()=>console.log('Quiet hours fixture http://127.0.0.1:59651'));
