import React,{useState,useEffect,useCallback,useRef} from 'react';
import {createRoot} from 'react-dom/client';
import {Theme,Button,TextArea,TextField,Spinner,Dialog} from '@radix-ui/themes';
import {Inbox,History,Settings,Bell,MessageCircle,Check,CheckCheck,TriangleAlert,CircleAlert,Info,ArrowUpRight,ChevronRight,X,Monitor,Server,RefreshCw} from 'lucide-react';
import Markdown from './Markdown';
import usePullRefresh from './usePullRefresh';
import LiquidGlass from 'liquid-glass-react';
import {activityGroups,activityTime,activityDateTime} from './activity';
import '@radix-ui/themes/styles.css';
import './style.css';

async function api(path,body){
 const controller=new AbortController();const deadline=setTimeout(()=>controller.abort(),10000);
 try{const response=await fetch('/api'+path,{method:body===undefined?'GET':'POST',headers:body===undefined?{}:{'Content-Type':'application/json'},body:body===undefined?undefined:JSON.stringify(body),signal:controller.signal});
 const value=await response.json();if(!response.ok){const e=Error(value.error||'Could not complete request');e.status=response.status;throw e;}return value;}finally{clearTimeout(deadline);}
}
const decision=task=>['approval','prompt'].includes(task.kind);
const safeLink=url=>/^https?:\/\//i.test(url??'');
const preferredAppearance=()=>window.matchMedia('(prefers-color-scheme: dark)').matches?'dark':'light';
function savedDrafts(){try{const value=JSON.parse(localStorage.getItem('hey-boss-drafts')||'{}');return Object.fromEntries(Object.entries(value).filter(([id,text])=>id.length<=128&&typeof text==='string'&&text.length<=16384));}catch{return {};}}
function TaskIcon({task,size=20}){
 const Icon=task.severity==='error'?CircleAlert:task.severity==='warning'?TriangleAlert:decision(task)?MessageCircle:task.severity==='success'?CheckCheck:task.kind==='update'?Info:Bell;
 return <span className={'task-icon '+(task.severity||'info')}><Icon size={size} strokeWidth={1.8}/></span>;
}
function App(){
 const [appearance,setAppearance]=useState(preferredAppearance);
 useEffect(()=>{const media=window.matchMedia('(prefers-color-scheme: dark)');const update=()=>setAppearance(preferredAppearance());media.addEventListener('change',update);return()=>media.removeEventListener('change',update);},[]);
 const [state,setState]=useState(null),[paired,setPaired]=useState(null),[tab,setTab]=useState('inbox'),[code,setCode]=useState('');
 const [error,setError]=useState(''),[notice,setNotice]=useState(''),[busy,setBusy]=useState(false),[pushBusy,setPushBusy]=useState(false),[settingsBusy,setSettingsBusy]=useState(false);
 const [drafts,setDrafts]=useState(savedDrafts),[sending,setSending]=useState(null),[selectedID,setSelectedID]=useState(null),[detail,setDetail]=useState(null),[detailLoading,setDetailLoading]=useState(false),[detailError,setDetailError]=useState('');
 useEffect(()=>{try{localStorage.setItem('hey-boss-drafts',JSON.stringify(drafts));}catch{}},[drafts]);
 useEffect(()=>{if(!state)return;const handled=new Set(state.tasks.filter(t=>t.status!=='pending').map(t=>t.taskID));setDrafts(current=>{if(!Object.keys(current).some(id=>handled.has(id)))return current;return Object.fromEntries(Object.entries(current).filter(([id])=>!handled.has(id)));});},[state]);
 const deepLink=useRef(null),detailGeneration=useRef(0),refreshing=useRef(null);
 const [connectionError,setConnectionError]=useState('');
 const refresh=useCallback(()=>refreshing.current??=(async()=>{
  try{
   await HeyBossReceipts.flush().catch(()=>{});
   const value=await api('/tasks');setState(value);setPaired(true);setConnectionError('');
   if('setAppBadge'in navigator)navigator.setAppBadge(value.tasks.filter(t=>t.status==='pending').length).catch(()=>{});
   if('serviceWorker'in navigator)navigator.serviceWorker.ready.then(async registration=>{
    for(const n of await registration.getNotifications())if(value.tasks.some(t=>t.taskID===n.tag&&t.status!=='pending')||n.data?.taskIDs?.length&&n.data.taskIDs.every(id=>value.tasks.some(t=>t.taskID===id&&t.status!=='pending')))n.close();
   }).catch(()=>{});
  }catch(e){if(e.status===401)setPaired(false);else setConnectionError('Unable to connect. Your requests are safe; retry when online.');}
 })().finally(()=>{refreshing.current=null;}),[]);
 const shellRef=useRef(null);
 const pull=usePullRefresh(shellRef,paired===true&&selectedID===null&&tab!=='settings',refresh);
 useEffect(()=>{
  refresh();const timer=setInterval(()=>{if(!document.hidden)refresh();},15000);
  const visible=()=>{if(!document.hidden)refresh();};document.addEventListener('visibilitychange',visible);window.addEventListener('online',refresh);window.addEventListener('pageshow',refresh);
  return()=>{clearInterval(timer);document.removeEventListener('visibilitychange',visible);window.removeEventListener('online',refresh);window.removeEventListener('pageshow',refresh);};
 },[refresh]);
 useEffect(()=>{if(!paired)return;const stream=new EventSource('/api/events');stream.onmessage=()=>{if(!document.hidden)refresh();};return()=>stream.close();},[paired,refresh]);
 useEffect(()=>{if('serviceWorker'in navigator)navigator.serviceWorker.register('/sw.js').catch(()=>setError('Notifications require HTTPS.'));},[]);
 async function loadDetail(id){
  const generation=++detailGeneration.current;setDetailLoading(true);setDetailError('');
  try{const {task}=await api('/tasks/'+encodeURIComponent(id));if(generation===detailGeneration.current)setDetail(task);}
  catch(e){if(generation===detailGeneration.current)setDetailError(e.message);}
  finally{if(generation===detailGeneration.current)setDetailLoading(false);}
 }
 async function markOpened(task){
  if(decision(task))return;
  try{await HeyBossReceipts.open(task.taskID);await refresh();}
  catch{setNotice('Opened here. Read receipt will sync when connected.');}
 }
 async function openTask(task){setSelectedID(task.taskID);setDetail(null);loadDetail(task.taskID);await markOpened(task);}
 useEffect(()=>{
  if(!paired)return;const id=new URLSearchParams(location.search).get('task');
  if(!id||id==='inbox'||deepLink.current===id)return;
  deepLink.current=id;setSelectedID(id);setDetail(null);loadDetail(id);
  HeyBossReceipts.open(id).then(refresh).catch(()=>setNotice('Read receipt will sync when connected.'));
 },[paired,refresh]);
 const selectedStatus=state?.tasks.find(t=>t.taskID===selectedID);
 const selected=detail?{...detail,...(selectedStatus?{status:selectedStatus.status,result:selectedStatus.result,handledBy:selectedStatus.handledBy}:{} )}:null;
 async function pair(event){event.preventDefault();if(busy)return;setBusy(true);setError('');try{await api('/pair',{code:code.trim()});await refresh();setCode('');}catch(e){setError(e.message);}finally{setBusy(false);}}
 async function respond(task,result){
  if(sending)return;setSending(task.taskID);setError('');
  try{await api('/tasks/'+encodeURIComponent(task.taskID)+'/resolve',{result});setDrafts(d=>{const n={...d};delete n[task.taskID];return n;});await refresh();if(selectedID===task.taskID)await loadDetail(task.taskID);}
  catch(e){setError(e.message);if(e.status===409)await refresh();}finally{setSending(null);}
 }
 async function enablePush(){
  setPushBusy(true);setError('');try{
   if(!('PushManager'in window))throw Error('Add Hey Boss to your iPhone Home Screen, then open it there.');
   const permission=await Notification.requestPermission();if(permission!=='granted')throw Error('Allow Hey Boss notifications in iPhone Settings.');
   const registration=await navigator.serviceWorker.ready;let subscription=await registration.pushManager.getSubscription();
   if(!subscription){const key=state.vapidPublicKey?.replace(/-/g,'+').replace(/_/g,'/');if(!key)throw Error('Push is not configured yet.');subscription=await registration.pushManager.subscribe({userVisibleOnly:true,applicationServerKey:Uint8Array.from(atob(key),c=>c.charCodeAt(0))});}
   await api('/subscribe',subscription.toJSON());setNotice('Notifications enabled');await refresh();
  }catch(e){setError(e.message);}finally{setPushBusy(false);}
 }
 async function savePreferences(changes){
  setSettingsBusy(true);setError('');try{const value=await api('/notifications',{...state.notifications,...changes});setState(s=>({...s,notifications:value.notifications}));}catch(e){setError(e.message);}finally{setSettingsBusy(false);}
 }
 const pending=(state?.tasks??[]).filter(t=>t.status==='pending').sort((a,b)=>({approval:0,prompt:1,alert:2,update:2}[a.kind]-{approval:0,prompt:1,alert:2,update:2}[b.kind]));
 const history=(state?.tasks??[]).filter(t=>t.status!=='pending');
 const standalone=window.matchMedia('(display-mode: standalone)').matches||navigator.standalone;
 const safari=/Safari/.test(navigator.userAgent)&&!/Chrome/.test(navigator.userAgent);
 const glassNav=children=>safari||window.matchMedia('(prefers-reduced-motion: reduce)').matches?<div className="nav-glass glass">{children}</div>:<LiquidGlass className="liquid-nav" padding="0" displacementScale={45} blurAmount={0.1} saturation={140} elasticity={0} cornerRadius={32} style={{position:'relative',top:'auto',left:'auto',transform:'none'}}><div className="nav-glass">{children}</div></LiquidGlass>;
 function responses(task){return task.status!=='pending'?<div className="result-panel"><Check size={18}/><div><strong>{task.status==='cancelled'?'Dismissed':task.result||'Read'}</strong><small>{decision(task)?'Answered':'Read'} on {task.handledBy==='mac'?'Mac':'iPhone'}</small></div></div>:decision(task)?<div className="response">{task.kind==='prompt'?<><TextArea aria-label="Your answer" placeholder="Your answer…" value={drafts[task.taskID]||''} onChange={e=>setDrafts({...drafts,[task.taskID]:e.target.value})} rows={3} onKeyDown={e=>{if((e.metaKey||e.ctrlKey)&&e.key==='Enter'&&drafts[task.taskID]?.trim()){e.preventDefault();respond(task,drafts[task.taskID]);}}}/><Button size="3" disabled={sending!==null||!drafts[task.taskID]?.trim()} loading={sending===task.taskID} onClick={()=>respond(task,drafts[task.taskID])}>Send answer <ArrowUpRight size={16}/></Button></>:<div className="choices">{task.options.map((option,i)=><Button key={option} size="3" variant={i===0?'solid':'soft'} disabled={sending!==null} loading={sending===task.taskID&&i===0} onClick={()=>respond(task,option)}>{option}</Button>)}</div>}</div>:null;}
 function link(task){return safeLink(task.linkURL)&&<Button asChild size="3"><a href={task.linkURL} target="_blank" rel="noreferrer" onClick={()=>markOpened(task)}>{task.linkLabel||'Open link'} <ArrowUpRight size={16}/></a></Button>;}
 const macState=state?.notifications?.macState,macIdle=state?.notifications?.macIdleSeconds;
 const activityLabel=macState==='away'?'Mac away':macState==='confirming'?'Confirming away':macState==='locked'?'Mac locked':macState==='offline'?'Mac offline':macState==='unknown'?'Checking Mac':macIdle==null?'Mac connected':macIdle>=60?`Idle ${Math.floor(macIdle/60)}m`:'Mac active';
 function closeReader(){setSelectedID(null);setDetail(null);detailGeneration.current++;deepLink.current=null;const url=new URL(location.href);url.searchParams.delete('task');url.searchParams.delete('opened');window.history.replaceState(null,'',url);}
 return <Theme appearance={appearance} accentColor="indigo" grayColor="slate" radius="large"><div className="shell" ref={shellRef}>
  <header><a className="brand" href="/"><img src="/icons/icon-192.png" alt=""/>Hey Boss</a><div className="header-actions"><span className="host-state"><span className={'status-dot '+(macState==='active'?'on':'')}/>{activityLabel}</span>{paired&&<button className="settings-button" aria-label="Settings" aria-pressed={tab==='settings'} onClick={()=>setTab(tab==='settings'?'inbox':'settings')}><Settings size={20}/></button>}</div></header>
  <div className={'pull-refresh '+(pull.loading?'is-refreshing':'')} style={{height:pull.distance}} role="status" aria-live="polite">{pull.distance>0&&<span><RefreshCw size={18} style={{transform:pull.loading?undefined:`rotate(${pull.distance*3}deg)`}}/>{pull.loading?'Refreshing…':pull.ready?'Release to refresh':'Pull to refresh'}</span>}</div>
  <main>
   {paired===null?<div className="loading"><Spinner size="3"/><p>Connecting…</p></div>:!paired?<section className="pair-card glass"><MessageCircle size={30}/><h1>Connect your iPhone</h1><p>Updates and decisions, synced with your Mac.</p><form onSubmit={pair}><label htmlFor="pair-code">Pairing code</label><TextField.Root id="pair-code" value={code} onChange={e=>setCode(e.target.value)} placeholder="Enter code from your Mac" autoComplete="off" autoCapitalize="characters" size="3"/><Button size="3" loading={busy} disabled={!code.trim()} type="submit">Connect <ArrowUpRight size={16}/></Button></form>{!standalone&&<p className="install-note">Share → Add to Home Screen to enable notifications.</p>}</section>:<>
   <div className="page-title"><h1>{tab==='inbox'?'Inbox':tab==='history'?'Activity':'Settings'}</h1>{tab!=='settings'&&<div className="title-actions">{tab==='inbox'&&pending.length>0&&<span className="count">{pending.length}</span>}<button className="refresh-button" aria-label="Refresh inbox" disabled={pull.loading} onClick={pull.run}><RefreshCw size={18}/></button></div>}</div>
   {tab==='settings'?<section className="settings glass">
    <div className="settings-row"><span className="setting-label"><Bell size={18}/>Notifications<small>{state.pushEnabled?'Enabled':'Not enabled'}</small></span><Button size="2" variant="soft" loading={pushBusy} onClick={enablePush}>{state.pushEnabled?'Check':'Enable'}</Button></div>
    <div className="settings-block"><label id="routing-label">Send notifications</label><div className="segmented" role="group" aria-labelledby="routing-label">{[['automatic','When away'],['always','Always'],['off','Off']].map(([value,label])=><button key={value} disabled={settingsBusy} aria-pressed={(state.notifications?.mode||'automatic')===value} onClick={()=>savePreferences({mode:value})}>{label}</button>)}</div><p className="fine">{state.notifications?.mode==='off'?'Updates still appear in your inbox.':state.notifications?.mode==='always'?'Notify this iPhone even while you use your Mac.':state.notifications?.macState==='unknown'?'Waiting for Mac status. iPhone push stays paused.':state.notifications?.notifyPhone?'You’re away from your Mac. iPhone push is active.':'Your Mac is available. iPhone push is paused.'}</p></div>
    <details className="settings-help"><summary>Connection details</summary><p>Opening an update marks it read on both devices. Questions close when you answer. Offline read receipts retry automatically.</p><p>iOS may retain a delivered Lock Screen notification until the app opens or another push arrives.</p><p>Automatic pushes require {Math.round((state.notifications?.awayAfterSeconds||600)/60)} minutes without mouse or keyboard input, confirmed for another minute. Unreliable readings stay quiet. A two-minute disconnect also enables phone pushes.</p><p>Mac activity: {macIdle==null?'idle reading unavailable':`last input ${Math.floor(macIdle)} seconds ago`}. Only aggregate inactivity is sampled; no keys, app contents or screen recording.</p><Button variant="soft" color="red" onClick={async()=>{await api('/logout',{});setDrafts({});setState(null);setPaired(false);setTab('inbox');}}>Disconnect this iPhone</Button></details>
   </section>:tab==='history'?<section className="activity-list">{history.length===0?<div className="empty glass"><History size={28}/><h2>No activity yet</h2><p>Read updates and answers will appear here.</p></div>:activityGroups(history).map(group=><section key={group.label}><h2 className="activity-date">{group.label}</h2><div className="activity glass">{group.tasks.map(task=><button className="activity-row" key={task.taskID} onClick={()=>openTask(task)}><TaskIcon task={task} size={17}/><div><strong>{task.title}</strong><span>{task.project} · {task.status==='cancelled'?'Dismissed':decision(task)?task.result:'Read'} · {task.handledBy==='mac'?'Mac':'iPhone'}</span></div><div className="activity-end"><time dateTime={activityDateTime(task)}>{activityTime(task)}</time><ChevronRight size={17}/></div></button>)}</div></section>)}</section>:<div className="task-list">
    {pending.length===0?<section className="empty glass"><span className="empty-symbol"><Check size={28}/></span><h2>You’re up to date</h2>{!state.pushEnabled&&<Button variant="soft" onClick={()=>setTab('settings')}>Enable notifications</Button>}</section>:pending.map(task=><article className={'task-card glass '+(task.severity||'info')} key={task.taskID}>
     <div className="task-meta"><span>{task.project||'Workspace'}</span><span className="host-badge">{task.sourceHost==='This Mac'||task.sourceHost==='This MacBook'?<Monitor size={12}/>:<Server size={12}/>} {task.sourceHost==='This Mac'||task.sourceHost==='This MacBook'?'Local':task.sourceHost||'Mac'}</span></div>
     <button className="task-open" onClick={()=>openTask(task)}><TaskIcon task={task}/><h2>{task.title}</h2><ChevronRight size={19}/></button>
     <p className="card-preview">{task.kind==='update'?task.description||task.question:task.question||task.description}</p>
     {task.kind==='prompt'?<div className="response"><Button variant="soft" size="3" onClick={()=>openTask(task)}>Answer <MessageCircle size={16}/></Button></div>:responses(task)}{!decision(task)&&safeLink(task.linkURL)&&<div className="notification-actions">{link(task)}</div>}
    </article>)}
   </div>}
   </>}
   {(error||connectionError)&&<div className="message error" role="alert"><CircleAlert size={18}/><span>{error||connectionError}</span><button aria-label="Dismiss error" onClick={()=>{setError('');setConnectionError('');}}><X size={18}/></button></div>}
   {notice&&!error&&!connectionError&&<div className="message" role="status"><span>{notice}</span><button aria-label="Dismiss message" onClick={()=>setNotice('')}><X size={18}/></button></div>}
  </main>
  {paired&&<nav aria-label="Main navigation">{glassNav(<div className="tabs">{[['inbox','Inbox',Inbox],['history','Activity',History]].map(([value,label,Icon])=><button key={value} className={tab===value?'selected':''} aria-current={tab===value?'page':undefined} onClick={()=>{setTab(value);setNotice('');}}><Icon size={21} strokeWidth={1.8}/><span>{label}</span>{value==='inbox'&&pending.length>0&&<b>{pending.length}</b>}</button>)}</div>)}</nav>}
  <Dialog.Root open={selectedID!==null} onOpenChange={open=>{if(!open)closeReader();}}><Dialog.Content className="reader-dialog" aria-describedby={undefined} onOpenAutoFocus={event=>{event.preventDefault();document.getElementById('reader-heading')?.focus();}}>
   <div className="reader-top"><span className="reader-context">{selected?.project||'Update'}</span><Dialog.Close><button className="icon-button" aria-label="Close reader"><X size={20}/></button></Dialog.Close></div>
   <Dialog.Title id="reader-heading" tabIndex={-1}>{selected?.title||state?.tasks.find(t=>t.taskID===selectedID)?.title||'Loading update…'}</Dialog.Title>
   {detailLoading?<div className="reader-loading"><Spinner size="3"/></div>:detailError?<p role="alert">{detailError} <Button variant="soft" onClick={()=>loadDetail(selectedID)}>Retry</Button></p>:selected&&<>
    <div className="reader-meta"><TaskIcon task={selected} size={16}/><span>{selected.sourceHost||'Mac'}</span>{!decision(selected)&&<span className="read-state"><Check size={13}/>{selected.status==='pending'?'Syncing read receipt':'Read'}</span>}</div>
    {selected.kind==='update'?<>{selected.description&&<Markdown>{selected.description}</Markdown>}{selected.question&&selected.question!==selected.description&&<Markdown>{selected.question}</Markdown>}</>:<>{selected.question&&<Markdown>{selected.question}</Markdown>}{selected.description&&selected.description!==selected.question&&<Markdown>{selected.description}</Markdown>}</>}
    {decision(selected)?responses(selected):<div className="reader-actions">{link(selected)}</div>}
   </>}
   {error&&<p className="reader-error" role="alert">{error}</p>}
  </Dialog.Content></Dialog.Root>
 </div></Theme>;
}
class AppBoundary extends React.Component{
 state={failed:false};static getDerivedStateFromError(){return {failed:true};}
 render(){return this.state.failed?<Theme appearance={preferredAppearance()}><div className="shell"><main className="empty glass recovery"><CircleAlert size={28}/><h2>Couldn’t display this view</h2><p>Your saved requests are safe.</p><Button onClick={()=>location.reload()}>Reload</Button></main></div></Theme>:this.props.children;}
}
createRoot(document.getElementById('root')).render(<AppBoundary><App/></AppBoundary>);
