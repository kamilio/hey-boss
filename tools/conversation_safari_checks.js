// Run with Playwright WebKit on the Conversations page. All chat API calls
// below stay in the fixture; capture and playback use real Web Audio.
async (page) => {
  const origin = new URL(page.url()).origin;
  const chat = "10000000-0000-4000-8000-000000000000";
  let active = false, muted = false, recorded = false, heard = false;
  const requests = [];
  await page.route(origin + "/api", async route => {
    const p = route.request().postDataJSON();
    requests.push(p.action);
    const r = {status:"ok"};
    if (p.action === "start") active = true;
    if (p.action === "stop") active = false;
    if (p.action === "mute") muted = !muted;
    if (p.action === "state") Object.assign(r, {
      chat, mine:active, voice:{phase:active ? "listening":"idle", message:"Listening"},
      heading:"Work with Codex", hint:"Folder conversation", placeholder:"Message Codex…",
      paused:false, muted, start:0, end:0, revision:1, messages:[], tools:[], visuals:[]
    });
    if (p.action === "exchange") {
      recorded ||= !!p.pcm && Buffer.from(p.pcm,"base64").some(x=>x!==0);
      heard ||= p.played === 1600;
      Object.assign(r,{generation:1,paused:false});
      if (!p.received) Object.assign(r,{rate:16000,offset:0,pcm:Buffer.alloc(3200).toString("base64")});
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
  await page.getByRole("button",{name:"Start voice"}).click();
  await page.getByRole("button",{name:"End voice"}).waitFor();
  const deadline=Date.now()+10000;
  while (!(recorded&&heard) && Date.now()<deadline) await new Promise(r=>setTimeout(r,50));
  if (!recorded || !heard) throw Error("Missing microphone frames or playback acknowledgment");
  await page.locator("#levels:visible").waitFor();
  await page.getByRole("button",{name:"Mute mic"}).click();
  await page.waitForFunction(()=>!window.testMic.enabled);
  await page.getByRole("button",{name:"Unmute mic"}).click();
  await page.waitForFunction(()=>window.testMic.enabled);
  await page.getByRole("button",{name:"End voice"}).click();
  await page.waitForFunction(()=>window.testMic.readyState==="ended");
  await page.getByRole("button",{name:"Start voice"}).waitFor();
  if (!requests.includes("stop")) throw Error("The Mac session was left running");
}
