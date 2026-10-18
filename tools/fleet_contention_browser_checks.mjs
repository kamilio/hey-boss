// Synthetic read-only fleet states; Chrome and the local server close on failure too.
import {createServer} from 'node:http';
import {readFileSync, mkdirSync} from 'node:fs';
import {resolve} from 'node:path';
import assert from 'node:assert/strict';
const {chromium} = await import(process.env.PLAYWRIGHT_MODULE || 'playwright');
const source = resolve('src/issues/web');
const project = {id:'named:Contention QA',name:'Contention QA'};
const runs = Array.from({length:2},(_,i)=>({id:'run-'+i,project_id:project.id,project_name:project.name,number:i+1,title:['Keep the current task running','Apply changes after the database is available'][i],state:'running',started_at:Date.now()-120000,finished_at:null,last_event:'Working through the existing task.'}));
let state = 'healthy';
const fleet = () => ({ok:true,machines:[{host:'remote',hostname:'Connected MacBook',role:'agent',state:state==='offline'?'disconnected':'connected',heartbeat:Date.now()/1000,desired_revision:'current',applied_revision:'current',sync_observed_at:Date.now()/1000,sync:{state:'healthy',message:'Changes are applied and sync is responding'},database_errors:state==='waiting'?{pull:'Database writer queue timed out before execution'}:{},workers:[{id:'preserved-worker',pid:123,intent:'running',managed:true,active:2,config:{name:'Productive worker',enabled:true,concurrency:3,projects:[project.id]},runs}]}],signals:[],conflicts:[]});
const mutations=[];
const server=createServer((req,res)=>{
  const url=new URL(req.url,'http://fixture'),path=url.pathname;
  const json=value=>{res.setHeader('Content-Type','application/json');res.end(JSON.stringify(value));};
  if(req.method!=='GET'){mutations.push(path);res.writeHead(405);return json({ok:false});}
  if(['/api/bootstrap','/api/agent-bootstrap'].includes(path))return json({ok:true,csrf:'fixture',project,projects:[project]});
  if(path==='/api/fleet/status')return json(fleet());
  if(path==='/api/fleet/events'){
    res.writeHead(200,{'Content-Type':'text/event-stream','Cache-Control':'no-cache'});
    res.write('event: connected\ndata: {}\n\n');return;
  }
  if(path==='/api/action')return json({ok:true,pull_requests:[]});
  try{
    const paired=url.searchParams.has('paired');
    const assets=paired||path.startsWith('/agent-web/')?'mobile/public/agent-web':path.startsWith('/artifact-web/')?'mobile/public/artifact-web':source;
    const name=path==='/workers'?'fleet.html':path.split('/').pop();
    if(!/^[\w.-]+$/.test(name))throw Error('Not found');
    let body=readFileSync(resolve(assets,name));
    if(name==='fleet.html'&&!paired)body=body.toString().replace('<!--app-shell-->',readFileSync(resolve(source,'app-shell.html'),'utf8').replace(/<!--[^]*?-->/g,''));
    if(name==='routes.js')body=body.toString().replace('/* ROUTE_DEFINITIONS */ []',readFileSync(resolve(source,'routes.json'),'utf8'));
    res.setHeader('Content-Type',name.endsWith('.css')?'text/css':name.endsWith('.js')?'text/javascript':name.endsWith('.png')?'image/png':'text/html');res.end(body);
  }catch{res.writeHead(404);res.end();}
});
await new Promise(done=>server.listen(0,'127.0.0.1',done));
let browser;
let checks=0;
try{
  browser=await chromium.launch({channel:'chrome',headless:true});
  const page=await browser.newPage({hasTouch:true}),errors=[];
  page.setDefaultTimeout(10000);page.on('pageerror',error=>errors.push(error.message));
  for(const paired of [false,true])for(const colorScheme of ['light','dark'])for(const width of [1440,768,390,320]){
    await page.setViewportSize({width,height:900});await page.emulateMedia({colorScheme});
    await page.goto(`http://127.0.0.1:${server.address().port}/workers${paired?'?paired=1':''}`);
    await page.locator('.activity-machine').waitFor();
    for(const next of ['healthy','waiting','recovered','offline']){
      state=next;await page.locator('#refresh').click();
      await page.waitForFunction(expected=>{
        const message=document.querySelector('#fleet-attention').textContent;
        return expected==='waiting'?message.includes('Database busy'):expected==='offline'?message.includes('offline; activity is last known'):!message.trim();
      },state);
      assert(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),'Layout fits '+width);
      assert.equal(await page.locator('.activity-machine').count(),1);
      assert.equal(await page.locator('.activity-machine .machine-connection').innerText(),state==='offline'?'Offline':'Connected');
      assert.match(await page.locator('.activity-machine-usage').innerText(),state==='offline'?/Last known activity/:/2 agents running · 1 available/);
      const scope=page.locator('.worker-scope-group');
      if(!await scope.evaluate(el=>el.open))await scope.locator('summary').first().click();
      const worker=page.locator('.activity-worker');
      if(await worker.count()&&!await worker.evaluate(el=>el.open))await worker.locator('summary').first().click();
      assert.equal(await page.locator('.activity-task').count(),2,'Both current tasks are retained');
      if(state==='waiting')assert.match(await page.locator('#fleet-attention').innerText(),/Database busy; data sync will retry/);
      if(process.env.FLEET_SCREENSHOTS&&[1440,390,320].includes(width)){
        mkdirSync(process.env.FLEET_SCREENSHOTS,{recursive:true});
        await page.screenshot({path:resolve(process.env.FLEET_SCREENSHOTS,`${paired?'paired':'desktop'}-${colorScheme}-${width}-${state}.png`),fullPage:true});
      }
      checks++;
    }
    await page.locator('#refresh').focus();await page.keyboard.press('Enter');
    assert(await page.locator('#refresh').evaluate(el=>el===document.activeElement),'Refresh retains keyboard focus');
  }
  assert.deepEqual(mutations,[],'Visual tests never send worker controls');assert.deepEqual(errors,[]);
  console.log(`PASS: ${checks} desktop/paired light/dark layouts, waiting/recovery/offline states, preserved tasks, keyboard focus, no mutations or runtime errors`);
}finally{await browser?.close();await new Promise(done=>server.close(done));}
