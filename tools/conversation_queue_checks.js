// Durable Mac queue controls, separate from the browser's offline outbox.
async (page, fixture) => {
  const chat='10000000-0000-4000-8000-000000000000';
  Object.assign(fixture,{chat,serverQueue:true,busy:true,queuePaused:false,queue:[],revision:1,attempts:0,
    accepted:new Map(),messages:new Map([[chat,[{speaker:'You',text:'Working turn'}]]])});
  await page.waitForFunction(()=>snapshot?.serverQueue);
  const send=async text=>{await page.getByRole('textbox',{name:'Message'}).fill(text);await page.getByRole('button',{name:'Send message',exact:true}).click();};
  await send('First saved message');
  await page.waitForFunction(()=>snapshot?.queue?.length===1);
  if(await page.evaluate(()=>queued().length)!==0)throw Error('Acknowledged queue item remained in the offline outbox');
  await send('Second saved message');
  await page.waitForFunction(()=>snapshot?.queue?.length===2);
  fixture.offline=true;
  await page.waitForFunction(()=>connected===false);
  await send('Third offline message');
  await page.reload({waitUntil:'domcontentloaded'});
  fixture.offline=false;
  await page.waitForFunction(()=>connected===true && [...document.querySelectorAll('#outbox article > div')].map(node=>node.textContent).join('|')==='First saved message|Second saved message|Third offline message');
  const texts=await page.locator('#outbox article > div').allTextContents();
  if(texts.join('|')!=='First saved message|Second saved message|Third offline message')throw Error('Offline outbox appeared ahead of the already saved queue: '+JSON.stringify(texts));
  await page.locator('#outbox article').filter({hasText:'Third offline message'}).getByRole('button',{name:'Cancel queued message'}).click();
  await page.waitForFunction(()=>queued().length===0 && snapshot?.queue?.length===2);
  await page.reload();
  await page.waitForFunction(()=>document.querySelector('#outbox').children.length===2);
  await page.locator('#outbox article').filter({hasText:'First saved message'}).getByRole('button',{name:'Cancel queued message'}).click();
  await page.waitForFunction(()=>snapshot?.queue?.length===1);
  if(fixture.queue[0].text!=='Second saved message')throw Error('Cancelled the wrong queued message');
  await page.getByRole('button',{name:'Stop reply',exact:true}).click();
  await page.getByRole('button',{name:'Resume queue',exact:true}).waitFor({state:'visible'});
  if(fixture.busy || !fixture.queuePaused || fixture.queue.length!==1)throw Error('Stop must pause and retain the queue');
  await page.getByRole('button',{name:'Resume queue',exact:true}).click();
  await page.waitForFunction(()=>snapshot?.queue?.length===0);
  if(fixture.messages.get(chat).at(-1).text!=='Second saved message')throw Error('Resume did not send the retained message');
  // A local offline item can be removed without contacting the Mac.
  fixture.offline=true;
  await page.waitForFunction(()=>connected===false);
  await send('Cancel while offline');
  await page.locator('#outbox article').filter({hasText:'Cancel while offline'}).getByRole('button',{name:'Cancel queued message'}).click();
  await page.waitForFunction(()=>queued().length===0);
  fixture.offline=false;
  await page.waitForFunction(()=>connected===true);
  if(fixture.queue.length!==0)throw Error('Cancelled offline message was sent on reconnect');
  return 'Conversation queue checks passed: server persistence, reload, per-message cancellation, Stop, Resume and offline cancellation';
}
