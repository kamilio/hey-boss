// Isolated local fixture: real assets and browser audio, synthetic chat API.
const fs=require('fs'),http=require('http'),path=require('path'),vm=require('vm');
const {webkit,chromium}=require(process.env.PLAYWRIGHT_CORE || 'playwright-core');
const root=path.resolve(__dirname,'..');
const assets={'/':'src/conversation_web/index.html','/components.css':'src/issues/web/components.css','/brand.png':'src/issues/web/icon.png'};
const fixture={};
const server=http.createServer((req,res)=>{
 if(fixture.offline){req.socket.destroy();return;}
 if(req.url==='/api' && fixture.messages){
  let body='';req.on('data',b=>body+=b);req.on('end',()=>{
   const p=JSON.parse(body),r={status:'ok'},chat=fixture.chat;
   if(p.action==='state' && fixture.blockState){res.writeHead(503);res.end();return;}
   if(p.action==='state')Object.assign(r,{chat,queuedSend:true,canSend:!fixture.busy,voice:{phase:fixture.busy?'thinking':'idle',detail:'Voice off'},start:0,end:fixture.messages.get(chat).length,revision:fixture.revision,messages:fixture.messages.get(chat),tools:[],visuals:[],heading:'Start a conversation'});
   if(p.action==='send'){
    fixture.attempts++;
    if(!fixture.accepted.has(p.message)){
     if(p.chat!==chat||fixture.busy){res.end(JSON.stringify({status:'error',error:'Wrong chat or busy'}));return;}
     fixture.accepted.set(p.message,p.text);fixture.messages.get(chat).push({speaker:'You',text:p.text});fixture.revision++;
    }
    r.accepted=p.message;
    if(fixture.loseReply){fixture.loseReply=false;fixture.offline=true;req.socket.destroy();return;}
   }
   if(p.action==='open'){fixture.chat=p.chat;fixture.revision++;}
   res.setHeader('Content-Type','application/json');res.end(JSON.stringify(r));
  });return;
 }
 const file=assets[req.url]||(req.url.match(/^\/(app.js|app.css|mic.js|sw.js|icon.png|manifest.webmanifest)$/)?'src/conversation_web'+req.url:null);
 if(!file){res.writeHead(404);return res.end();}
 res.setHeader('Content-Type',req.url.endsWith('.js')?'text/javascript':req.url.endsWith('.css')?'text/css':req.url.endsWith('.png')?'image/png':req.url.endsWith('.webmanifest')?'application/manifest+json':'text/html');
 res.setHeader('Cache-Control','no-store');res.setHeader('Content-Security-Policy',"default-src 'self'; img-src 'self' data:; frame-ancestors 'none'; base-uri 'none'; form-action 'self'");
 res.end(fs.readFileSync(path.join(root,file)));
});
(async()=>{
 await new Promise(r=>server.listen(0,'127.0.0.1',r));const origin='http://127.0.0.1:'+server.address().port;
 try {
  for(const [name,engine] of [['webkit',webkit],['chrome',chromium]]) {
   for(const file of ['conversation_safari_checks.js','conversation_recovery_checks.js','conversation_offline_checks.js']) {
    const browser=await engine.launch(name==='chrome'?{channel:'chrome',headless:true}:{headless:true});
    try {
     for(const key of Object.keys(fixture))delete fixture[key];fixture.browser=name;
     const context=await browser.newContext({viewport:{width:390,height:844},serviceWorkers:file.includes('offline')?'allow':'block'});
     const page=await context.newPage();await page.goto(origin);
     console.log(name+': '+await vm.runInThisContext(fs.readFileSync(path.join(root,'tools',file),'utf8'))(page,fixture));
    } finally {await browser.close();}
   }
  }
 } finally {server.close();}
})().catch(e=>{console.error(e);process.exitCode=1;server.close();});
