// Run with Playwright WebKit on the Conversations page. All chat API calls
// below stay in the fixture; capture and playback use real Web Audio.
async (page) => {
  const origin = await page.evaluate(()=>location.origin);
  const silence = await page.evaluate(()=>btoa('\0'.repeat(3200)));
  let chat = "10000000-0000-4000-8000-000000000000";
  let active = false, muted = false, recorded = false, heard = false, stall = false;
  let phase = 'listening', paused = false, approval = null, failStartPoll = true, interruptLabel = 'Interrupt reply';
  const frames = [];
  const requests = [];
  await page.route(origin + "/api", async route => {
    const p = route.request().postDataJSON();
    requests.push(p.action);
    const r = {status:"ok"};
    if (p.action === "list") r.chats=[{id:chat,title:"Current chat"}];
    if (p.action === "start") active = true;
    if (active && p.action === "state" && failStartPoll) {
      failStartPoll = false;
      return route.fulfill({status:503,body:''});
    }
    if (p.action === "stop") active = false;
    if (p.action === "mute") muted = !muted;
    if (p.action === "pause") paused = !paused;
    if (p.action === "interrupt") phase = 'recording';
    if (p.action === "finish") phase = 'thinking';
    if (p.action === "allow" || p.action === "decline") {
      if(p.text!==approval?.id) throw Error("Decision applied to a stale approval");
      approval=null;
    }
    if (p.action === "state") Object.assign(r, {
      chat, mine:active, voice:{phase:active ? phase:"idle", message:phase, interrupt:phase==='speaking', approval:!!approval,
        listeningPaused:active && ['listening','recording'].includes(phase) ? muted : undefined},
      heading:"Work with Codex", hint:"Folder conversation", placeholder:"Message Codex…",
      paused, muted, approval, interruptLabel, passage:phase==='speaking'?'A spoken reply':'', spoken:2,
      start:0, end:0, revision:1, messages:[], tools:[], visuals:[]
    });
    if (p.action === "exchange") {
      if (p.pcm) {
        const audio=await page.evaluate(encoded=>{
          const bytes=Uint8Array.from(atob(encoded),c=>c.charCodeAt(0));
          if(bytes.length>16384) throw Error("Microphone packet exceeds native limit");
          const pcm=new Int16Array(bytes.buffer), numbered=[];
          for(let i=0;i<pcm.length;i+=1280) if(pcm[i]===-31337) numbered.push(pcm[i+1]);
          return {numbered,nonzero:pcm.some(x=>x!==0)};
        },p.pcm);
        frames.push(...audio.numbered); recorded ||= audio.nonzero;
      }
      if (stall) { stall=false; await page.waitForTimeout(1200); }
      heard ||= p.played === 1600;
      Object.assign(r,{generation:1,paused:false});
      if (!p.received) Object.assign(r,{rate:16000,offset:0,pcm:silence});
    }
    await route.fulfill({json:r});
  });
  await page.route(origin+"/app.js", async route => {
    const response=await route.fetch();
    // Install the synthetic source in the same WebKit world as the app.
    const mock = () => Object.defineProperty(MediaDevices.prototype,"getUserMedia",{value:async()=>{
      const context=starting.ctx, oscillator=context.createOscillator(), dest=context.createMediaStreamDestination();
      oscillator.connect(dest); oscillator.start();
      window.testMic=dest.stream.getAudioTracks()[0];
      if(window.holdMic) await new Promise(resolve=>window.grantMic=resolve);
      return dest.stream;
    }});
    await route.fulfill({response,body:(await response.text())+"\n("+mock.toString()+")();"});
  });
  await page.setViewportSize({width:390,height:844});
  await page.goto(origin+'/');
  await page.waitForFunction(()=>document.querySelector('#status').textContent==='Ready' && navigator.mediaDevices.getUserMedia.toString().includes('createOscillator'));
  await page.getByRole("heading",{name:"Work with Codex"}).waitFor();
  await page.getByPlaceholder("Message Codex…").waitFor();
  for (const colorScheme of ["light", "dark"]) {
    await page.emulateMedia({colorScheme});
    for (const [width,height] of [[320,568],[390,844],[844,390],[768,1024],[1440,900]]) {
      await page.setViewportSize({width,height});
      const fits = await page.evaluate(() => {
        const root = document.documentElement, dock = document.querySelector("footer").getBoundingClientRect();
        const brand = document.querySelector(".brand span").getBoundingClientRect();
        return root.scrollWidth <= innerWidth && dock.bottom <= innerHeight && brand.width > 0
          && [...document.styleSheets].some(s=>s.href?.endsWith("/components.css") && s.cssRules.length);
      });
      if (!fits) throw Error("Missing Hey Boss theme or clipped controls at "+width+"px in "+colorScheme);
    }
  }
  await page.setViewportSize({width:390,height:844});
  await page.evaluate(()=>{
    window.screenLocks=0;
    Object.defineProperty(navigator,'wakeLock',{value:{request:async type=>{
      if(type!=='screen') throw Error('Unexpected wake lock');
      window.screenLocks++;
      return {release:async()=>{window.screenLocks--;}};
    }}});
  });
  await page.getByRole("button",{name:"Start voice"}).click();
  await page.getByRole("button",{name:"End voice"}).waitFor();
  if(await page.getByRole("heading",{name:"Work with Codex"}).isVisible()) throw Error("An active conversation still offered to start one");
  if(await page.evaluate(()=>window.screenLocks)!==1) throw Error("Foreground voice did not request the screen");
  await page.getByRole("button",{name:"Chats",exact:true}).click();
  await page.getByRole("button",{name:"Current chat"}).click();
  if (requests.includes("open") || requests.includes("stop")) throw Error("Selecting the current chat interrupted voice");
  const deadline=Date.now()+10000;
  while (!(recorded&&heard) && Date.now()<deadline) await page.waitForTimeout(50);
  if (!recorded || !heard) throw Error("Missing microphone frames or playback acknowledgment");
  await page.evaluate(()=>{
    const port=voice.mic.port, receive=port.onmessage;
    window.micPackets=0;
    port.onmessage=e=>{const pcm=new Int16Array(e.data);pcm[0]=-31337;pcm[1]=++window.micPackets;receive(e);};
  });
  stall=true;
  const resumeDeadline=Date.now()+7000;
  while(frames.length<22 && Date.now()<resumeDeadline) await page.waitForTimeout(50);
  if(frames.length<22 || frames.some((n,i)=>n!==i+1)) throw Error("A short connection stall interrupted or lost microphone audio");
  await page.locator("#levels:visible").waitFor();
  await page.getByRole("button",{name:"Pause listening"}).click();
  await page.waitForFunction(()=>!window.testMic.enabled);
  const exchanges=()=>requests.filter(action=>action==='exchange').length, beforePause=exchanges();
  await page.waitForTimeout(200);
  const pausedPackets=await page.evaluate(()=>window.micPackets);
  await page.waitForTimeout(1300);
  const extra=await page.evaluate(()=>window.micPackets)-pausedPackets;
  if(extra) throw Error('Paused microphone still posted '+extra+' buffers in 1.3s');
  const heartbeats=exchanges()-beforePause;
  if(heartbeats<1 || heartbeats>5) throw Error('Unexpected paused microphone exchange count: '+heartbeats);
  await page.getByRole("button",{name:"Resume listening"}).click();
  await page.waitForFunction(()=>window.testMic.enabled);
  await page.waitForFunction(base=>window.micPackets>base, pausedPackets);
  phase='speaking';
  await page.getByRole("button",{name:"Mute microphone",exact:true}).waitFor();
  await page.getByRole("button",{name:"Pause reply"}).click();
  await page.getByRole("button",{name:"Resume reply"}).click();
  if (paused || await page.locator('#caption b').textContent()!=='A ') throw Error("Playback controls or spoken caption lost their place");
  await page.getByRole("button",{name:"Interrupt reply",exact:true}).waitFor();
  interruptLabel='Skip reading';
  await page.getByRole("button",{name:"Skip reading",exact:true}).click();
  await page.getByRole("button",{name:"End voice",exact:true}).waitFor();
  await page.getByRole("button",{name:"Pause listening"}).click();
  await page.getByRole("button",{name:"Resume listening"}).click();
  await page.getByRole("button",{name:"Send voice message"}).click();
  if (!requests.includes("finish") || requests.includes("stop")) throw Error("The recording bubble discarded the unfinished turn");
  for(const [id,label] of [['one','Decline'],['two','Allow once']]) {
    approval={id,text:'Fixture action '+id};
    await page.getByText(approval.text).waitFor();
    await page.getByRole("button",{name:label}).click();
    await page.locator('#approval').waitFor({state:'hidden'});
  }
  await page.getByRole("button",{name:"End voice"}).click();
  await page.waitForFunction(()=>window.testMic.readyState==="ended");
  if(await page.evaluate(()=>window.screenLocks)!==0) throw Error("Ending voice kept the display awake");
  await page.getByRole("button",{name:"Start voice"}).waitFor();
  await page.getByRole("heading",{name:"Work with Codex"}).waitFor();
  if (!requests.includes("stop")) throw Error("The Mac session was left running");
  // The Mac can switch chats while Safari's microphone permission sheet is open.
  const starts = requests.filter(a=>a==="start").length;
  await page.evaluate(()=>window.holdMic=true);
  await page.getByRole("button",{name:"Start voice"}).click();
  await page.waitForFunction(()=>typeof window.grantMic==='function');
  chat = "20000000-0000-4000-8000-000000000000";
  await page.waitForFunction(id=>snapshot?.chat===id,chat);
  await page.evaluate(()=>window.grantMic());
  await page.waitForFunction(()=>window.testMic.readyState==="ended",null,{timeout:4000});
  if(requests.filter(a=>a==="start").length!==starts) throw Error("Voice started in a different chat");
  await page.evaluate(()=>window.holdMic=false);
  phase='listening';
  await page.getByRole("button",{name:"Start voice"}).click();
  await page.getByRole("button",{name:"End voice"}).waitFor();
  await page.evaluate(()=>voice.mic.port.onmessage=null);
  await page.getByText("No microphone audio. Tap Start voice to reconnect.").waitFor({timeout:12000});
  if(await page.evaluate(()=>!!voice || window.testMic.readyState!=="ended")) throw Error("An unresponsive microphone left voice running");
  // Two seconds of input must reach the Mac at the same duration and pitch at 16 kHz.
  await page.evaluate(async()=>{
    for (const rate of [8000,11025,44100,48000]) {
      // Leave one render quantum beyond the last 80 ms packet so it isn't on WebKit's shutdown boundary.
      const ctx=new OfflineAudioContext(1,rate*2+128,rate); await ctx.audioWorklet.addModule('/mic.js');
      const mic=new AudioWorkletNode(ctx,'conversation-mic'), tone=ctx.createOscillator();
      let samples=0, cycles=0, last=0;
      const received=new Promise(resolve=>{mic.port.onmessage=({data})=>{
        for(const value of new Int16Array(data)){if(last<=0 && value>0) cycles++;last=value;}
        samples+=data.byteLength/2;if(samples>=32000) resolve();
      };});
      tone.frequency.value=300; tone.connect(mic).connect(ctx.destination); tone.start();
      await ctx.startRendering(); await Promise.race([received,new Promise(r=>setTimeout(r,1500))]);
      if(samples!==32000 || Math.abs(cycles-600)>1) throw Error('Incorrect microphone conversion at '+rate+' Hz: '+samples+' samples, '+cycles+' cycles');
    }
  });
  return 'Conversations Safari checks passed';
}
