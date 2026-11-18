// Run with WebKit. All state and mutations stay inside this fixture.
async page => {
  const origin = new URL(page.url()).origin;
  const chatA = '10000000-0000-4000-8000-000000000000', chatB = '20000000-0000-4000-8000-000000000000';
  let chat = chatA, failed = false, denySend = true, sends = 0, revision = 1, polls = 0, lists = 0;
  const messages = new Map([[chatA, []], [chatB, []]]);
  await page.route(origin+'/api', async route => {
    const p = route.request().postDataJSON(), r = {status:'ok'};
    if (p.action === 'state') {
      polls++;
      if (failed) return route.fulfill({status:503,body:''});
      Object.assign(r,{chat,voice:{phase:'idle',detail:'Voice off'},start:0,end:messages.get(chat).length,revision});
      if (p.revision!==revision || p.chat!==chat)
        Object.assign(r,{heading:'Start a conversation',messages:messages.get(chat),tools:[],visuals:[]});
    } else if (p.action === 'send') {
      if (denySend) return route.fulfill({json:{status:'error',error:'Wait for the current turn to finish.'}});
      messages.get(chat).push({speaker:'You',text:p.text}); revision++; sends++;
    } else if (p.action === 'open') { chat=p.chat; revision++; }
    else if (p.action === 'list') {
      lists++;
      await new Promise(r=>setTimeout(r,250));
      Object.assign(r,{chats:[{id:chatA,title:'First chat'},{id:chatB,title:'Second chat'}]});
    }
    return route.fulfill({json:r});
  });
  await page.goto(origin);
  const field=page.getByRole('textbox',{name:'Message'}), notice=page.locator('#notice');
  await page.getByRole('heading',{name:'Start a conversation'}).waitFor();
  await page.getByRole('button',{name:'Chats',exact:true}).dblclick();
  await page.getByRole('dialog',{name:'Chats'}).waitFor();
  if(lists!==1) throw Error('Double-tapping Chats requested the same page twice');
  await page.getByRole('button',{name:'Done'}).click();
  await field.fill('Keep this draft.');
  const open = async title => {
    await page.getByRole('button',{name:'Chats',exact:true}).click();
    await page.getByRole('button',{name:title,exact:true}).click();
    await page.waitForFunction(()=>!document.querySelector('#chats').open);
  };
  await open('Second chat');
  await page.waitForFunction(()=>document.querySelector('#draft').value==='');
  await field.fill('Another draft.');
  await open('First chat');
  await page.waitForFunction(()=>document.querySelector('#draft').value==='Keep this draft.');
  await page.getByRole('button',{name:'Send message'}).click();
  await page.getByText('Wait for the current turn to finish.').waitFor();
  if(await field.inputValue()!=='Keep this draft.') throw Error('Rejected send discarded the draft');
  denySend=false;
  await page.getByRole('button',{name:'Send message'}).click();
  await page.getByRole('article',{name:'You'}).getByText('Keep this draft.').waitFor();
  if(sends!==1 || await field.inputValue()!=='') throw Error('Retry did not send the draft exactly once');
  failed=true; await notice.getByText('The Mac is unavailable.',{exact:false}).waitFor();
  await field.fill('Offline draft.');
  failed=false;
  await notice.waitFor({state:'hidden',timeout:6000});
  if(await field.inputValue()!=='Offline draft.') throw Error('Reconnect discarded the draft');
  // A live region should announce a change, not every identical polling response.
  await page.evaluate(()=>{
    window.statusWrites=0;
    new MutationObserver(()=>window.statusWrites++).observe(document.querySelector('#status'),{childList:true,characterData:true,subtree:true});
  });
  const baseline=polls, deadline=Date.now()+6000;
  while(polls<baseline+2 && Date.now()<deadline) await new Promise(r=>setTimeout(r,100));
  if(polls<baseline+2 || await page.evaluate(()=>window.statusWrites)!==0) throw Error('Unchanged status was repeatedly announced');
}
