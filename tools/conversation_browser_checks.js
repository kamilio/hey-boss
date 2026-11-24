// Isolated local fixture: real assets and browser audio, synthetic chat API.
const fs=require('fs'),http=require('http'),path=require('path'),vm=require('vm');
const {webkit,chromium}=require(process.env.PLAYWRIGHT_CORE || 'playwright-core');
const root=path.resolve(__dirname,'..');
const assets={'/':'src/conversation_web/index.html','/components.css':'src/issues/web/components.css','/brand.png':'src/issues/web/icon.png'};
const fixture={};
const server=http.createServer((req,res)=>{
 if(fixture.offline){req.socket.destroy();return;}
 if(req.url==='/sw.js' && fixture.failWorker){res.writeHead(503);return res.end();}
 if(req.url==='/pair' && fixture.pairing){
  let body='';req.on('data',b=>body+=b);req.on('end',()=>{
   fixture.paired=body==='test-pairing-code';res.writeHead(fixture.paired?200:403);res.end();
  });return;
 }
 if(req.url==='/api' && fixture.pairing && !fixture.paired){res.writeHead(403);return res.end();}
 if(req.url==='/api' && fixture.messages){
  let body='';req.on('data',b=>body+=b);req.on('end',()=>{
   const p=JSON.parse(body),r={status:'ok'},chat=fixture.chat;
   if(p.action==='list'){
    if(fixture.failList){res.writeHead(503);res.end();return;}
    const chats=fixture.chats || [...fixture.messages.keys()].map(id=>({id,title:'Saved conversation'}));
    const start=p.after ? chats.findIndex(c=>c.id===p.after)+1 : 0;
    r.chats=chats.slice(start,start+30);r.next=start+30<chats.length?r.chats.at(-1).id:null;
   }
   if(p.action==='state' && fixture.blockState){res.writeHead(503);res.end();return;}
   if(p.action==='state')Object.assign(r,{chat,queuedSend:true,serverQueue:!!fixture.serverQueue,queue:fixture.queue || [],queuePaused:!!fixture.queuePaused,canCancel:!!fixture.busy,canSend:fixture.serverQueue || !fixture.busy,voice:{phase:fixture.busy?'thinking':'idle',detail:'Voice off'},start:0,end:fixture.messages.get(chat).length,revision:fixture.revision,messages:fixture.messages.get(chat),tools:[],visuals:[],heading:'Start a conversation'});
   if(p.action==='cancel'){fixture.busy=false;fixture.queuePaused=true;}
   if(p.action==='cancel_queued')fixture.queue=fixture.queue.filter(item=>item.id!==p.message);
   if(p.action==='resume_queue'){fixture.queuePaused=false;const next=fixture.queue.shift();if(next){fixture.messages.get(chat).push({speaker:'You',text:next.text});fixture.busy=true;fixture.revision++;}}
   if(p.action==='send'){
    fixture.attempts++;
    if(!fixture.accepted.has(p.message)){
     if(p.chat!==chat||(!fixture.serverQueue&&fixture.busy)){res.end(JSON.stringify({status:'error',error:'Wrong chat or busy'}));return;}
     fixture.accepted.set(p.message,p.text);
     if(fixture.serverQueue&&fixture.busy)fixture.queue.push({id:p.message,text:p.text});
     else fixture.messages.get(chat).push({speaker:'You',text:p.text});fixture.revision++;
    }
    r.accepted=p.message;
    if(fixture.loseReply){fixture.loseReply=false;fixture.offline=true;req.socket.destroy();return;}
   }
   if(p.action==='open'){fixture.chat=p.chat;fixture.revision++;}
   if(p.action==='new'){
    fixture.chat=require('node:crypto').randomUUID();fixture.messages.set(fixture.chat,[]);fixture.revision++;
    fixture.chats?.unshift({id:fixture.chat,title:'New chat'});
   }
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
   for(const file of ['conversation_safari_checks.js','conversation_recovery_checks.js','conversation_offline_checks.js','conversation_pairing_checks.js','conversation_sidebar_checks.js','conversation_queue_checks.js']) {
    const browser=await engine.launch(name==='chrome'?{channel:'chrome',headless:true}:{headless:true});
    try {
     for(const key of Object.keys(fixture))delete fixture[key];fixture.browser=name;
     if(file.includes('pairing')) Object.assign(fixture,{pairing:true,failWorker:true});
     const context=await browser.newContext({viewport:{width:390,height:844},serviceWorkers:/offline|pairing|queue/.test(file)?'allow':'block'});
     const page=await context.newPage();await page.goto(origin);
     console.log(name+': '+await vm.runInThisContext(fs.readFileSync(path.join(root,'tools',file),'utf8'))(page,fixture));
    } finally {await browser.close();}
   }
  }
 } finally {server.close();}
})().catch(e=>{console.error(e);process.exitCode=1;server.close();});
