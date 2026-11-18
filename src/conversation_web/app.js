'use strict';
const $ = id => document.getElementById(id);
let chat, voice, starting, snapshot, after, refreshCount = 0, rendered, before;
const drafts = new Map();
function text(el, value) { if (el.textContent !== value) el.textContent = value; }
function notice(e, polling = false) {
  text($('notice'), e.message || String(e));
  $('notice').toggleAttribute('data-connection', polling);
}
async function api(action, fields = {}) {
  const res = await fetch('/api', {method:'POST', headers:{'Content-Type':'application/json'},
    body:JSON.stringify({action, chat, session:voice?.id, ...fields}), signal:AbortSignal.timeout(6500)});
  if (res.status === 403) {
    if (!$('pair').open) $('pair').showModal();
    throw Error('Pair this device to continue.');
  }
  if (!res.ok) throw Error('The Mac is unavailable. Check the connection and try again.');
  const data = await res.json();
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
    if ($('draft').value) drafts.set(chat,$('draft').value); else drafts.delete(chat);
    $('draft').value=drafts.get(s.chat) || ''; drafts.delete(s.chat);
    if (before!=null) { before=null; rendered=null; chat=s.chat; return refresh(); }
  }
  const previous = snapshot;
  chat = s.chat; snapshot = s;
  const v = s.voice, mine = !!voice && s.mine;
  text($('status'), v.approval ? (mine ? 'Approval needed' : 'Approve on your Mac') : v.phase === 'idle' ? (v.detail || 'Ready') : v.message);
  $('talk').ariaLabel = mine ? 'End voice' : 'Start voice';
  $('talk').dataset.phase = v.phase;
  $('talk').disabled = !!starting || (!mine && v.phase !== 'idle');
  if (mine) {
    const track=voice.track.getAudioTracks()[0];
    track.enabled=!s.muted;
    if (s.muted) voice.input.length=0;
  }
  button('mute', mine, s.muted ? 'Unmute mic' : 'Mute mic');
  button('pause', mine && v.phase === 'speaking', s.paused ? 'Resume reply' : 'Pause reply');
  button('interrupt', mine && v.interrupt);
  button('finish', mine && v.phase === 'recording');
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
    $('empty').hidden = s.messages.length > 0;
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
}
function cleanup(v) {
  if (!v || v.closed) return;
  v.closed = true; clearTimeout(v.timer); v.track?.getTracks().forEach(t => t.stop());
  v.wake?.release().catch(()=>{});
  if (v.ctx.state !== 'closed') v.ctx.close().catch(()=>{});
  if (voice === v) voice = null;
  if (starting === v) starting = null;
  $('talk').disabled = false; $('talk').ariaLabel = 'Start voice'; $('talk').dataset.phase = 'idle';
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
    if (!v.closed) { play(v,r); v.timer = setTimeout(()=>pump(v),v.input.length >= 6 ? 0 : 65); }
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
    const resumed = ctx.resume();
    v.track = await navigator.mediaDevices.getUserMedia({audio:{channelCount:1,echoCancellation:true,noiseSuppression:true}});
    if (v.closed) { v.track.getTracks().forEach(t=>t.stop()); return; }
    await resumed; await ctx.audioWorklet.addModule('/mic.js');
    const source = ctx.createMediaStreamSource(v.track), mic = new AudioWorkletNode(ctx,'conversation-mic');
    source.connect(mic); mic.connect(ctx.destination); v.mic = mic; v.source = source;
    const heights = Array(11).fill(4);
    const drawMeter = () => $('levels').firstElementChild.setAttribute('d', heights.map((h,i)=>`M${2+i*5} ${12-h/2}v${h}`).join(' '));
    drawMeter();
    mic.port.onmessage = e => {
      if (voice !== v) return;
      v.lastCapture = performance.now();
      const enabled = v.track.getAudioTracks()[0].enabled;
      if (enabled && v.input.length <= 60) v.input.push(e.data);
      if (!['listening','recording'].includes($('talk').dataset.phase)) return;
      const pcm = new Int16Array(e.data);
      let sum = 0;
      if (enabled) for (const n of pcm) sum += n*n;
      heights.shift(); heights.push(4 + 18 * Math.min(1, Math.sqrt(sum/pcm.length)/4096)**0.75);
      drawMeter();
    };
    v.track.getAudioTracks()[0].onended = () => { if (!v.closed) { stop(v).catch(()=>{}); notice(Error('Microphone disconnected.')); } };
    if (v.closed) return;
    await api('start',{session:v.id,chat:selected});
    if (v.closed) { await api('stop',{session:v.id}); return; }
    voice = v; starting = null; v.lastCapture = performance.now(); pump(v);
    navigator.wakeLock?.request('screen').then(lock=>{
      if (v.closed) return lock.release();
      v.wake=lock;
    }).catch(()=>{});
    await refresh().catch(e=>notice(e,true));
  } catch(e) { cleanup(v); throw e; }
}
const act = (id, fn) => { const el=$(id); el.onclick=async()=>{
  el.disabled=true; $('notice').textContent='';
  try { await fn(); } catch(e) { notice(e); } finally { el.disabled=false; }
}; };
// Start the AudioContext synchronously in the Safari user gesture.
$('talk').onclick = () => (voice ? stop(voice).then(refresh) : start()).catch(notice);
for (const id of ['mute','pause','interrupt','finish']) act(id,async()=>{await api(id); await refresh();});
act('earlier',async()=>{before=snapshot.start;rendered=null;await refresh();});
act('latest',async()=>{before=null;rendered=null;await refresh();});
for (const action of ['allow','decline']) act(action,async()=>{await api(action,{text:snapshot.approval.id});await refresh();});
act('fresh',async()=>{ if (voice) await stop(voice); await api('new'); before=null; rendered=null; await refresh(); });
$('composer').onsubmit = async e => { e.preventDefault(); const selected=chat, draft=$('draft').value, text=draft.trim(); if (!text) return;
  const submit=e.submitter || $('composer').querySelector('button'); if (submit.disabled) return; submit.disabled=true;
  $('notice').textContent='';
  try {
    await api('send',{text});
    if (chat===selected) { if ($('draft').value===draft) $('draft').value=''; }
    else if (drafts.get(selected)===draft) drafts.delete(selected);
    await refresh();
  }
  catch(e){notice(e);} finally{submit.disabled=false;}
};
async function list() {
  const data=await api('list',{after}); after=data.next;
  for (const c of data.chats) {
    const b=document.createElement('button'); b.textContent=c.title;
    if (c.id===chat) b.setAttribute('aria-current','true');
    b.onclick=async()=>{
      $('chats').close();
      if (c.id===chat) return;
      try{if(voice)await stop(voice);await api('open',{chat:c.id});before=null;rendered=null;await refresh();}catch(e){notice(e);}
    };
    $('list').append(b);
  }
  if (!$('list').children.length) $('list').textContent='No saved chats yet.';
  $('more').hidden=!after;
}
act('history',async()=>{after=null;$('list').replaceChildren();await list();$('chats').showModal();});
act('more',list); act('close',()=>$('chats').close());
$('pairForm').onsubmit=async e=>{e.preventDefault();try{
  const r=await fetch('/pair',{method:'POST',body:$('code').value.trim()});if(!r.ok)throw Error('Pairing code did not match.');
  $('code').value=''; $('pair').close(); $('notice').textContent=''; await refresh();
}catch(e){$('pairError').textContent=e.message;}};
function leave() {
  const v=voice || starting; if(!v)return;
  cleanup(v); navigator.sendBeacon('/api',JSON.stringify({action:'stop',chat,session:v.id}));
}
addEventListener('pagehide',leave);
document.addEventListener('visibilitychange',()=>{if(document.hidden)leave();});
(async function observe(){if(!document.hidden && !$('pair').open){try{await refresh();}catch(e){notice(e,true);}}
  setTimeout(observe,voice || snapshot?.voice.phase==='thinking' ? 450 : 1400);
})();
