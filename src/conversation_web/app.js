'use strict';
const $ = id => document.getElementById(id);
let chat, voice, starting, snapshot, after, refreshCount = 0, rendered, before;
let connected = null, sending = false;
let listing = false;
const wide = matchMedia('(min-width: 800px)');
const draftKey = id => 'conversation-draft:' + (id || 'new');
const queuePrefix = 'conversation-send:';
function queued() {
  return Object.keys(localStorage).filter(key=>key.startsWith(queuePrefix))
    .map(key=>JSON.parse(localStorage.getItem(key))).sort((a,b)=>a.created-b.created || a.id.localeCompare(b.id));
}
function connection() {
  text($('connection'), connected ? '' : connected===null ? 'Connecting to your Mac…' : 'Offline · messages stay on this device until your Mac reconnects.');
  $('talk').disabled = !!starting || (!voice && (!connected || (!!snapshot && snapshot.voice.phase!=='idle')));
  if(!connected && !voice) text($('status'),connected===null ? 'Connecting…' : 'Offline');
  $('fresh').disabled = !connected; $('history').disabled = !connected;
  $('newChat').disabled = !connected;
  for(const row of $('list').children) row.disabled=!connected;
  $('composer').querySelector('button').disabled = !chat;
}
function outbox() {
  const list=$('outbox'), old=new Map(Array.from(list.children,el=>[el.key,el]));
  const local=queued(), remote=(snapshot?.queue || []).map(item=>({...item,id:item.id.toLowerCase(),chat,remote:true}));
  const items=[...remote,...local.filter(item=>!remote.some(saved=>saved.id===item.id))];
  for(const [index,item] of items.entries()) {
    let row=old.get(item.id); old.delete(item.id);
    if(!row) {
      row=document.createElement('article'); row.key=item.id; row.className='user queued';
      row.append(document.createElement('div'),document.createElement('small'),document.createElement('button'),document.createElement('button'));
      row.children[2].className='button';
      row.lastChild.className='button'; list.append(row);
    }
    if(list.children[index]!==row)list.insertBefore(row,list.children[index] || null);
    text(row.firstChild,item.text);
    text(row.children[1],item.remote ? (snapshot.queuePaused ? 'Queued · paused' : 'Queued · waiting for this turn to finish') : item.error || (item.chat!==chat ? 'Queued in another chat' : connected ? (snapshot?.canSend ? 'Queued · ready to send' : 'Queued · waiting for this turn to finish') : 'Queued · waiting for connection'));
    const action=row.children[2];
    action.hidden=item.remote || (!item.error && item.chat===chat);
    text(action,item.chat!==chat ? 'Open chat' : 'Retry');
    action.onclick=async()=>{try{
      if(item.chat!==chat) { await api('open',{chat:item.chat}); before=null; rendered=null; await refresh(); }
      else { text($('notice'),''); delete item.error; localStorage.setItem(queuePrefix+item.id,JSON.stringify(item)); outbox(); await flushQueue(); }
    }catch(e){notice(e);}};
    const cancel=row.lastChild; text(cancel,'Cancel'); cancel.disabled=item.chat!==chat;
    cancel.setAttribute('aria-label','Cancel queued message');
    cancel.onclick=async()=>{cancel.disabled=true;try{
      await navigator.locks.request('conversation-send',async()=>{
        // Wait for any in-flight send so cancelling cannot race its acknowledgement.
        if(item.remote || !localStorage.getItem(queuePrefix+item.id)) {
          const result=await api('cancel_queued',{chat:item.chat,message:item.id});
          if(result.cancelled===false)text($('notice'),'This message has already been sent.');
        } else {
          if(connected && snapshot?.serverQueue) await api('cancel_queued',{chat:item.chat,message:item.id});
        }
        localStorage.removeItem(queuePrefix+item.id);
      });
      outbox(); if(connected)await refresh();
    }catch(e){notice(e);}finally{cancel.disabled=false;}};
  }
  for(const row of old.values())row.remove();
}
async function flushQueue() {
  if(sending || !connected || !snapshot?.queuedSend || !snapshot.canSend || $('pair').open) return;
  sending=true;
  try {
    await navigator.locks.request('conversation-send',{ifAvailable:true},async lock=>{
      if(!lock) return;
      const item=queued().find(item=>item.chat===chat);
      if(!item || item.error) return;
      try {
        const state=await api('state');
        if(state.chat!==item.chat || !state.canSend || !state.queuedSend) return;
        const result=await api('send',{chat:item.chat,text:item.text,message:item.id});
        if(result.accepted?.toLowerCase()!==item.id) throw Error('The Mac did not confirm this queued message. Update Hey Boss before retrying.');
        localStorage.removeItem(queuePrefix+item.id);
      } catch(e) {
        if(connected && !$('pair').open) { item.error=e.message; localStorage.setItem(queuePrefix+item.id,JSON.stringify(item)); }
        notice(e,!connected);
      }
      outbox();
      await refresh().catch(e=>notice(e,true));
    });
  } catch(e) { notice(e); }
  finally { sending=false; }
}
function text(el, value) { if (el.textContent !== value) el.textContent = value; }
function notice(e, polling = false) {
  text($('notice'), polling && !connected ? '' : e.message || String(e));
  $('notice').toggleAttribute('data-connection', polling);
}
async function api(action, fields = {}) {
  let res;
  try { res = await fetch('/api', {method:'POST', headers:{'Content-Type':'application/json'},
    body:JSON.stringify({action, chat, session:voice?.id, ...fields}), signal:AbortSignal.timeout(6500)}); }
  catch { connected=false; connection(); throw Error('The Mac is unavailable. Check the connection and try again.'); }
  if (res.status === 403) {
    connected=false; connection();
    $('chats').close();
    if (!$('pair').open) $('pair').show();
    throw Error('Pair this device to continue.');
  }
  if (!res.ok) { connected=false; connection(); throw Error('The Mac is unavailable. Check the connection and try again.'); }
  let data;
  try { data=await res.json(); } catch { connected=false; connection(); throw Error('The Mac’s reply was interrupted. Reconnecting…'); }
  if (data.status !== 'ok') throw Error(data.error || 'The Mac could not complete this action.');
  return data;
}
function button(id, available, label) {
  const el = $(id); el.hidden = !available; if (label) text(el, label);
}
async function refresh() {
  const sequence = ++refreshCount;
  const s = await api('state',{revision:rendered,before});
  if (sequence !== refreshCount) return;
  if ($('notice').hasAttribute('data-connection')) {
    $('notice').removeAttribute('data-connection'); text($('notice'), '');
  }
  if (chat && chat !== s.chat) {
    if (starting) cleanup(starting);
    try { saveDraft(); $('draft').value=localStorage.getItem(draftKey(s.chat)) || ''; }
    catch { notice(Error('Could not save the draft on this device.')); }
    if (before!=null) { before=null; rendered=null; chat=s.chat; return refresh(); }
  }
  const wasConnected = connected;
  connected=true;
  const previous = snapshot;
  if(!chat) {
    try {
      $('draft').value=$('draft').value || localStorage.getItem(draftKey(s.chat)) || '';
      if($('draft').value) localStorage.setItem(draftKey(s.chat),$('draft').value);
      localStorage.removeItem(draftKey(null));
    }
    catch { notice(Error('Could not read the saved draft on this device.')); }
  }
  chat = s.chat; snapshot = s;
  try { localStorage.setItem('conversation-chat',chat); } catch { notice(Error('Could not save on this device.')); }
  const v = s.voice, mine = !!voice && s.mine;
  $('empty').hidden = v.phase!=='idle' || s.end>0;
  text($('status'), v.approval ? (mine ? 'Approval needed' : 'Approve on your Mac') : v.phase === 'idle' ? (v.detail || 'Ready') : v.message);
  const recording = mine && v.phase === 'recording';
  $('talk').ariaLabel = recording ? 'Send voice message' : mine ? 'End voice' : 'Start voice';
  text($('voice-label'), recording ? 'Send now' : v.phase === 'idle' ? 'Start voice' : '');
  $('talk').dataset.phase = v.phase;
  $('talk').disabled = !!starting || (!mine && v.phase !== 'idle');
  if (mine) {
    voice.setMuted(s.muted);
    if (previous?.voice.listeningPaused && !v.listeningPaused && voice.timer) {
      clearTimeout(voice.timer); pump(voice);
    }
  }
  button('mute', mine && v.phase !== 'starting', v.listeningPaused != null
    ? (v.listeningPaused ? 'Resume listening' : 'Pause listening') : (s.muted ? 'Unmute microphone' : 'Mute microphone'));
  button('pause', mine && v.phase === 'speaking', s.paused ? 'Resume reply' : 'Pause reply');
  button('interrupt', mine && v.interrupt, s.interruptLabel || 'Interrupt reply');
  button('end', recording);
  button('cancel', !!s.canCancel);
  button('resumeQueue', !!s.queuePaused && !!s.queue?.length);
  if (previous?.passage !== s.passage || previous?.spoken !== s.spoken) {
    $('caption').replaceChildren();
    if (s.passage) {
      const heard = document.createElement('b'); heard.textContent = s.passage.slice(0,s.spoken);
      $('caption').append(heard,s.passage.slice(s.spoken));
    }
  }
  $('approval').hidden = !mine || !s.approval;
  if (s.approval) text($('approval').querySelector('pre'),s.approval.text);
  if (s.messages) {
    $('empty').querySelector('h1').textContent = s.heading || 'Start a conversation';
    $('empty').querySelector('p').textContent = s.hint || '';
    $('draft').placeholder = s.placeholder || 'Message Hey Boss…';
    button('earlier', s.start>0); button('latest', before!=null);
    const entering = rendered==null || previous?.chat!==s.chat;
    const timeline = $('timeline'), follow = entering || timeline.scrollHeight-timeline.scrollTop-timeline.clientHeight < 70;
    const list = $('messages'), nodes = [];
    const old = new Map(previous?.chat===s.chat ? Array.from(list.children,el=>[el.key,el]) : []);
    const rowFor = (key,tag) => {
      const el=old.get(key) || document.createElement(tag); el.key=key; nodes.push(el); return el;
    };
    const inserts = at => {
      for (const [i,tool] of s.tools.filter(t=>t.at===at).entries()) {
        const row=rowFor('tool:'+at+':'+i,'details');
        if (!row.firstChild) row.append(document.createElement('summary'),document.createElement('pre'));
        text(row.firstChild,tool.label+' · '+tool.state); text(row.lastChild,tool.detail);
      }
      for (const visual of s.visuals.filter(v=>v.at===at)) {
        const b=rowFor('image:'+visual.id,'button');
        if (b.tagName==='IMG') continue;
        text(b,visual.title || 'Show image');
        b.onclick=async()=>{b.disabled=true;try{const r=await api('image',{media:visual.id});
          if (!b.isConnected) return;
          const img=document.createElement('img'); img.key=b.key; img.alt=b.textContent;
          img.src='data:image/png;base64,'+r.png;b.replaceWith(img);}catch(e){b.disabled=false;notice(e);}};
      }
    };
    inserts(s.start);
    for (const [i,msg] of s.messages.entries()) {
      const row = rowFor(s.start+i,'article');
      if (row.speaker!==msg.speaker || row.content!==(msg.html ?? msg.text)) {
        row.speaker=msg.speaker; row.content=msg.html ?? msg.text;
        row.className = msg.speaker === 'You' ? 'user' : 'markdown';
        row.setAttribute('aria-label', msg.speaker);
        if(msg.html!=null) row.innerHTML=msg.html; else row.textContent=msg.text;
      }
      inserts(s.start+i+1);
    }
    for (const [i,node] of nodes.entries()) if (list.children[i]!==node) list.insertBefore(node,list.children[i] || null);
    while (list.children.length>nodes.length) list.lastElementChild.remove();
    rendered = s.revision;
    if (before!=null) { if (entering) timeline.scrollTop=0; }
    else if (follow) timeline.scrollTop = timeline.scrollHeight;
  }
  connection();
  if(wide.matches && !$('pair').open && !$('chats').open) {
    const focused=document.activeElement; $('chats').show(); focused?.focus({preventScroll:true});
  }
  let selectedRow=false;
  for(const row of $('list').children) { const selected=row.dataset.chat===chat; if(selected)row.setAttribute('aria-current','true');else row.removeAttribute('aria-current'); selectedRow ||= selected; }
  if($('chats').open && (!previous || (previous.chat!==chat && !selectedRow) || !wasConnected || (!after && $('list').children.length<=30 && (previous.end!==s.end || (previous.voice.phase!=='idle' && v.phase==='idle'))))) void list(true);
  try { outbox(); void flushQueue(); } catch { notice(Error('Could not read queued messages on this device.')); }
}
function cleanup(v) {
  if (!v || v.closed) return;
  v.closed = true; clearTimeout(v.timer);
  v.track?.getTracks().forEach(t => { t.onended=null; t.stop(); });
  for (const node of v.nodes) { try { node.stop(); } catch {} }
  v.nodes.clear();
  v.wake?.release().catch(()=>{});
  let closing;
  const closeAudio = () => {
    clearTimeout(closing);
    v.source?.disconnect(); v.mic?.disconnect();
    // Close both the processor and its port: Chrome and Safari retain different edges.
    if (v.mic) { v.mic.port.onmessage = null; v.mic.port.close(); }
    const close = () => v.ctx.close().catch(()=>{});
    // Safari can deliver a pending unlock after close() has finished.
    v.ctx.onstatechange = () => { if (v.ctx.state === 'running') close(); };
    if (v.ctx.state !== 'closed') close();
  };
  if (v.mic && v.ctx.state === 'running') {
    v.mic.port.onmessage = ({data}) => {
      if (data !== null) return;
      clearTimeout(closing); v.source?.disconnect(); v.mic.disconnect();
      // Let Chrome render the disconnected graph before closing its context.
      closing = setTimeout(closeAudio,50);
    };
    v.mic.port.postMessage(null);
    closing = setTimeout(closeAudio,250); // A suspended or failed worklet cannot acknowledge.
  } else closeAudio();
  if (voice === v) voice = null;
  if (starting === v) starting = null;
  $('talk').disabled = false; $('talk').ariaLabel = 'Start voice'; $('talk').dataset.phase = 'idle';
  text($('voice-label'), 'Start voice'); button('end', false);
}
async function stop(v, tell = true) {
  if (!v || v.closed) return;
  cleanup(v);
  if (tell) await api('stop', {session:v.id});
}
function play(v, r) {
  const now = v.ctx.currentTime;
  if (v.gen !== r.generation || r.paused) {
    for (const n of v.nodes) { try { n.stop(); } catch {} }
    v.nodes.clear();
    v.received = v.gen === r.generation ? v.played : 0;
    v.played = v.received; v.segments = []; v.end = now; v.gen = r.generation;
  }
  if (!r.pcm || r.paused) return;
  if (r.offset !== v.received) throw Error('Playback lost its place. Start voice again.');
  const raw = Uint8Array.from(atob(r.pcm),c=>c.charCodeAt(0)), pcm = new Int16Array(raw.buffer);
  const buffer = v.ctx.createBuffer(1,pcm.length,r.rate), out = buffer.getChannelData(0);
  for (let i=0;i<pcm.length;i++) out[i] = pcm[i]/32768;
  const n = v.ctx.createBufferSource(); n.buffer = buffer; n.connect(v.ctx.destination);
  const at = Math.max(now+0.012,v.end);
  v.segments.push({at,offset:r.offset,length:pcm.length,rate:r.rate});
  v.end = at+buffer.duration; v.received += pcm.length;
  v.nodes.add(n); n.onended = () => v.nodes.delete(n); n.start(at);
}
async function pump(v) {
  v.timer = null;
  if (v.closed) return;
  try {
    if (v.ctx.state !== 'running') throw Error('Audio interrupted. Tap Start voice to reconnect.');
    if (!v.track.getAudioTracks()[0].enabled) v.lastCapture = performance.now();
    else if (performance.now()-v.lastCapture > 8000) throw Error('No microphone audio. Tap Start voice to reconnect.');
    for (const s of v.segments) {
      const elapsed = Math.floor((v.ctx.currentTime-s.at)*s.rate);
      if (elapsed < 0) break;
      v.played = Math.max(v.played,s.offset+Math.min(elapsed,s.length));
    }
    v.segments = v.segments.filter(s => s.offset+s.length > v.played);
    // Each chunk is 80 ms. Keep short stalls in order until the Mac's five-second
    // disconnect deadline, but send at most 15 KiB per exchange.
    if (v.input.length > 60) throw Error('Microphone connection fell behind. Start voice again.');
    const chunks = v.input.splice(0,6);
    const bytes = new Uint8Array(chunks.length*2560);
    chunks.forEach((c,i)=>bytes.set(new Uint8Array(c),i*2560));
    const r = await api('exchange',{session:v.id,generation:v.gen,played:v.played,received:v.received,
      pcm:bytes.length ? btoa(String.fromCharCode(...bytes)) : undefined});
    if (!v.closed) {
      play(v,r);
      const delay = snapshot?.voice.listeningPaused && !v.segments.length ? 450 : 65;
      v.timer = setTimeout(()=>pump(v),v.input.length >= 6 ? 0 : delay);
    }
  } catch(e) { if (!v.closed) { stop(v).catch(()=>{}); notice(e); } }
}
async function start() {
  if (starting || voice || !chat) return;
  const selected = chat;
  $('notice').textContent = '';
  if (!navigator.mediaDevices?.getUserMedia) throw Error('Open the trusted HTTPS address in Safari to use the microphone.');
  const ctx = new AudioContext(), v = {ctx,id:crypto.randomUUID(),nodes:new Set(),segments:[],input:[],gen:-1,played:0,received:0,end:0};
  starting = v; $('talk').disabled = true;
  try {
    // Unlock output inside the tap, before waiting for microphone permission.
    await Promise.all([ctx.resume(),
      navigator.mediaDevices.getUserMedia({audio:{channelCount:1,echoCancellation:true,noiseSuppression:true}}).then(track=>{
        if (v.closed) track.getTracks().forEach(t=>t.stop()); else v.track=track;
      })]);
    if (v.closed) return;
    await ctx.audioWorklet.addModule('/mic.js');
    if (v.closed) return;
    const source = ctx.createMediaStreamSource(v.track), mic = new AudioWorkletNode(ctx,'conversation-mic');
    source.connect(mic); mic.connect(ctx.destination); v.mic = mic; v.source = source;
    const heights = Array(11).fill(4);
    const drawMeter = () => $('levels').firstElementChild.setAttribute('d', heights.map((h,i)=>`M${2+i*5} ${12-h/2}v${h}`).join(' '));
    drawMeter();
    const track = v.track.getAudioTracks()[0];
    v.setMuted = muted => {
      if (track.enabled === !muted) return;
      track.enabled = !muted; mic.port.postMessage(muted);
      if (muted) { v.input.length=0; heights.fill(4); drawMeter(); }
      else v.lastCapture=performance.now();
    };
    mic.port.onmessage = e => {
      if (voice !== v || !track.enabled) return;
      v.lastCapture = performance.now();
      if (v.input.length <= 60) v.input.push(e.data);
      if (!['listening','recording'].includes($('talk').dataset.phase)) return;
      const pcm = new Int16Array(e.data);
      let sum = 0;
      for (const n of pcm) sum += n*n;
      heights.shift(); heights.push(4 + 18 * Math.min(1, Math.sqrt(sum/pcm.length)/4096)**0.75);
      drawMeter();
    };
    track.onended = () => { if (!v.closed) { stop(v).catch(()=>{}); notice(Error('Microphone disconnected.')); } };
    await api('start',{session:v.id,chat:selected});
    if (v.closed) { await api('stop',{session:v.id}); return; }
    voice = v; starting = null; v.lastCapture = performance.now(); pump(v);
    navigator.wakeLock?.request('screen').then(lock=>{
      if (v.closed) return lock.release();
      v.wake=lock;
    }).catch(()=>{});
    await refresh().catch(e=>notice(e,true));
  } catch(e) { if (!v.closed) { cleanup(v); throw e; } }
}
const act = (id, fn) => { const el=$(id); el.onclick=async()=>{
  el.disabled=true; $('notice').textContent='';
  try { await fn(); } catch(e) { notice(e); } finally { el.disabled=false; }
}; };
// Start the AudioContext synchronously in the Safari user gesture.
$('talk').onclick = () => {
  if (!voice) return start().catch(notice);
  const action = snapshot.voice.phase==='recording' ? api('finish') : stop(voice);
  action.then(refresh).catch(notice);
};
act('cancel',async()=>{await api('cancel');await refresh();});
act('resumeQueue',async()=>{await api('resume_queue');await refresh();});
act('end',async()=>{if (voice) await stop(voice); await refresh();});
for (const id of ['mute','pause','interrupt']) act(id,async()=>{await api(id); await refresh();});
act('earlier',async()=>{before=snapshot.start;rendered=null;await refresh();});
act('latest',async()=>{before=null;rendered=null;await refresh();});
for (const action of ['allow','decline']) act(action,async()=>{await api(action,{text:snapshot.approval.id});await refresh();});
for(const id of ['fresh','newChat']) act(id,async()=>{ if (voice) await stop(voice); await api('new'); if(!wide.matches)$('chats').close(); before=null; rendered=null; await refresh(); });
$('draft').onkeydown = e => {
  if (e.key==='Enter' && !e.shiftKey && !e.isComposing && e.keyCode!==229) {
    e.preventDefault(); if (!e.repeat) $('composer').requestSubmit();
  }
};
function saveDraft() {
  if($('draft').value) localStorage.setItem(draftKey(chat),$('draft').value);
  else localStorage.removeItem(draftKey(chat));
}
$('draft').oninput=()=>{try{saveDraft();}catch{notice(Error('Could not save on this device. Keep this page open or copy your message.'));}};
$('composer').onsubmit = async e => {
  e.preventDefault(); const text=$('draft').value.trim(); if(!text || !chat) return;
  try {
    const item={id:crypto.randomUUID(),chat,text,created:Math.max(Date.now(),...queued().map(item=>item.created+1))};
    localStorage.setItem(queuePrefix+item.id,JSON.stringify(item));
    $('draft').value=''; saveDraft(); $('notice').textContent=''; outbox();
    $('timeline').scrollTop=$('timeline').scrollHeight;
    await flushQueue();
  } catch { notice(Error('Could not save on this device. Keep this page open or copy your message.')); }
};
async function list(reset=false) {
  if(listing) return;
  listing=true; $('more').disabled=true; $('retryChats').hidden=true;
  try {
  const data=await api('list',{after:reset ? null : after});
  after=data.next;
  if(reset) $('list').replaceChildren();
  for (const c of data.chats) {
    const b=document.createElement('button'); b.textContent=c.title; b.title=c.title; b.dataset.chat=c.id;
    if (c.id===chat) b.setAttribute('aria-current','true');
    b.onclick=async()=>{
      if(!wide.matches) $('chats').close();
      if (c.id===chat) return;
      try{if(voice)await stop(voice);await api('open',{chat:c.id});before=null;rendered=null;await refresh();}catch(e){notice(e);}
    };
    $('list').append(b);
  }
  text($('chatStatus'),$('list').children.length ? '' : 'No saved chats yet.');
  $('more').hidden=!after;
  } catch(e) {
    text($('chatStatus'),'Couldn’t load chats. Check your Mac’s connection and retry.'); $('retryChats').hidden=false;
  } finally { listing=false; $('more').disabled=false; }
}
act('history',async()=>{await list(true);if(!$('pair').open && !$('chats').open)$('chats').showModal();$('history').ariaExpanded=String($('chats').open);});
act('more',()=>list()); act('retryChats',()=>list(true)); act('close',()=>$('chats').close());
$('chats').addEventListener('close',()=>{$('history').ariaExpanded=String($('chats').open);});
$('chats').onclick=e=>{if(wide.matches || e.target!==$('chats'))return;const r=$('chats').getBoundingClientRect();if(e.clientX<r.left || e.clientX>r.right || e.clientY<r.top || e.clientY>r.bottom)$('chats').close();};
wide.addEventListener('change',()=>{$('chats').close();if(wide.matches && !$('pair').open){$('chats').show();if(connected)void list(true);}});
$('pairForm').onsubmit=async e=>{e.preventDefault();try{
  $('pairForm').querySelector('button').disabled=true; $('pairError').textContent='';
  const r=await fetch('/pair',{method:'POST',body:$('code').value.trim(),signal:AbortSignal.timeout(6500)});
  if(!r.ok)throw Error(r.status===403 ? 'That code didn’t match. Copy the pairing code from your Mac and try again.' : 'Your Mac could not connect. Try again.');
  $('code').value=''; $('pair').close(); $('notice').textContent=''; await refresh().catch(e=>notice(e,true));
}catch(e){$('pairError').textContent=e instanceof TypeError || e.name==='TimeoutError' ? 'Couldn’t reach your Mac. Check that it’s awake and on the same Wi-Fi, then try again.' : e.message;}
finally{$('pairForm').querySelector('button').disabled=false;}};
function leave() {
  const v=voice || starting; if(!v)return;
  cleanup(v); navigator.sendBeacon('/api',JSON.stringify({action:'stop',chat,session:v.id}));
}
addEventListener('pagehide',leave);
addEventListener('offline',()=>{connected=false;connection();outbox();});
addEventListener('online',()=>refresh().catch(e=>notice(e,true)));
addEventListener('storage',()=>{outbox();void flushQueue();});
try {
  chat=localStorage.getItem('conversation-chat');
  $('draft').value=localStorage.getItem(draftKey(chat)) || '';
  connection(); outbox();
} catch { notice(Error('Could not read saved messages on this device.')); }
async function enableOffline() {
  if(!('serviceWorker' in navigator)) return;
  $('retryOffline').disabled=true;
  try { await navigator.serviceWorker.register('/sw.js'); $('offlineNotice').hidden=true; }
  catch(e) {
    console.warn('Offline setup failed:',e);
    $('offlineNotice').hidden=!!navigator.serviceWorker.controller;
    text($('offlineNotice').querySelector('span'),e.name==='SecurityError'
      ? 'Trust your Mac’s certificate to enable offline opening, then retry.'
      : 'Offline opening is unavailable. You can still use Conversations while connected.');
  } finally { $('retryOffline').disabled=false; }
}
$('retryOffline').onclick=enableOffline;
void enableOffline();
document.addEventListener('visibilitychange',()=>{if(document.hidden)leave();});
(async function observe(){if(!document.hidden && !$('pair').open){try{await refresh();}catch(e){notice(e,true);}}
  setTimeout(observe,voice || snapshot?.voice.phase==='thinking' ? 450 : 1400);
})();
