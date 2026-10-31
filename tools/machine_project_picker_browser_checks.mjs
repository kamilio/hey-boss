// Project selection through the real dialog; fleet writes are captured in an isolated fixture.
import {createServer} from 'node:http';
import {readFileSync} from 'node:fs';
import {resolve} from 'node:path';
import assert from 'node:assert/strict';
const {chromium}=await import(process.env.PLAYWRIGHT_MODULE||'playwright');
const source=resolve('src/issues/web');
const atlas='github.com/acme/atlas',tools='github.com/acme/tools';
const projects=[{id:atlas,name:'Atlas'},{id:tools,name:'Tools'},{id:'named:Notes',name:'Notes'},{id:'github.com/acme/hidden',name:'Hidden',hidden_at:1}];
const machine={host:'local',hostname:'Devbox',state:'connected',workspace:'~/Work',projects:{},workers:[]};
const machines={local:machine,remote:{projects:{[atlas]:{git:'git@github.com:acme/atlas.git',path:'/remote/atlas'},'github.com/acme/configured':{git:'https://github.com/acme/configured.git',path:'/remote/configured'},'github.com/acme/hidden':{git:'https://github.com/acme/hidden.git',path:'/remote/hidden'}}}};
const writes=[];
let revision=1;
const server=createServer(async(req,res)=>{
  const path=new URL(req.url,'http://fixture').pathname;
  const json=value=>{res.setHeader('Content-Type','application/json');res.end(JSON.stringify(value));};
  if(path==='/api/bootstrap')return json({ok:true,csrf:'fixture',project:projects[0],projects});
  if(path==='/api/fleet/status')return json({ok:true,machines:[{...machine,heartbeat:Date.now()/1000}],signals:[]});
  if(path==='/api/fleet/configuration'){
    if(req.method==='POST'){
      let body='';for await(const chunk of req)body+=chunk;
      const input=JSON.parse(body);writes.push(input);
      if(input.machine_update){
        const u=input.machine_update,id=u.git.replace(/^git@github.com:/,'github.com/').replace(/^https:\/\//,'').replace(/\.git$/,'');
        machine.projects[id]={git:u.git,path:u.path||u.workspace+'/'+id.split('/').pop(),reuse_existing:!u.path};revision++;
      }
    }
    return json({ok:true,revision:String(revision),text:'machines: {}',document:{machines}});
  }
  if(path==='/api/fleet/events'){res.writeHead(200,{'Content-Type':'text/event-stream'});res.write(': connected\n\n');return;}
  try{
    const name=path==='/workers'?'fleet.html':path.split('/').pop();
    if(!/^[\w.-]+$/.test(name))throw Error('Not found');
    let body=readFileSync(resolve(source,name));
    if(name==='fleet.html')body=body.toString().replace('<!--app-shell-->',readFileSync(resolve(source,'app-shell.html'),'utf8').replace(/<!--[^]*?-->/g,''));
    if(name==='routes.js')body=body.toString().replace('/* ROUTE_DEFINITIONS */ []',readFileSync(resolve(source,'routes.json'),'utf8'));
    res.setHeader('Content-Type',name.endsWith('.css')?'text/css':name.endsWith('.js')?'text/javascript':name.endsWith('.png')?'image/png':'text/html');res.end(body);
  }catch{res.writeHead(404);res.end();}
});
await new Promise(done=>server.listen(0,'127.0.0.1',done));
let browser;
try{
  browser=await chromium.launch({channel:'chrome',headless:true});
  const page=await browser.newPage({viewport:{width:1440,height:1000}}),errors=[];
  page.setDefaultTimeout(5000);page.on('pageerror',e=>errors.push(e.message));
  await page.goto(`http://127.0.0.1:${server.address().port}/workers`);
  const open=()=>page.getByRole('button',{name:'+ Add project',exact:true}).click();
  await open();
  const picker=page.getByRole('combobox',{name:'Project',exact:true}),git=page.getByRole('textbox',{name:'Git repository',exact:true});
  await picker.waitFor();
  assert.equal(await picker.locator('option[value="'+atlas+'"]').count(),1,'Registry and configuration projects are deduplicated');
  assert.equal(await picker.locator('option[value="github.com/acme/configured"]').count(),1,'Configured projects are available before registry enrollment');
  assert.equal(await picker.locator('option[value="github.com/acme/hidden"]').count(),0,'Hidden projects stay hidden');
  assert.equal(await picker.locator('option[value="named:Notes"]').evaluate(option=>option.disabled),true,'Projects without a Git identity cannot select a different project accidentally');
  await picker.selectOption(atlas);
  assert.equal(await git.inputValue(),'git@github.com:acme/atlas.git','Selection preserves the configured Git transport');
  assert.equal(await page.locator('#machine-path').inputValue(),'','Remote checkout paths are not copied');
  assert.equal(await page.locator('#machine-path').getAttribute('placeholder'),'Automatic · ~/Work/atlas');
  await picker.selectOption(tools);
  assert.equal(await git.inputValue(),'https://github.com/acme/tools.git','Repository identities suggest HTTPS when no saved URL exists');
  await picker.selectOption(atlas);
  for(const colorScheme of ['light','dark'])for(const width of [1440,390,320]){
    await page.emulateMedia({colorScheme});await page.setViewportSize({width,height:900});
    assert(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth&&document.querySelector('dialog[open]').getBoundingClientRect().right<=innerWidth),`${colorScheme} ${width}px fits`);
    if(process.env.PROJECT_PICKER_SCREENSHOTS)await page.screenshot({path:resolve(process.env.PROJECT_PICKER_SCREENSHOTS,`${colorScheme}-${width}.png`)});
  }
  await page.getByRole('button',{name:'Save project',exact:true}).click();
  await page.locator('#machine-editor').waitFor({state:'hidden'});
  assert.deepEqual(writes.at(-1).machine_update,{host:'local',action:'project',git:'git@github.com:acme/atlas.git',workspace:'~/Work',path:'',worker:''});
  await open();await picker.selectOption(tools);
  await git.fill('https://github.com/acme/manual.git');
  assert.equal(await picker.inputValue(),'','Typing another repository clears the previous selection');
  await page.getByRole('button',{name:'Save project',exact:true}).click();await page.locator('#machine-editor').waitFor({state:'hidden'});
  assert.equal(writes.at(-1).machine_update.git,'https://github.com/acme/manual.git');
  await open();await picker.selectOption(atlas);await picker.selectOption('');
  assert.equal(await git.inputValue(),'','New repository starts with a blank Git field');
  await page.getByRole('button',{name:'Cancel',exact:true}).click();
  machine.configuration_error=atlas+': Git clone failed';
  await page.locator('#refresh').click();
  await page.locator('[data-machine-action="edit-project"][data-project="'+atlas+'"]').first().click();
  assert.equal(await picker.isVisible(),false,'Editing a checkout keeps its existing identity');
  assert.equal(await git.inputValue(),'git@github.com:acme/atlas.git');
  assert.deepEqual(errors,[]);
  console.log('Project picker checks passed: existing and configured projects, Git autofill, hidden and non-Git projects, manual repository, save payload, edit checkout, desktop/mobile and both themes.');
}finally{
  await browser?.close();server.closeAllConnections();await new Promise(done=>server.close(done));
}
