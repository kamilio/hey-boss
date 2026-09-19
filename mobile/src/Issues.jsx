import React,{useState,useEffect,useRef} from 'react';
import {Button,TextField,TextArea} from '@radix-ui/themes';
import {emptyIssueDraft,loadIssueDraft,saveIssueDraft,issuePayload} from './issue-draft';

export default function Issues({api}){
 const [draft,setDraft]=useState(()=>loadIssueDraft(localStorage));
 const [state,setState]=useState({projects:[],creations:[],connected:false});
 const [error,setError]=useState(''),[message,setMessage]=useState(''),[busy,setBusy]=useState(false);
 const current=useRef(draft),running=useRef(false),mounted=useRef(true);
 function keep(next){
  // Persist before sending; storage failure must never discard the retry key.
  saveIssueDraft(localStorage,next);current.current=next;if(mounted.current)setDraft(next);
 }
 function change(field,value){
  const next={...current.current,[field]:value,requestID:crypto.randomUUID(),submitted:false};
  current.current=next;setDraft(next);setError('');
  try{saveIssueDraft(localStorage,next);}catch{setError('Could not save this draft. Free some browser storage before submitting.');}
 }
 async function refresh(){
  if(running.current)return;running.current=true;
  try{
   if(current.current.submitted){
    if(mounted.current)setBusy(true);
    try{
     const {creation}=await api('/issues',issuePayload(current.current));
     if(creation.status==='error'){
      keep({...current.current,submitted:false,requestID:crypto.randomUUID()});
      if(mounted.current)setError(creation.error);
     }else{
      keep({...emptyIssueDraft(),project:current.current.project});
      if(mounted.current){setError('');setMessage(creation.status==='synced'?`Synced as issue #${creation.number}`:'Pending — saved on the server. It will sync when the supervisor reconnects.');}
     }
    }catch(e){
     if([400,409].includes(e.status)){
      keep({...current.current,submitted:false,requestID:crypto.randomUUID()});
      if(mounted.current)setError(e.message);
     }else if(mounted.current)setError(e.status===401?'Pair this device again to send the saved draft.':e.status?e.message:'Pending — saved on this phone. Retrying when connected.');
    }
   }
   const value=await api('/issues');if(mounted.current)setState(value);
  }catch(e){if(mounted.current)setError(e.status?e.message:'Unable to refresh issues. Your draft is saved; retry when online.');}
  finally{running.current=false;if(mounted.current)setBusy(false);}
 }
 useEffect(()=>{
  mounted.current=true;refresh();const timer=setInterval(()=>{if(!document.hidden)refresh();},5000);
  const visible=()=>{if(!document.hidden)refresh();};
  window.addEventListener('online',refresh);document.addEventListener('visibilitychange',visible);
  return()=>{mounted.current=false;clearInterval(timer);window.removeEventListener('online',refresh);document.removeEventListener('visibilitychange',visible);};
 },[]);
 async function submit(event){
  event.preventDefault();if(running.current)return;setError('');setMessage('');
  try{keep({...current.current,submitted:true});await refresh();}
  catch{setError('Could not save this draft. Free some browser storage before submitting.');}
 }
 async function restore(summary){
  if(current.current.submitted)return;
  try{
   const {creation}=await api('/issues/'+encodeURIComponent(summary.requestID));
   if(current.current.submitted)return;
   keep({...emptyIssueDraft(),project:creation.project,title:creation.title,body:creation.body,labels:creation.labels.join(', ')});
   setError('');setMessage('Draft restored. Edit it and submit again.');
  }catch(e){setError(e.status?e.message:'Could not restore the draft. Check your connection and browser storage, then retry.');}
 }
 return <section className="issues-page">
  <form className="issue-form glass" onSubmit={submit}>
   <h2>Create an issue</h2>
   <label htmlFor="issue-project">Project</label>
   <select id="issue-project" required value={draft.project} disabled={draft.submitted} onChange={e=>change('project',e.target.value)}>
    <option value="">Select a project</option>
    {draft.project&&!state.projects.some(p=>p.id===draft.project)&&<option value={draft.project}>{draft.project}</option>}
    {state.projects.map(project=><option key={project.id} value={project.id}>{project.name}</option>)}
   </select>
   {!state.projects.length&&<p className="fine">Waiting for registered projects. Connect the supervisor to load them.</p>}
   <label htmlFor="issue-title">Title</label>
   <TextField.Root id="issue-title" required value={draft.title} disabled={draft.submitted} onChange={e=>change('title',e.target.value)} placeholder="What needs to happen?" size="3"/>
   <label htmlFor="issue-description">Description <span className="fine">optional</span></label>
   <TextArea id="issue-description" value={draft.body} disabled={draft.submitted} onChange={e=>change('body',e.target.value)} placeholder="Details for the agent" rows={4}/>
   <label htmlFor="issue-labels">Labels <span className="fine">optional, separated by commas</span></label>
   <TextField.Root id="issue-labels" value={draft.labels} disabled={draft.submitted} onChange={e=>change('labels',e.target.value)} placeholder="ready, bug"/>
   <Button size="3" loading={busy} disabled={!draft.title.trim()||!draft.project} type="submit">{draft.submitted?'Retry pending submission':'Create issue'}</Button>
   {draft.submitted&&<p role="status" className="fine">Pending — this draft is saved on your phone. It retries with the same request ID.</p>}
   {error&&<p className="reader-error" role="alert">{error}</p>}
   {message&&<p className="fine" role="status">{message}</p>}
  </form>
  <div className="issue-deliveries"><h2>Your submissions</h2><p className="fine">{state.connected?'Supervisor connected':'Supervisor offline — accepted issues stay queued on the server.'}</p>
   {!state.creations.length&&<p className="fine">Your submitted issues will appear here.</p>}
   {state.creations.map(creation=><article className="issue-delivery glass" key={creation.requestID}>
    <span className="fine">{state.projects.find(p=>p.id===creation.project)?.name||creation.project}</span><h3>{creation.title}</h3>
    <p role="status">{creation.status==='synced'?`Synced · #${creation.number}`:creation.status==='error'?'Error':'Pending — waiting for the supervisor'}</p>
    {creation.status==='error'&&<><p className="reader-error">{creation.error}</p><Button variant="soft" disabled={draft.submitted} onClick={()=>restore(creation)}>Edit saved draft</Button></>}
   </article>)}
  </div>
 </section>;
}
