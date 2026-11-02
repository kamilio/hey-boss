// Isolated project-worker interactions; no real fleet is modified.
import {createServer} from 'node:http';
import {readFileSync} from 'node:fs';
import {resolve} from 'node:path';
import assert from 'node:assert/strict';
const {chromium}=await import(process.env.PLAYWRIGHT_MODULE||'playwright');
const projects=['ashby-mcp','hey-gh','hey-proxy','poe-code','ans'].map(name=>({name,id:'github.com/acme/'+name}));
const shared={id:'private-worker-123456',managed:true,pid:12,intent:'running',active:0,runs:[],config:{name:'Worker 123',enabled:true,projects:projects.slice(0,3).map(p=>p.id),concurrency:3,provider:'claude',tags:['ready']}};
const paused={id:'private-worker-654321',managed:true,pid:null,intent:'pause',active:0,runs:[],config:{name:'Worker 456',enabled:false,projects:[projects[3].id],concurrency:2}};
const machine={host:'local',hostname:'MacBook Pro',state:'connected',workspace:'~',workers:[shared,paused,...Array.from({length:28},(_,i)=>({...paused,id:'old-'+i,intent:'stop',config:{...paused.config,name:'Worker '+i}}))],projects:Object.fromEntries(projects.map(p=>[p.id,{git:'https://'+p.id+'.git',path:'~/'+p.name}]))};
let revision=1,failSave=false;
const writes=[];
const source=resolve('src/issues/web');
const server=createServer(async(req,res)=>{
 const path=new URL(req.url,'http://fixture').pathname;
 const json=(value,status=200)=>{res.writeHead(status,{'Content-Type':'application/json'});res.end(JSON.stringify(value));};
 if(path==='/api/bootstrap')return json({ok:true,csrf:'fixture',projects,project:projects[0]});
 if(path==='/api/fleet/status')return json({ok:true,machines:[{...machine,heartbeat:Date.now()/1000}],signals:[]});
 if(path==='/api/fleet/events'){res.writeHead(200,{'Content-Type':'text/event-stream'});res.write(': connected\n\n');return;}
 if(path==='/api/fleet/configuration'||path==='/api/fleet'){
  if(req.method==='POST'){
   let raw='';for await(const chunk of req)raw+=chunk;const body=JSON.parse(raw);writes.push(body);
   if(failSave)return json({ok:false,error:'Configuration changed. Reload before saving.'},409);
   if(body.kind==='signal'){const w=machine.workers.find(w=>w.id===body.worker);w.intent=body.signal==='resume'?'running':'pause';w.config.enabled=w.intent==='running';w.pid=12;return json({ok:true});}
   if(body.worker_update){const u=body.worker_update,w=machine.workers.find(w=>w.id===u.id);Object.assign(w.config,u.config);w.intent=u.intent;}
   const u=body.machine_update;
   if(u){
    const id=u.git?.replace(/^https:\/\//,'').replace(/\.git$/,'')||u.project;
    if(u.action==='project'||u.action==='add'&&u.git){machine.projects[id]={git:u.git,path:u.path||'~/'+id.split('/').pop()};machine.workspace=u.workspace;}
    if(u.action==='project'&&u.worker){const w=machine.workers.find(w=>w.id===u.worker);if(!w.config.projects.includes(id))w.config.projects.push(id);}
    if(u.action==='add')machine.workers.push({id:u.id,managed:true,pid:12,intent:'running',active:0,runs:[],config:{enabled:true,projects:[id],concurrency:u.concurrency}});
    if(u.action==='remove'){const w=machine.workers.find(w=>w.id===u.id);w.retiring=true;w.intent='drain';w.config.enabled=false;}
   }
   revision++;
  }
  return json({ok:true,revision:String(revision),text:'machines: {}',document:{machines:{local:machine}}});
 }
 try{
  const name=path==='/workers'?'fleet.html':path.split('/').pop();if(!/^[\w.-]+$/.test(name))throw Error();
  let content=readFileSync(resolve(source,name));
  if(name==='fleet.html')content=content.toString().replace('<!--app-shell-->',readFileSync(resolve(source,'app-shell.html'),'utf8').replace(/<!--[^]*?-->/g,''));
  if(name==='routes.js')content=content.toString().replace('/* ROUTE_DEFINITIONS */ []',readFileSync(resolve(source,'routes.json'),'utf8'));
  res.writeHead(200,{'Content-Type':name.endsWith('.js')?'text/javascript':name.endsWith('.css')?'text/css':'text/html'});res.end(content);
 }catch{res.writeHead(404);res.end();}
});
await new Promise(done=>server.listen(0,'127.0.0.1',done));
let browser;
try{
 browser=await chromium.launch({channel:'chrome',headless:true});const page=await browser.newPage({viewport:{width:1440,height:1000}}),errors=[];
 page.setDefaultTimeout(5000);page.on('pageerror',e=>errors.push(e.message));
 await page.goto(`http://127.0.0.1:${server.address().port}/workers`);
 await page.getByRole('heading',{name:'Project workers',exact:true}).waitFor();
 const card=id=>page.locator('.project-worker[data-worker="'+id+'"]');
 await card(shared.id).waitFor();assert.equal(await page.locator('.project-worker').count(),2);
 assert.equal(await page.locator('.worker-scope-group').count(),0,'There is no extra group layer');
 assert.doesNotMatch(await page.locator('#worker-board').innerText(),/Worker \d|private-worker|28 paused|stopped workers/);
 assert(await card(paused.id).getByRole('button',{name:'Resume',exact:true}).isVisible());
 if(process.env.PROJECT_WORKER_SCREENSHOTS)await page.screenshot({path:resolve(process.env.PROJECT_WORKER_SCREENSHOTS,'initial.png'),fullPage:true});
 await card(shared.id).getByRole('button',{name:'Add project',exact:true}).click();
 assert.equal(await page.locator('#machine-worker-field').isVisible(),false,'Adding a project keeps the clicked project worker as the target');
 await page.locator('#machine-project').selectOption(projects[4].id);
 await page.getByRole('button',{name:'Add project',exact:true}).last().click();await page.locator('#machine-editor').waitFor({state:'hidden'});
 await page.waitForFunction(()=>document.querySelector('.project-worker').textContent.includes('ans'));
 assert.equal(writes.at(-1).machine_update.worker,shared.id);assert.equal(shared.config.projects.length,4);assert.equal(shared.config.concurrency,3);assert.equal(shared.config.provider,'claude');
 await card(paused.id).getByRole('button',{name:'Resume',exact:true}).click();await card(paused.id).getByRole('button',{name:'Pause',exact:true}).waitFor();
 assert.equal(writes.at(-1).worker,paused.id);
 await card(shared.id).getByRole('button',{name:/Increase agent limit/}).click();await page.waitForFunction(()=>document.querySelector('.project-worker .worker-count').textContent==='4');
 assert.equal(shared.config.concurrency,4);
 await page.getByRole('button',{name:'Add project worker',exact:true}).click();await page.locator('#machine-project').selectOption(projects[3].id);await page.locator('#machine-slots').fill('5');
 await page.locator('#machine-editor-save').click();await page.locator('#machine-editor').waitFor({state:'hidden'});await page.waitForFunction(()=>document.querySelectorAll('.project-worker').length===3);
 assert.equal(writes.at(-1).machine_update.action,'add');assert.equal(writes.at(-1).machine_update.concurrency,5);
 // Reproduce a refresh between pointer-down and pointer-up: it must not replace the button.
 const remove=card(shared.id).getByRole('button',{name:'Remove',exact:true});const handle=await remove.elementHandle();await remove.hover();await page.mouse.down();
 await page.evaluate(()=>document.querySelector('#refresh').click());await page.waitForTimeout(600);
 assert(await handle.evaluate(e=>e.isConnected),'Refresh preserves the actual control under the pointer');await page.mouse.up();
 await page.locator('#machine-editor').waitFor({state:'visible'});assert.equal(await page.locator('#machine-worker-field').isVisible(),false);
 failSave=true;await page.locator('#machine-editor-save').click();await page.getByText('Configuration changed. Reload before saving.',{exact:true}).waitFor();assert(await card(shared.id).isVisible());
 failSave=false;await page.locator('#machine-editor-save').click();await page.locator('#machine-editor').waitFor({state:'hidden'});await card(shared.id).waitFor({state:'detached'});
 assert.equal(writes.at(-1).machine_update.id,shared.id,'Removal targets the clicked project worker');assert(await card(paused.id).isVisible());
 const created=machine.workers.at(-1);created.active=1;await page.locator('#refresh').click();
 await card(created.id).getByRole('button',{name:'Remove',exact:true}).click();await page.locator('#machine-editor-save').click();await page.locator('#machine-editor').waitFor({state:'hidden'});
 await card(created.id).getByText('Removing · finishing agents',{exact:true}).waitFor();
 assert(await card(created.id).getByRole('button',{name:'Stop now',exact:true}).isVisible(),'Busy removal keeps agents visible until they finish');
 created.active=0;await page.locator('#refresh').click();await card(created.id).waitFor({state:'detached'});
 machine.state='disconnected';await page.locator('#refresh').click();await page.locator('.machine-connection').filter({hasText:'Offline'}).waitFor();await page.getByRole('button',{name:'Add project worker',exact:true}).click();
 await page.getByText('This machine is offline. Changes apply when it reconnects.',{exact:true}).waitFor();await page.getByRole('button',{name:'Cancel',exact:true}).click();
 machine.state='connected';await page.locator('#refresh').click();
 for(const colorScheme of ['light','dark'])for(const width of [1440,768,390,320]){
  await page.emulateMedia({colorScheme});await page.setViewportSize({width,height:950});
  assert(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),`${colorScheme} ${width}px fits`);
  if(process.env.PROJECT_WORKER_SCREENSHOTS)await page.screenshot({path:resolve(process.env.PROJECT_WORKER_SCREENSHOTS,`${colorScheme}-${width}.png`),fullPage:true});
 }
 assert.deepEqual(errors,[]);console.log('PASS project workers: flat hierarchy, hidden history and IDs, add project, create, resume, agent limit, stable refresh, failed removal and successful removal, mobile and themes');
}finally{await browser?.close();server.closeAllConnections();await new Promise(done=>server.close(done));}
