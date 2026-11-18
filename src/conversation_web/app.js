'use strict';
const $ = id => document.getElementById(id);
let chat, voice, starting, snapshot, after, refreshCount = 0, rendered, before;
const drafts = new Map();
const notice = e => { $('notice').textContent = e.message || String(e); };
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
  const el = $(id); el.hidden = !available; if (label) el.textContent = label;
}
async function refresh() {
  const sequence = ++refreshCount;
  const s = await api('state',{revision:rendered,before});
  if (sequence !== refreshCount) return;
  if (chat && chat !== s.chat) {
    if ($('draft').value) drafts.set(chat,$('draft').value); else drafts.delete(chat);
    $('draft').value=drafts.get(s.chat) || ''; drafts.delete(s.chat);
    if (before!=null) { before=null; rendered=null; chat=s.chat; return refresh(); }
  }
  chat = s.chat; snapshot = s;
  const v = s.voice, mine = !!voice && s.mine;
  $('status').textContent = v.approval ? (mine ? 'Approval needed' : 'Approve on your Mac') : v.phase === 'idle' ? (v.detail || 'Ready') : v.message;
  $('talk').textContent = mine ? 'End voice' : 'Start voice';
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
  $('caption').replaceChildren();
  if (s.passage) {
    const heard = document.createElement('b'); heard.textContent = s.passage.slice(0,s.spoken);
    $('caption').append(heard,s.passage.slice(s.spoken));
  }
  $('approval').hidden = !mine || !s.approval;
  if (s.approval) $('approval').querySelector('pre').textContent=s.approval.text;
  if (s.messages) {
    button('earlier', s.start>0); button('latest', before!=null);
    const timeline = $('timeline'), follow = !rendered || timeline.scrollHeight-timeline.scrollTop-timeline.clientHeight < 70;
    $('empty').hidden = s.messages.length > 0;
    const nodes = [];
    const inserts = at => {
      for (const tool of s.tools.filter(t=>t.at===at)) {
        const row=document.createElement('details'), label=document.createElement('summary'), detail=document.createElement('pre');
        label.textContent=tool.label+' · '+tool.state; detail.textContent=tool.detail; row.append(label,detail); nodes.push(row);
      }
      for (const visual of s.visuals.filter(v=>v.at===at)) {
        const b=document.createElement('button'); b.textContent=visual.title || 'Show image';
        b.onclick=async()=>{try{const r=await api('image',{media:visual.id});const img=document.createElement('img');
          img.alt=b.textContent;img.src='data:image/png;base64,'+r.png;b.replaceWith(img);}catch(e){notice(e);}};
        nodes.push(b);
      }
    };
    inserts(s.start);
    for (const [i,msg] of s.messages.entries()) {
      const row = document.createElement('article'), name = document.createElement('small'), body = document.createElement('div');
      if (msg.speaker === 'You') row.className = 'user';
      name.textContent = msg.speaker; if(msg.html) body.innerHTML=msg.html; else body.textContent = msg.text;
      row.append(name,body); nodes.push(row); inserts(s.start+i+1);
    }
    $('messages').replaceChildren(...nodes); rendered = s.revision;
    if (before!=null) timeline.scrollTop=0; else if (follow) timeline.scrollTop = timeline.scrollHeight;
  }
}
function cleanup(v) {
  if (!v || v.closed) return;
  v.closed = true; clearTimeout(v.timer); v.track?.getTracks().forEach(t => t.stop());
  if (v.ctx.state !== 'closed') v.ctx.close().catch(()=>{});
  if (voice === v) voice = null;
  if (starting === v) starting = null;
  $('talk').disabled = false; $('talk').textContent = 'Start voice';
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
    for (const s of v.segments) {
      const elapsed = Math.floor((v.ctx.currentTime-s.at)*s.rate);
      if (elapsed < 0) break;
      v.played = Math.max(v.played,s.offset+Math.min(elapsed,s.length));
    }
    v.segments = v.segments.filter(s => s.offset+s.length > v.played);
    const chunks = v.input.splice(0);
    if (chunks.length > 6) throw Error('Microphone connection fell behind. Start voice again.');
    const bytes = new Uint8Array(chunks.length*2560);
    chunks.forEach((c,i)=>bytes.set(new Uint8Array(c),i*2560));
    const r = await api('exchange',{session:v.id,generation:v.gen,played:v.played,received:v.received,
      pcm:bytes.length ? btoa(String.fromCharCode(...bytes)) : undefined});
    if (!v.closed) { play(v,r); v.timer = setTimeout(()=>pump(v),65); }
  } catch(e) { if (!v.closed) { stop(v).catch(()=>{}); notice(e); } }
}
async function start() {
  if (starting || voice) return;
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
    mic.port.onmessage = e => { if (voice === v && v.track.getAudioTracks()[0].enabled && v.input.length < 7) v.input.push(e.data); };
    v.track.getAudioTracks()[0].onended = () => { if (!v.closed) { stop(v).catch(()=>{}); notice(Error('Microphone disconnected.')); } };
    if (v.closed) return;
    await api('start',{session:v.id});
    if (v.closed) { await api('stop',{session:v.id}); return; }
    voice = v; starting = null; pump(v); await refresh();
  } catch(e) { cleanup(v); throw e; }
}
const act = (id, fn) => { $(id).onclick = () => { $('notice').textContent=''; Promise.resolve().then(fn).catch(notice); }; };
// Start the AudioContext synchronously in the Safari user gesture.
$('talk').onclick = () => (voice ? stop(voice).then(refresh) : start()).catch(notice);
for (const id of ['mute','pause','interrupt','finish']) act(id,async()=>{await api(id); await refresh();});
act('earlier',async()=>{before=snapshot.start;rendered=null;await refresh();});
act('latest',async()=>{before=null;rendered=null;await refresh();});
for (const action of ['allow','decline']) act(action,async()=>{await api(action,{text:snapshot.approval.id});await refresh();});
act('fresh',async()=>{ if (voice) await stop(voice); await api('new'); before=null; rendered=null; await refresh(); });
$('composer').onsubmit = async e => { e.preventDefault(); const text = $('draft').value.trim(); if (!text) return;
  const submit=e.submitter || $('composer').querySelector('button'); if (submit.disabled) return; submit.disabled=true;
  try { await api('send',{text}); if ($('draft').value.trim()===text) $('draft').value=''; await refresh(); }
  catch(e){notice(e);} finally{submit.disabled=false;}
};
async function list() {
  const data=await api('list',{after}); after=data.next;
  for (const c of data.chats) {
    const b=document.createElement('button'); b.textContent=c.title;
    b.onclick=async()=>{try{if(voice)await stop(voice);await api('open',{chat:c.id});$('chats').close();before=null;rendered=null;await refresh();}catch(e){$('chats').close();notice(e);}};
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
(async function observe(){if(!document.hidden && !$('pair').open){try{await refresh();}catch(e){notice(e);}}
  setTimeout(observe,voice || snapshot?.voice.phase==='thinking' ? 450 : 1400);
})();
