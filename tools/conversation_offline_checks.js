// Run through conversation_browser_checks.js with real assets and service workers.
// Every chat operation stays in this fixture; no provider or live chat is used.
async (page, fixture) => {
  const origin=new URL(page.url()).origin, context=page.context();
  const first='10000000-0000-4000-8000-000000000000', second='20000000-0000-4000-8000-000000000000';
  Object.assign(fixture,{chat:first,offline:false,busy:false,loseReply:false,revision:1,attempts:0,
    accepted:new Map(),messages:new Map([[first,[]],[second,[]]])});
  const {accepted,messages}=fixture;
  // WebKit simulates a lost Mac connection; Chrome also disables browser networking.
  const disconnect=async offline=>{fixture.offline=offline;if(fixture.browser==='chrome')await context.setOffline(offline);};
  await page.waitForFunction(()=>!!snapshot?.queuedSend);
  await page.waitForFunction(()=>!!navigator.serviceWorker.controller);
  const field=page.getByRole('textbox',{name:'Message'});
  const send=async text=>{await field.fill(text);await page.getByRole('button',{name:'Send message'}).click();};
  await field.fill('Draft survives closing.');
  await disconnect(true);
  await page.reload({waitUntil:'domcontentloaded'});
  if(await field.inputValue()!=='Draft survives closing.') throw Error('Offline reload lost the draft');
  if(!await page.evaluate(()=>getComputedStyle(document.body).display==='flex' && [...document.styleSheets].filter(s=>s.href).every(s=>s.cssRules.length))) throw Error('Offline shell lost its styles');
  await send('First queued message.');
  await send('Second queued message.');
  await page.reload({waitUntil:'domcontentloaded'});
  if(await page.locator('#outbox article').count()!==2 || await field.inputValue()!=='') throw Error('Offline queue was not persisted before clearing the draft');
  await field.fill('An unsent draft.');
  fixture.busy=true;await disconnect(false);
  await page.waitForFunction(()=>connected);
  if(fixture.attempts!==0) throw Error('Busy Mac received a queued send');
  fixture.busy=false;
  await page.waitForFunction(()=>!document.querySelector('#outbox').children.length);
  if([...accepted.values()].join('|')!=='First queued message.|Second queued message.' || await field.inputValue()!=='An unsent draft.') throw Error('Reconnect reordered messages or erased the draft');
  // The Mac accepts the message, but its acknowledgement never reaches the browser.
  fixture.loseReply=true;await send('Lost acknowledgement.');
  await page.waitForFunction(()=>!connected);
  await disconnect(true);await page.reload({waitUntil:'domcontentloaded'});
  if(await page.locator('#outbox article').count()!==1) throw Error('Lost acknowledgement dropped the queued message');
  await disconnect(false);
  await page.waitForFunction(()=>!document.querySelector('#outbox').children.length);
  if(accepted.size!==3 || fixture.attempts!==4) throw Error('Acknowledgement retry duplicated a message');
  // A changed selection must never send a queued message to the new conversation.
  await disconnect(true);await send('Belongs to first chat.');
  fixture.chat=second;fixture.revision++;await disconnect(false);
  await page.waitForFunction(id=>snapshot?.chat===id,second);
  if(messages.get(second).length || accepted.size!==3) throw Error('Queue followed the Mac into another chat');
  await page.getByRole('button',{name:'Open chat',exact:true}).click();
  await page.waitForFunction(()=>!document.querySelector('#outbox').children.length);
  if(messages.get(first).at(-1).text!=='Belongs to first chat.') throw Error('Original chat did not receive its queued message');
  // Two open tabs share the outbox and must not race its writes or sends.
  fixture.busy=true;await page.waitForFunction(()=>snapshot.canSend===false);
  await send('One shared outbox.');
  const other=await context.newPage();await other.goto(origin);
  await other.waitForFunction(()=>!!snapshot);
  if(await other.locator('#outbox article').count()!==1) throw Error('Second tab cannot see the queue');
  const before=fixture.attempts;fixture.busy=false;
  await page.waitForFunction(()=>!document.querySelector('#outbox').children.length);
  await other.waitForFunction(()=>!document.querySelector('#outbox').children.length);
  if(fixture.attempts-before!==1 || accepted.size!==5) throw Error('Two tabs submitted the same queue item');
  await other.close();
  // A full/unavailable browser store must keep the text visible instead of claiming success.
  await page.evaluate(()=>{window.originalSet=Storage.prototype.setItem;Storage.prototype.setItem=function(k,v){if(k.startsWith('conversation-send:'))throw new DOMException('Full','QuotaExceededError');return window.originalSet.call(this,k,v);};});
  await send('Keep on storage failure.');
  if(await field.inputValue()!=='Keep on storage failure.' || accepted.size!==5) throw Error('Storage failure discarded or sent the draft');
  await page.evaluate(()=>Storage.prototype.setItem=window.originalSet);
  return 'Conversations offline checks passed: reload, ordered queue, busy Mac, lost acknowledgement, chat changes, two tabs, storage failure';
}
