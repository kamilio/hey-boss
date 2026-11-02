"use strict";
// Runtime membership comes from the process snapshot, never saved pickup settings.
function fleetView(data, now = Date.now()) {
  const live = [], saved = [], offline = [];
  const machines = data.machines || [];
  for (const machine of machines) {
    if (machine.state !== "connected" || now / 1000 - (machine.heartbeat || 0) > 15) {
      offline.push(machine);
      continue;
    }
    for (const worker of machine.workers || []) {
      (worker.pid > 0 ? live : saved).push({machine, worker});
    }
  }
  return {live, saved, offline,
    supervisor: machines.find(m => ["supervisor", "controller"].includes(m.role)),
    active: live.reduce((n, {worker}) => n + (worker.runs || []).filter(r => r.finished_at == null).length, 0),
    capacity: live.reduce((n, {worker}) => n + (worker.config?.concurrency || 1), 0)};
}
function elapsed(run, now = Date.now()) {
  if(!Number.isFinite(run.started_at))return '';
  const seconds = Math.max(0, Math.floor(((run.finished_at ?? now) - run.started_at) / 1000));
  const pad=n=>String(n).padStart(2,'0');
  if(seconds<60)return seconds+'s';
  if(seconds<3600)return Math.floor(seconds/60)+'m '+pad(seconds%60)+'s';
  if(seconds<86400)return Math.floor(seconds/3600)+'h '+pad(Math.floor(seconds/60)%60)+'m';
  return Math.floor(seconds/86400)+'d '+pad(Math.floor(seconds/3600)%24)+'h';
}
// An agent belongs to its task's project, regardless of its machine or scheduler.
function projectView(data, now = Date.now()) {
  const groups = new Map();
  for (const machine of data.machines || []) {
    const online = machine.state === 'connected' && now / 1000 - (machine.heartbeat || 0) <= 15;
    for (const worker of machine.workers || []) for (const run of worker.runs || []) {
      if (run.kind === 'chief') continue;
      const id = run.project_id;
      if (!groups.has(id)) groups.set(id, {id, name: run.project_name || id || 'Project', active: [], history: []});
      const entry = {run, machine, worker, online: online && worker.pid > 0};
      groups.get(id)[run.finished_at == null ? 'active' : 'history'].push(entry);
    }
  }
  return [...groups.values()].sort((a,b) => Number(b.active.length > 0) - Number(a.active.length > 0) || a.name.localeCompare(b.name));
}
function infrastructureLabel(run) {
  return ['GitHub quota exhausted', 'GitHub authentication failed', 'GitHub permission denied', 'GitHub request failed', 'Worker environment check failed', 'Database service unavailable', 'Model proxy unavailable', 'Approval service unavailable'].find(label => (run.summary || '').startsWith(label)) || 'Infrastructure unavailable';
}
function infrastructureGuidance(run) {
  const label = infrastructureLabel(run);
  if (label === 'GitHub quota exhausted') {
    const retry = run.retry_at != null ? 'Automatic retry at ' + new Date(run.retry_at).toLocaleString(undefined,{month:'short',day:'numeric',hour:'numeric',minute:'2-digit',second:'2-digit',timeZoneName:'short'}) + '.' : 'The attempt has ended; its saved session is retained.';
    return 'GitHub quota exhausted. ' + retry + ' Its slot and claim are released; saved work is retained.';
  }
  if (label.startsWith('GitHub ') || label === 'Worker environment check failed' || label === 'Infrastructure unavailable') return run.summary || 'This attempt ended because a required service was unavailable. Its saved session is retained.';
  const service = label === 'Database service unavailable' ? 'database service' : label === 'Model proxy unavailable' ? 'model proxy' : 'approval service';
  return 'The ' + service + ' failed during this attempt. ' + (run.retry_at != null ? 'The saved session will retry automatically.' : 'The attempt has ended; its saved session is retained.') + (service === 'database service' ? ' Read the current issue state before repeating any uncertain write.' : '');
}
function retryLabel(run, now = Date.now()) {
  const seconds = Math.max(0, Math.ceil((run.retry_at - now) / 1000));
  return seconds ? 'Retry in ' + (seconds < 60 ? seconds + 's' : Math.ceil(seconds / 60) + 'm') : 'Waiting to retry';
}
function agentState(entry) {
  if (entry.run.state === 'attempt_held') return 'Task attempt protected';
  if (!entry.online && entry.run.finished_at == null) return 'Last seen';
  if (entry.run.retry_at != null) return retryLabel(entry.run);
  if (entry.run.state === 'infrastructure_blocked') return infrastructureLabel(entry.run);
  return ({running:'Working',reserved:'Starting',starting:'Starting',completed:'Completed',blocked:'Needs attention',needs_input:'Needs your answer',approval_required:'Needs approval',interrupted:'Interrupted',failed:'Needs attention',stopped:'Stopped',cancelled:'Stopped',unclaimed:'Not started',timed_out:'Interrupted'})[entry.run.state] || 'Working';
}
function scheduledRetries(group) {
  const attempts=[...group.active,...group.history];
  return group.history.filter(entry=>entry.run.retry_at != null &&
    !attempts.some(other=>other.run.number===entry.run.number && (other.run.started_at||0)>(entry.run.started_at||0)));
}
// Assignment links resolve once, then use the normal device/run conversation URL.
function assignedAgentEntry(data, query) {
  const agent = query.get('agent'), project = query.get('project'), number = Number(query.get('issue'));
  if (!agent || agent.startsWith('human:') || !project || !Number.isSafeInteger(number) || number <= 0) return;
  return projectView(data).flatMap(p => [...p.active, ...p.history])
    .filter(e => e.run.project_id === project && e.run.number === number &&
      (e.run.actor_id === agent || (agent.startsWith('codex:') && e.run.session_id === agent.slice(6))))
    .sort((a,b) => (b.run.started_at || 0) - (a.run.started_at || 0))[0];
}
async function resolveAssignedAgent(data, query, read) {
  const recent=assignedAgentEntry(data,query);
  if(recent)return recent;
  return read('/api/fleet/assignment?'+new URLSearchParams({project:query.get('project'),issue:query.get('issue'),agent:query.get('agent')}));
}
function managedFleet(data) {return {...data,machines:(data.machines||[]).map(m=>({...m,workers:(m.workers||[]).filter(w=>w.managed!==false&&(!w.retiring||w.pid>0))}))};}
function deviceView(data, project, now = Date.now()) {
  return (data.machines || []).map(machine => {
    const workers = (machine.workers || []).filter(w => !project || !w.config?.projects?.length || w.config.projects.includes(project));
    const online = machine.state === 'connected' && now / 1000 - (machine.heartbeat || 0) <= 15;
    const live = workers.filter(w => w.pid > 0), saved = workers.filter(w => !(w.pid > 0));
    return {machine, online, live, saved,
      active: online ? live.reduce((n,w) => n + (w.active ?? (w.runs || []).filter(r => r.finished_at == null).length), 0) : 0,
      capacity: online ? live.reduce((n,w) => n + (w.config?.concurrency || 1), 0) : 0};
  }).filter(d => !project || d.live.length || d.saved.length);
}
function workerPhase(worker,machine,now=Date.now()) {
  const active=worker.active??(worker.runs||[]).filter(r=>r.finished_at==null).length;
  const error=worker.error||(machine.configuration_error||'').split('; ').find(s=>s.startsWith('"'+worker.id+'":')||s.startsWith(worker.id+':'));
  if(machine.state!=='connected'||now/1000-(machine.heartbeat||0)>15)return {group:'attention',label:'Offline',note:'Showing last known state. Reconnect this machine to apply changes.'};
  if(error)return {group:'attention',label:'Configuration error',note:error.replace(/^"?[^:]+"?:\s*/, '')};
  if(worker.retry_at>now/1000&&!worker.pid)return {group:'attention',label:'Retrying',note:retryLabel({retry_at:worker.retry_at*1000},now)};
  if((worker.intent==='pause'||worker.intent==='drain'||worker.config?.enabled===false)&&worker.pid&&(active||worker.job_active))return {group:'working',label:'Finishing work',note:'Current agents can finish. No new tasks will start.'};
  if(worker.intent==='pause')return {group:'paused',label:'Paused',note:'Not picking up new tasks.'};
  if(worker.intent==='drain')return {group:'paused',label:'Draining',note:'Will stop after current work finishes.'};
  if(!worker.pid)return worker.intent==='running'?{group:'attention',label:'Starting',note:'Waiting for the worker to start.'}:{group:'stopped',label:'Stopped',note:'Saved configuration. Start it to pick up tasks.'};
  if(worker.config?.enabled===false)return {group:'paused',label:'Paused',note:'Not picking up new tasks.'};
  if(worker.job_active)return {group:'working',label:'Working',note:jobActivity(worker.job_active)};
  return active?{group:'working',label:'Working',note:active+' active '+(active===1?'agent':'agents')}:{group:'ready',label:'Ready',note:'Waiting for eligible issues.'};
}
function jobActivity(count) {return count+' running '+(count===1?'job':'jobs')+' · no task slots used';}
function slotUsage(worker,machine,now=Date.now()) {
  const online=machine.state==='connected'&&now/1000-(machine.heartbeat||0)<=15;
  const running=online&&worker.pid>0;
  const capacity=running?(worker.config?.concurrency||1):0;
  const occupied=running?(worker.active??(worker.runs||[]).filter(r=>r.finished_at==null).length):0;
  const pickup=worker.config?.enabled&&(!worker.intent||worker.intent==='running');
  return {capacity,occupied,available:pickup?Math.max(0,capacity-occupied):0,paused:pickup?0:Math.max(0,capacity-occupied),online,running};
}
function projectWorkers(machine) {
  return (machine.workers||[]).filter(w=>w.managed===true &&
    ((w.active||w.job_active||(w.runs||[]).some(r=>r.finished_at==null)) ||
      !w.retiring && w.intent!=='stop' && w.intent!=='drain' && (w.pid>0||w.intent==='running'||w.intent==='pause')));
}
function workerCapacityUpdate(document, host, id, delta) {
  const worker=document.machines?.[host]?.workers?.find(w=>w.id===id&&!w.retiring);
  if(!worker)throw Error('This worker is no longer configured. Reload the page.');
  const concurrency=(worker.config?.concurrency||1)+delta;
  if(!Number.isInteger(concurrency)||concurrency<1||concurrency>1024)throw Error('Agent limit must be between 1 and 1024.');
  return {host,id,intent:worker.intent,config:{concurrency}};
}
function workerLimitState(worker,machine,now=Date.now()) {
  const applied=worker.config?.concurrency||1;
  const limit=worker.desired_concurrency??machine.desired_workers?.find(w=>w.id===worker.id)?.config?.concurrency??applied;
  let status='',note='';
  if(limit!==applied){
    if(machine.state!=='connected'||now/1000-(machine.heartbeat||0)>15){status='Queued';note='Applies when this machine reconnects.';}
    else if(machine.configuration_error){status='Blocked';note='Resolve the setup error to apply this limit.';}
    else {status='Pending';note='Waiting for this worker to apply the new limit.';}
  }else if((worker.active||0)>limit){status='Finishing';note='Current agents will finish; new tasks respect the lower limit.';}
  return {limit,applied,status,note};
}
function configurationProblems(machine) {
  const identifiers=[...Object.keys(machine.projects||{}),...(machine.workers||[]).flatMap(w=>[w.id,'"'+w.id+'"'])];
  const parts=[];
  for(const part of (machine.configuration_error||'').split('; ').filter(Boolean)){
    if(!parts.length||identifiers.some(id=>part.startsWith(id+':')))parts.push(part);
    else parts[parts.length-1]+='; '+part;
  }
  return parts.map(detail=>{
    const project=Object.keys(machine.projects||{}).find(id=>detail.startsWith(id+': '))||null;
    const worker=(machine.workers||[]).find(w=>detail.startsWith(w.id+':')||detail.startsWith('"'+w.id+'":'))?.id||null;
    const reason=project?detail.slice(project.length+2):detail;
    const summary=reason.startsWith('Git clone failed')?'Couldn’t clone repository':reason.startsWith('Git clone timed out')?'Repository connection timed out':project?'Checkout needs attention':'Worker settings need attention';
    return {project,worker,summary,detail:reason};
  });
}
function jobConversation(entry,data,now=Date.now()) {
  if(entry?.run?.kind!=='job')return entry;
  const machine=(data.machines||[]).find(m=>m.node===entry.run.machine||m.host===entry.machine.host);
  return {...entry,machine:machine||entry.machine,online:!!machine&&machine.state==='connected'&&now/1000-(machine.heartbeat||0)<=15};
}
function syncProblem(machine, now=Date.now()) {
  const waiting=['configure','pull','heartbeat'].filter(operation=>machine.database_errors?.[operation]);
  if(waiting.length){const names={configure:'settings',pull:'data sync',heartbeat:'activity'};return 'Database busy; '+waiting.map(operation=>names[operation]).join(' and ')+' will retry';}
  if(machine.peer_sync_error)return 'Peer sync: '+machine.peer_sync_error;
  const sample=machine.sync;
  const local=['supervisor','controller'].includes(machine.role);
  const observedAge=local?0:now-machine.sync_observed_at*1000;
  if(!sample || !local && (!machine.sync_observed_at || observedAge<0 || observedAge>30000))return 'Data sync is not verified';
  // A fresh exchange with ordinary queued changes is progress, not an alert.
  if(sample.state==='catching_up'&&Number.isFinite(sample.exchange_age_ms)&&sample.exchange_age_ms+observedAge<=30000&&!sample.pending_reservations&&!sample.conflicting_attempts)return null;
  if(sample.state!=='healthy')return sample.message||'Data sync needs attention';
  return null;
}
function fleetAttention(data,now=Date.now()) {
  const notices=[],name=machine=>machine.hostname||machine.host;
  const devices=(data.machines||[]).map(machine=>({machine,online:machine.state==='connected'&&now/1000-(machine.heartbeat||0)<=15})).sort((a,b)=>name(a.machine).localeCompare(name(b.machine)));
  const problems=devices.map(d=>({machine:d.machine,note:!d.online?'offline; activity is last known':d.machine.configuration_error?'new settings have not applied':syncProblem(d.machine,now)})).filter(d=>d.note);
  if(problems.length||data.configuration?.error)notices.push({key:'machines',summary:data.configuration?.error?'Configuration file needs attention':problems.length+' '+(problems.length===1?'machine needs':'machines need')+' attention',note:[data.configuration?.error,...problems.map(d=>name(d.machine)+': '+d.note)].filter(Boolean).join(' · ')});
  const retained=data.conflicts?.length||0;
  const reports=devices.filter(d=>d.machine.host!=='local'&&!['supervisor','controller'].includes(d.machine.role)&&d.machine.conflicts>0).map(d=>name(d.machine)+': '+d.machine.conflicts);
  if(retained||reports.length){
    const summary=retained?(retained>=100?'100+':retained)+' saved offline '+(retained===1?'edit needs':'edits need')+' review':'Saved offline edits reported';
    const notes=[];
    if(retained)notes.push('Retained on the main machine after earlier sync conflicts.');
    if(reports.length)notes.push('Machine reports: '+reports.join(' · ')+'.');
    if(retained&&reports.length)notes.push('These counts can include copies of the same edits.');
    notices.push({key:'saved-edits',summary,note:notes.join(' ')});
  }
  return notices;
}
if (typeof module !== 'undefined') module.exports = {syncProblem, fleetAttention, fleetView, elapsed, projectView, agentState, scheduledRetries, retryLabel, deviceView, assignedAgentEntry, resolveAssignedAgent, managedFleet, workerPhase, slotUsage, projectWorkers, workerCapacityUpdate, workerLimitState, configurationProblems, jobConversation};
if (typeof document !== 'undefined') (() => {
  const $ = id => document.getElementById(id);
  const element = (tag, cls, text) => {const e=document.createElement(tag);if(cls)e.className=cls;if(text!==undefined)e.textContent=text;return e;};
  const mobile = document.documentElement.dataset.agentMobile === 'true';
  const detail = location.pathname.endsWith('/session');
  const base = '/agents';
  const route = () => {const query=new URLSearchParams(location.hash.slice(1));if(!detail)query.set('workers','');return query;};
  let projects=[], defaultProject, csrf, last, refreshing=false, disposed=false;
  let workerSearch=route().get('find')||'', workerEdit, workerEditRevision, workerEditPreview, workerEditBusy=false;
  let activityFilter='all';
  let cursor=0, olderCursor=0, loading=false, loaded=false, generation=0, follow=!route().has('at'), selected, historical, assignmentLoading=false;
  const seen = new Set();
  let takeoverBusy=false, takeoverTarget;
  let steerBusy=false, steerTarget, steerRequest;
  const steerDrafts=new Map();
  const takeovers=new Map();
  const takeoverKey=entry=>entry.machine.host+':'+entry.run.id;
  function savedTakeover(entry) {
    const key=takeoverKey(entry);
    if(!takeovers.has(key)){try{const saved=JSON.parse(sessionStorage.getItem('takeover:'+key));if(saved)takeovers.set(key,saved);}catch{}}
    return takeovers.get(key);
  }
  function saveTakeover(entry,value) {const key=takeoverKey(entry);takeovers.set(key,value);try{sessionStorage.setItem('takeover:'+key,JSON.stringify(value));}catch{}}
  HeyBossUI.icons();
  if(detail)$('nav-workers')?.removeAttribute('aria-current');
  else $('nav-workers')?.setAttribute('aria-current','page');
  const picker = new HeyBossUI.ProjectPicker({onSelect(project){
    const query=detail?new URLSearchParams():route();
    query.set('project',project);query.delete('scope');
    location.href=(detail?(mobile?'/project-resource':'/'):'/workers')+'#'+query;
  }});
  function context() {
    if(!defaultProject){if(last)render(last);return;}
    const rawId=HeyBossUI.projectId(defaultProject.id);
    const matched=projects.find(p=>p.id===rawId||p.name===rawId)||defaultProject||{id:rawId,name:rawId};
    const id=matched.id;
    const query=new URLSearchParams(location.hash.slice(1));
    if(detail&&(!query.get('project')||query.get('project')!==id)){query.set('project',id);history.replaceState(null,'','#'+query);}
    picker.update(projects, matched);
    if(last)render(last);
  }
  const link = (entry) => base+'/session#'+new URLSearchParams({project:entry.run.project_id,host:entry.machine.host,run:entry.run.id,...(entry.run.number?{issue:entry.run.number}:{})});
  const runLabel = run => HeyBossUI.actorLabel(run.actor_id || (run.session_id ? 'codex:'+run.session_id : 'agent:unknown'), run.model);
  const stateBadge = entry => element('span','agent-state '+(entry.online&&entry.run.finished_at==null?'is-live':entry.run.state==='completed'?'is-done':'is-quiet'),agentState(entry));
  function renderOverview(data) {
    $('worker-config').hidden=false;
    $('config-source').textContent=data.configuration?.source||'';
    $('config-source').hidden=!data.configuration?.source;
    renderWorkerBoard(data);
  }
  const projectLabel=id=>projects.find(p=>p.id===id)?.name||id.replace(/^named:/,'').split('/').pop();
  const workerLabel=w=>(w.config?.projects||[]).map(projectLabel).join(' + ')||'All projects';
  function runtime(run){
    const time=element('time','agent-runtime',elapsed(run));time.hidden=!time.textContent;time.title='Agent runtime';
    if(!time.hidden&&run.finished_at==null)time.dataset.startedAt=run.started_at;
    return time;
  }
  function machineControls(machine) {
    const bar=element('div','machine-controls');
    const add=element('button','button small','Add worker');add.type='button';Object.assign(add.dataset,{machineAction:'add',host:machine.host,focus:machine.host+':add'});
    bar.append(add);
    return bar;
  }
  const capacityRequests=new Map(),retryRequests=new Map(),signalRequests=new Set();
  function configurationRecovery(machine,expanded){
    const panel=element('div','setup-recovery');
    for(const problem of configurationProblems(machine)){
      const row=element('div','setup-problem'),copy=element('div','setup-copy');
      const title=element('div','setup-title');
      if(problem.project)title.append(element('strong','',projectLabel(problem.project)),element('span','',problem.summary));
      else title.append(element('span','',problem.summary));
      copy.append(title);
      const detail=element('details','setup-detail');detail.dataset.key=machine.host+':setup:'+ (problem.project||problem.worker||problem.detail);detail.open=expanded.has(detail.dataset.key);
      detail.append(element('summary','','Details'),element('p','',problem.detail));copy.append(detail);
      const actions=element('div','setup-actions');
      if(problem.project){
        const retry=element('button','setup-action',machine.project_retries?.[problem.project]?'Retry pending':'Retry');retry.type='button';Object.assign(retry.dataset,{retryProject:problem.project,host:machine.host});retry.disabled=!!machine.project_retries?.[problem.project]||retryRequests.has(machine.host+':'+problem.project);actions.append(retry);
        const edit=element('button','setup-action','Edit checkout');edit.type='button';Object.assign(edit.dataset,{machineAction:'edit-project',host:machine.host,project:problem.project});actions.append(edit);
      }else if(problem.worker){const edit=element('button','setup-action','Edit worker');edit.type='button';Object.assign(edit.dataset,{editWorker:problem.worker,host:machine.host});actions.append(edit);}
      else {const edit=element('a','setup-action','Edit settings');edit.href='/workers#view=configuration';actions.append(edit);}
      row.append(copy,actions);
      const status=retryRequests.get(machine.host+':'+problem.project)||(machine.project_retries?.[problem.project]?(machine.state==='connected'?'Waiting for the next setup attempt.':'Retry queued · waiting for connection.'):'');if(status){const note=element('p','setup-status',status);note.setAttribute('role','status');row.append(note);}
      panel.append(row);
    }
    return panel;
  }
  let capacityBusy=false;
  function capacityControls(machine,worker){
    const key=machine.host+':'+worker.id,request=capacityRequests.get(key);
    const state=workerLimitState(worker,machine);
    if(request?.saved&&state.limit===request.limit)capacityRequests.delete(key);
    const pending=capacityRequests.get(key),limit=pending?.limit??state.limit;
    const controls=element('span','capacity-controls'),count=element('span','worker-stepper');count.setAttribute('role','group');
    const label=workerLabel(worker)+' on '+(machine.hostname||machine.host);count.setAttribute('aria-label','Agent limit for '+label);
    for(const [delta,text] of [[-1,'−'],[0,String(limit)],[1,'+']]){
      const item=element(delta?'button':'span',delta?'worker-step':'worker-count',text);
      if(delta){item.type='button';Object.assign(item.dataset,{capacityDelta:delta,worker:worker.id,host:machine.host,focus:machine.host+':'+worker.id+':capacity:'+delta});item.setAttribute('aria-label',(delta>0?'Increase':'Decrease')+' agent limit for '+label);item.disabled=capacityBusy||worker.retiring||delta<0&&limit<=1||delta>0&&limit>=1024;}
      count.append(item);
    }
    controls.append(count,element('span','capacity-caption','agents'));
    const status=pending?(pending.saved?'Pending':'Saving…'):state.status;
    if(status){const note=element('span','capacity-status',status);note.setAttribute('role','status');note.title=pending?'Saving the requested agent limit.':state.note;controls.append(note);}
    return controls;
  }
  function removalActions(machine,worker){
    const actions=element('div','device-actions');
    if(!worker.retiring){const remove=element('button','button small','Remove');remove.type='button';Object.assign(remove.dataset,{machineAction:'remove',host:machine.host,removeWorker:worker.id,focus:machine.host+':'+worker.id+':remove'});actions.append(remove);}
    else if(worker.pid>0){actions.append(element('span','device-note','Finishing current work before removal.'));const kill=element('button','button small danger','Stop now');kill.type='button';Object.assign(kill.dataset,{signal:'stop',worker:worker.id,host:machine.host});actions.append(kill);}
    return actions;
  }
  function renderFleetAttention(data){
    const container=$('fleet-attention'),notices=fleetAttention(data);
    for(const child of [...container.children])if(!notices.some(n=>n.key===child.dataset.notice))child.remove();
    for(const [index,notice] of notices.entries()){
      let health=container.querySelector('[data-notice="'+notice.key+'"]');
      if(!health){health=element('details','fleet-health'+(notice.key==='saved-edits'?' fleet-review':''));health.dataset.notice=notice.key;health.open=true;health.append(element('summary'),element('p'));}
      const summary=health.querySelector('summary'),note=health.querySelector('p');
      if(summary.textContent!==notice.summary)summary.textContent=notice.summary;
      if(note.textContent!==notice.note)note.textContent=notice.note;
      if(container.children[index]!==health)container.insertBefore(health,container.children[index]||null);
    }
  }

  // Keep keyed cards and controls in place while polling updates their content.
  function updateWorkerNodes(parent, children) {
    const key=node=>node.nodeType===1?(node.dataset.key||node.dataset.focus||node.id):null;
    const keyed=new Map([...parent.childNodes].filter(key).map(node=>[key(node),node]));
    let cursor=parent.firstChild;
    for(const next of children){
      const id=key(next);
      let node=id?keyed.get(id):cursor&&!key(cursor)?cursor:null;
      if(!node||node.nodeName!==next.nodeName){node=next;parent.insertBefore(node,cursor);}
      else {
        if(node!==cursor)parent.insertBefore(node,cursor);
        if(node.nodeType===3){if(node.textContent!==next.textContent)node.textContent=next.textContent;}
        else {
          for(const attr of [...node.attributes])if(!next.hasAttribute(attr.name))node.removeAttribute(attr.name);
          for(const attr of next.attributes)if(node.getAttribute(attr.name)!==attr.value)node.setAttribute(attr.name,attr.value);
          updateWorkerNodes(node,[...next.childNodes]);
        }
      }
      cursor=node.nextSibling;
    }
    while(cursor){const next=cursor.nextSibling;cursor.remove();cursor=next;}
  }
  function renderWorkerBoard(data) {
    document.querySelector('.page-heading h1').textContent='Workers';
    document.title='Workers · Hey Boss';
    $('overview-note').textContent='Choose the projects. Set how many agents can work on them.';
    $('worker-view-help').hidden=true;
    document.body.classList.add('workers-page');
    const board=$('worker-board');
    const focusedMachine=document.activeElement?.closest('.project-worker-machine')?.dataset.host;
    const expanded=new Set([...board.querySelectorAll('details[open]')].map(d=>d.dataset.key));
    renderFleetAttention(data);
    const filters=[['all','All'],['busy','Working'],['paused','Paused'],['attention','Needs attention']];
    updateWorkerNodes($('worker-filters'),filters.map(([value,label])=>{const b=element('button','worker-filter',label);b.type='button';b.dataset.activityFilter=value;b.dataset.key=value;b.setAttribute('aria-pressed',String(activityFilter===value));return b;}));
    const search=workerSearch.trim().toLowerCase(),sections=[];
    for(const machine of [...(data.machines||[])].sort((a,b)=>Number(b.host==='local')-Number(a.host==='local')||(a.hostname||a.host).localeCompare(b.hostname||b.host))){
      const online=machine.state==='connected'&&Date.now()/1000-(machine.heartbeat||0)<=15;
      const workers=projectWorkers(machine);
      const shown=workers.filter(w=>{
        const phase=workerPhase(w,machine);
        return (activityFilter==='all'||activityFilter==='busy'&&phase.group==='working'||activityFilter==='paused'&&(w.intent==='pause'||w.config?.enabled===false&&!w.retiring)||activityFilter==='attention'&&phase.group==='attention')&&(!search||[workerLabel(w),w.id,machine.host,machine.hostname,...(w.runs||[]).flatMap(r=>[r.title,r.summary,r.last_event])].join(' ').toLowerCase().includes(search));
      });
      if(!shown.length&&(search||activityFilter!=='all'))continue;
      const section=element('section','project-worker-machine');section.dataset.host=machine.host;section.dataset.key='machine:'+machine.host;
      const heading=element('div','project-worker-machine-heading');
      const title=element('div'),name=element('h2','',machine.host==='local'?'This machine · '+(machine.hostname||'Local'):machine.hostname||machine.host);name.tabIndex=-1;title.append(name,element('span','machine-connection '+(online?'is-online':'is-offline'),online?'Connected':'Offline · changes apply after reconnecting'));
      heading.append(title,machineControls(machine));section.append(heading);
      if(machine.configuration_error)section.append(configurationRecovery(machine,expanded));
      const cards=element('div','project-worker-list');
      for(const w of shown){
        const phase=workerPhase(w,machine),usage=slotUsage(w,machine),key=machine.host+':'+w.id;
        const card=element('article','project-worker');card.dataset.key=key;card.dataset.worker=w.id;
        const header=element('div','project-worker-header'),identity=element('div','project-worker-identity');
        identity.append(element('span','project-worker-label','Worker'),element('h3','',workerLabel(w)));
        header.append(identity,capacityControls(machine,w));card.append(header);
        const status=element('div','project-worker-status');
        status.append(element('span','worker-state is-'+phase.group,w.retiring?'Removing · finishing agents':phase.label),element('span','',online?usage.occupied+' '+(usage.occupied===1?'agent':'agents')+' running':'Last known activity'));
        card.append(status);
        if(phase.group==='attention')card.append(element('p','activity-problem',phase.note));
        const actions=element('div','project-worker-actions');actions.dataset.key=key+':actions';
        if(!w.retiring){
          const add=element('button','button small','Add project');add.type='button';Object.assign(add.dataset,{machineAction:'project',host:machine.host,projectWorker:w.id,focus:key+':project'});add.disabled=!(w.config?.projects||[]).length;add.title=add.disabled?'This worker already includes all projects.':'';actions.append(add);
          const paused=w.intent==='pause'||w.intent==='drain'||w.intent==='stop'||w.config?.enabled===false;
          const pickup=element('button','button small',paused?'Resume':'Pause');pickup.type='button';Object.assign(pickup.dataset,{signal:paused?'resume':'pause',worker:w.id,host:machine.host,focus:key+':pickup'});pickup.disabled=signalRequests.has(key);pickup.title=paused?'Start picking up tasks.':'Let current agents finish and pause new tasks.';actions.append(pickup);
          const edit=element('button','button small','Settings');edit.type='button';Object.assign(edit.dataset,{editWorker:w.id,host:machine.host,focus:key+':settings'});actions.append(edit);
        }
        actions.append(...removalActions(machine,w).childNodes);card.append(actions);
        const runs=(w.runs||[]).filter(r=>r.finished_at==null);
        if(runs.length){
          const tasks=element('div','project-worker-tasks');tasks.dataset.key=key+':tasks';
          for(const run of runs){
            const task=element('a','project-worker-task');task.href=link({machine,run});task.dataset.key=key+':run:'+run.id;
            const text=element('span');text.append(element('strong','',run.title||'Working'),element('small','',[(run.project_name||projectLabel(run.project_id||'')),run.number?'#'+run.number:'',runLabel(run)].filter(Boolean).join(' · ')));if(run.summary||run.last_event)text.append(element('span','project-worker-task-note',run.summary||run.last_event));const state=element('span','project-worker-task-state');state.append(element('span','',agentState({run,online})),runtime(run));task.append(text,state);tasks.append(task);
          }
          card.append(tasks);
        }else if(usage.occupied||w.job_active)card.append(element('p','activity-note',w.job_active?jobActivity(w.job_active):'Agents are working. Task details are not available yet.'));
        cards.append(card);
      }
      if(!shown.length)cards.append(element('p','project-worker-empty','Add a worker to start running agents on this machine.'));
      section.append(cards);sections.push(section);
    }
    updateWorkerNodes(board,sections.length?sections:[element('p','worker-empty','No workers match this filter.')]);
    if(focusedMachine&&document.activeElement===document.body)[...board.querySelectorAll('.project-worker-machine')].find(section=>section.dataset.host===focusedMachine)?.querySelector('h2')?.focus({preventScroll:true});
  }

  function taskNavigation(run={}) {
    const project=run.project_id||route().get('project');
    const number=Number(run.number||route().get('issue'));
    const issue=Number.isSafeInteger(number)&&number>0?number:null;
    const hash=new URLSearchParams(project?{project}:{});
    if(issue>0)hash.set('issue',issue);
    const back=$('back');
    back.href=(mobile?'/project-resource':'/')+'#'+hash;
    const arrow=element('span','','←');arrow.setAttribute('aria-hidden','true');
    back.replaceChildren(arrow,document.createTextNode(issue?'Issue #'+issue:'Issues'));
  }
  function renderDetail(data) {
    taskNavigation();
    const assignment = route().get('agent');
    if (assignment) {
      const entry = assignedAgentEntry(data, route());
      if (entry) history.replaceState(null, '', link(entry));
      else {
        selected=undefined;
        $('session-title').textContent='Loading conversation';
        $('session-status').textContent='Finding the saved session for this assignment…';
        resolveAssignment();
        return;
      }
    }
    const resource=HeyBossRoutes.resolve();
    selected=projectView(data).flatMap(p=>[...p.active,...p.history]).find(e=>resource?.entity==='agent'&&(e.machine.host===resource.host||e.machine.hostname===resource.host)&&e.run.id===resource.id);
    if(!selected&&resource?.entity==='agent'&&resource.project){
      selected=historical||{machine:(data.machines||[]).find(m=>m.host===resource.host||m.hostname===resource.host)||{host:resource.host,state:'disconnected'},run:{id:resource.id,project_id:resource.project,title:'Saved creator conversation',finished_at:1,state:'completed',standalone:true},online:false};
    }
    if(!selected){const issueNum=Number(route().get('issue')),proj=route().get('project');if(issueNum>0&&proj){$('session-issue').hidden=false;$('session-issue').href=(mobile?'/project-resource#':'/#')+new URLSearchParams({project:proj,issue:issueNum});$('session-issue').textContent='Issue #'+issueNum+' ↗';}$('session-title').textContent='Conversation unavailable';$('session-status').textContent=assignment?'No recorded conversation for this assignment is in recent activity. Return to the issue to see its activity.':'This agent is no longer in recent activity.';$('conversation-empty').textContent='The saved session could not be loaded.';$('takeover-open').hidden=true;$('steer-open').hidden=true;$('steering-updates').hidden=true;$('resume-panel').hidden=true;$('takeover-note').hidden=true;return;}
    selected=jobConversation(selected,data);
    const {run,machine}=selected;
    taskNavigation(run);
    document.title=(run.title||'Conversation')+' · Hey Boss';
    $('session-title').textContent=run.title||'Preparing your task';
    $('session-context').textContent=runLabel(run)+' · '+(run.project_name||'Project')+' · '+(machine.hostname||machine.host);
    $('session-state').replaceChildren(stateBadge(selected));
    $('session-issue').hidden=!run.number;
    $('session-issue').href=(mobile?'/project-resource#':'/#')+new URLSearchParams({project:run.project_id,issue:run.number});$('session-issue').textContent='Issue #'+run.number+' ↗';
    $('session-status').textContent=run.state==='infrastructure_blocked'?infrastructureGuidance(run):!selected.online&&run.finished_at==null?'Device disconnected. Showing the conversation loaded so far.':run.finished_at!=null?'This conversation has ended.':'Live conversation · updates as the agent works';
    renderTakeover();
    if(route().get('steer')==='1') {
      const params=route();params.delete('steer');history.replaceState(null,'',location.pathname+'#'+params);
      if(!$('steer-open').hidden&&!$('steer-open').disabled)$('steer-open').click();
      else {$('steer-note').hidden=false;$('steer-note').textContent='This agent cannot be steered right now. Its conversation remains available; steering requires a connected, running task.';}
    }
    if(!loaded&&!loading)loadConversation();
  }
  function render(data) {last=data;if(detail)renderDetail(data);else renderOverview(data);}
  async function resolveAssignment() {
    if(assignmentLoading)return;
    assignmentLoading=true;const token=generation;
    try {
      const query=route();
      const entry=await resolveAssignedAgent(last,query,read);
      if(token!==generation||disposed)return;
      historical=entry;
      const params=new URLSearchParams(link(entry).split('#')[1]);
      if(query.has('at'))params.set('at',query.get('at'));
      history.replaceState(null,'',base+'/session#'+params);
      renderDetail(last);
    } catch(e) {
      if(token!==generation||disposed)return;
      $('session-title').textContent='Conversation unavailable';
      $('session-status').textContent=e.message;
      $('conversation-empty').textContent='The saved session could not be loaded.';
      fail(e);
    } finally {assignmentLoading=false;}
  }
  async function read(path) {
    const response=await fetch(path,{cache:'no-store'});const data=await response.json();
    if(!response.ok||data.ok===false)throw Error(data.error?.message||data.error||'Could not connect. Try again.');return data;
  }
  function fail(error) {$('error').textContent=error.message;$('error').hidden=false;}
  function renderTakeover() {
    if(!selected)return;
    const state=savedTakeover(selected), button=$('takeover-open');
    button.hidden=selected.run.standalone||Boolean(state?.stopped)||(runEnded()&&!state?.pending);
    button.disabled=takeoverBusy||steerBusy||!selected.online;
    button.textContent=state?.pending?'Check takeover':'Take over';
    $('steer-open').hidden=selected.run.standalone||runEnded()||Boolean(state?.pending||state?.stopped)||selected.run.stop_requested===true;
    $('steer-open').disabled=steerBusy||!selected.online;
    $('steer-open').title=selected.online?'Add an instruction while this agent keeps working':'Reconnect this device to steer its agent';
    $('takeover-note').hidden=!state?.pending;
    $('takeover-note').textContent=state?.error||(!selected.online?'Reconnect this device to finish taking over.':state?.assigned?'Stopping the agent. Your issue is assigned to you; the resume command will appear when it stops.':'Waiting for the device to confirm takeover…');
    $('resume-panel').hidden=!state?.stopped;
    if(state?.stopped){$('resume-command').textContent=state.resume_command||'';$('resume-copy').hidden=!state.resume_command;$('resume-command').hidden=!state.resume_command;$('resume-note').textContent=state.resume_command?'The agent has stopped and the issue is assigned to you. Run this command in your terminal to continue the saved session.':'The agent has stopped and the issue is assigned to you. It did not save a resumable session.';}
  }
  const runEnded=()=>selected.run.finished_at!=null;
  async function takeOver(entry) {
    if(takeoverBusy)return;
    takeoverBusy=true;const token=generation;
    saveTakeover(entry,{...savedTakeover(entry),pending:true,error:null});
    if(selected)renderTakeover();
    try{
      const response=await fetch('/api/fleet/takeover',{method:'POST',headers:{'Content-Type':'application/json',...(csrf?{'X-Hey-Boss-CSRF':csrf}:{})},body:JSON.stringify({host:entry.machine.host,run:entry.run.id})});
      const result=await response.json();
      if(!response.ok||result.ok===false)throw Error(result.error?.message||result.error||'Could not take over. Refresh and try again.');
      saveTakeover(entry,{pending:!result.stopped,assigned:true,stopped:result.stopped,resume_command:result.resume_command});
      if(token===generation&&!disposed){$('error').hidden=true;await refresh();}
    }catch(e){saveTakeover(entry,{...savedTakeover(entry),pending:true,error:e.message});if(token===generation&&!disposed)fail(e);}finally{takeoverBusy=false;if(selected&&token===generation&&!disposed)renderTakeover();}
  }
  let refreshQueued=false,lastRefreshAt=0;
  async function refresh() {
    if(refreshing){refreshQueued=true;return;}
    const wait=Math.max(0,400-(Date.now()-lastRefreshAt));
    if(wait>0){if(refreshQueued)return;refreshQueued=true;setTimeout(()=>{refreshQueued=false;refresh();},wait);return;}
    refreshing=true;
    try{render(await read('/api/fleet/status'));}catch(e){fail(e);if(last)render(last);}finally{lastRefreshAt=Date.now();refreshing=false;if(refreshQueued){refreshQueued=false;setTimeout(()=>refresh(),400);}}
  }
  function message(item) {
    const row=element(item.role==='tool'||item.role==='activity'?'details':'article','chat-message '+item.role);row.dataset.message=item.id;
    if(item.role==='tool'||item.role==='activity'){
      const summary=element('summary','',({'Result':'Tool result','Thinking':'Thinking','exec_command':'Run a command','functions.exec':'Use tools','functions.apply_patch':'Edit files','apply_patch':'Edit files','write_stdin':'Check a command','Search':'Search the web'})[item.label]||'Tool activity');
      const pre=element('pre','',item.label+'\n\n'+item.text);pre.tabIndex=0;row.append(summary,pre);
    }else{
      const label=element('div','message-author',item.role==='user'?'You':item.label||runLabel(selected?.run||{}));const body=element('div','message-body');
      if(item.html&&item.role==='assistant')body.innerHTML=item.html;else body.textContent=item.text;
      row.append(label,body);
    }
    return row;
  }
  async function loadConversation(earlier=false) {
    if(loading||!selected)return;
    loading=true;const token=generation;
    $('load-earlier').disabled=true;$('conversation').setAttribute('aria-busy','true');
    try{
      const initial=!loaded;
      const query={host:route().get('host')||selected.machine.host,run:selected.run.id,project:selected.run.project_id,cursor};
      if(earlier)query.before=olderCursor;else if(initial){if(route().has('at'))query.at=route().get('at');else query.latest=1;}
      const oldHeight=document.documentElement.scrollHeight,oldScroll=scrollY;
      const data=await read('/api/fleet/conversation?'+new URLSearchParams(query));
      if(token!==generation||disposed)return;
      if(data.run){historical={...selected,run:{...data.run,model:data.run.model || selected.run.model}};selected=historical;renderDetail(last);}
      if(data.created_resources){
        const resources=data.created_resources;
        $('session-resources').hidden=!resources.length;
        $('session-resources-title').textContent=(selected.run.standalone?'Created in this session':'Created in this run')+' · '+resources.length;
        $('session-resources-list').replaceChildren(...resources.map(resource=>{
          const item=element('li'),a=element('a','',resource.title);
          a.href=(resource.kind==='artifact'?'/artifacts#':mobile?'/project-resource#':'/#')+new URLSearchParams({project:resource.project_id,[resource.kind]:resource.id});
          item.append(element('span','resource-kind',resource.kind==='issue'?'Issue #'+resource.id:'Artifact'),a);return item;
        }));
        $('session-resources-more').hidden=!data.more_created_resources;
      }
      if(!earlier){
        const receipts=data.steering||[];
        $('steering-updates').hidden=!receipts.length;$('steering-count').textContent=receipts.length?'('+receipts.length+')':'';
        $('steering-list').replaceChildren(...receipts.map(receipt=>{
          const row=element('li'),state=({queued:'Queued',sending:'Delivery unconfirmed',delivered:'Delivered',rejected:'Not delivered',superseded:'Replaced by newer GitHub status',uncertain:'Delivery unconfirmed'})[receipt.state]||'Delivery unconfirmed';
          row.append(element('div','steering-receipt',state+' · '+({session:'This agent',issue:'This issue',project:'This project'})[receipt.scope]),element('p','steering-instruction',receipt.text));
          if(receipt.error)row.append(element('p','steering-problem',receipt.error));
          return row;
        }));
      }
      const entries=data.messages.filter(m=>!seen.has(m.id));
      const fragment=document.createDocumentFragment();
      for(const item of entries){seen.add(item.id);fragment.append(message(item));}
      if(earlier)$('conversation').prepend(fragment);else $('conversation').append(fragment);
      if(initial&&route().has('at')){
        const target=[...$('conversation').children].find(e=>e.dataset.message===route().get('at'));
        if(target){target.classList.add('is-origin');if(target.tagName==='DETAILS')target.open=true;target.prepend(element('div','origin-invocation-label','Creating invocation'));requestAnimationFrame(()=>target.scrollIntoView({block:'center'}));}
      }
      if(!earlier)cursor=data.cursor;loaded=true;
      if(initial||earlier){olderCursor=data.older_cursor||0;$('load-earlier').hidden=!data.has_earlier;}
      if(earlier)scrollTo(0,oldScroll+document.documentElement.scrollHeight-oldHeight);
      $('conversation-empty').hidden=seen.size>0;
      $('conversation-empty').textContent=selected.run.state==='infrastructure_blocked'&&infrastructureLabel(selected.run)==='GitHub quota exhausted'&&!selected.run.session_id?'No agent started during this attempt.':data.availability==='waiting'?(selected.run.finished_at!=null?'Saved history is unavailable on this device.':'The agent is getting started. Its saved conversation will appear here.'):'No messages yet. This page will update as the agent works.';
      if(!earlier&&follow&&entries.length){$('conversation-end').scrollIntoView({behavior:'instant',block:'end'});}
      $('jump-live').hidden=follow||!seen.size;
      if(data.has_more&&!earlier&&!route().has('at'))setTimeout(()=>loadConversation(),0);
    }catch(e){fail(e);}finally{if(token===generation){loading=false;$('load-earlier').disabled=false;$('conversation').setAttribute('aria-busy','false');}}
  }
  $('overview-page').hidden=detail;$('session-page').hidden=!detail;
  if(detail){document.body.classList.add('conversation-page');$('load-earlier').onclick=()=>{follow=false;loadConversation(true);};$('jump-live').onclick=()=>{if(route().has('at')){const params=route();params.delete('at');history.replaceState(null,'','#'+params);loaded=false;cursor=0;seen.clear();$('conversation').replaceChildren();loadConversation();}follow=true;$('conversation-end').scrollIntoView({behavior:'smooth',block:'end'});$('jump-live').hidden=true;};addEventListener('scroll',()=>{follow=!route().has('at')&&$('conversation-end').getBoundingClientRect().bottom<=innerHeight+160;$('jump-live').hidden=follow||!seen.size;},{passive:true});}
  $('takeover-open').onclick=()=>{
    if(!selected)return;
    if(savedTakeover(selected)?.pending){takeOver(selected);return;}
    takeoverTarget=selected;
    $('takeover-task').textContent='Issue #'+selected.run.number+' · '+(selected.run.title||'Preparing your task')+' · '+(selected.machine.hostname||selected.machine.host);
    $('takeover-dialog').showModal();$('takeover-cancel').focus();
  };
  $('takeover-cancel').onclick=()=>$('takeover-dialog').close();
  $('takeover-confirm').onclick=()=>{const entry=takeoverTarget;$('takeover-dialog').close();if(entry)takeOver(entry);};
  const steerDraft=()=>({text:$('steer-text').value,scope:$('steer-form').elements.scope.value});
  const keepSteerDraft=()=>{if(steerTarget)steerDrafts.set(takeoverKey(steerTarget),steerDraft());};
  $('steer-open').onclick=()=>{
    if(!selected||!selected.online||runEnded()||steerBusy)return;
    steerTarget=selected;
    const draft=steerDrafts.get(takeoverKey(selected))||{text:'',scope:'session'};
    $('steer-text').value=draft.text;$('steer-form').elements.scope.value=draft.scope;
    $('steer-task').textContent='Issue #'+selected.run.number+' · '+(selected.run.title||'Your task')+' · '+(selected.machine.hostname||selected.machine.host);
    $('steer-error').hidden=true;$('steer-dialog').showModal();$('steer-text').focus();$('steer-text').scrollIntoView({block:'center'});
  };
  $('steer-form').oninput=keepSteerDraft;
  $('steer-dialog').addEventListener('cancel',event=>{if(steerBusy)event.preventDefault();else keepSteerDraft();});
  $('steer-dialog').addEventListener('close',()=>{if(!steerBusy&&!$('steer-dialog').open&&!$('steer-open').hidden)$('steer-open').focus();});
  $('steer-cancel').onclick=()=>{keepSteerDraft();$('steer-dialog').close();};
  $('steer-form').onsubmit=async event=>{
    event.preventDefault();if(steerBusy||!steerTarget)return;
    const draft=steerDraft(),entry=steerTarget,token=generation,key=takeoverKey(entry);
    if(!draft.text.trim()||new TextEncoder().encode(draft.text).length>32000){$('steer-error').textContent='Enter an instruction of up to 32000 bytes.';$('steer-error').hidden=false;return;}
    const payload=JSON.stringify({host:entry.machine.host,run:entry.run.id,...draft});
    if(steerRequest?.payload!==payload)steerRequest={payload,id:HeyBossUI.requestId()};
    steerBusy=true;keepSteerDraft();$('steer-error').hidden=true;
    for(const control of $('steer-form').elements)control.disabled=true;
    // Sending should never hold the conversation behind a modal. Keep the draft
    // and request ID until acceptance, since a lost reply may already be saved.
    $('steer-dialog').close();$('main').focus({preventScroll:true});
    $('steer-note').hidden=false;$('steer-note').textContent='Sending your instruction… You can keep reading the conversation.';
    renderTakeover();
    const controller=new AbortController();
    const timeout=setTimeout(()=>controller.abort(),15000);
    try{
      const response=await fetch('/api/fleet/steer',{method:'POST',signal:controller.signal,headers:{'Content-Type':'application/json',...(csrf?{'X-Hey-Boss-CSRF':csrf}:{})},body:JSON.stringify({...JSON.parse(payload),request_id:steerRequest.id})});
      const result=await response.json();
      if(!response.ok||result.ok===false)throw Error(result.error?.message||result.error||'Could not send. Your instruction is preserved; retry when the device reconnects.');
      if(!['queued','delivered'].includes(result.state))throw Error(result.error||'Delivery is unconfirmed. Check the conversation before sending another instruction.');
      steerDrafts.delete(key);steerRequest=null;
      if(token===generation&&!disposed){$('steer-text').value='';$('steer-note').textContent=result.state==='delivered'?'Instruction delivered to this agent.':({session:'Message queued for this agent.',issue:'Issue requirement saved. Message queued for this agent.',project:'Project instruction saved. Updates queued for agents using project instructions.'})[draft.scope]+' Watch the conversation for delivery confirmation.';}
    }catch(error){
      if(token===generation&&!disposed){
        $('steer-note').hidden=true;
        $('steer-text').value=draft.text;$('steer-form').elements.scope.value=draft.scope;
        $('steer-error').textContent=controller.signal.aborted?'No confirmation yet. Your instruction is preserved. Retry to check the same submission without sending it twice.':error.message;
        $('steer-error').hidden=false;
        for(const control of $('steer-form').elements)control.disabled=false;
        $('steer-dialog').showModal();$('steer-text').focus();$('steer-text').scrollIntoView({block:'center'});
      }
    }
    finally{clearTimeout(timeout);steerBusy=false;for(const control of $('steer-form').elements)control.disabled=false;if(selected&&!disposed)renderTakeover();}
  };
  $('resume-copy').onclick=async()=>{const command=$('resume-command').textContent;try{await navigator.clipboard.writeText(command);$('copy-status').textContent='Command copied.';}catch{$('copy-status').textContent='Select the command and copy it with your keyboard.';const range=document.createRange();range.selectNodeContents($('resume-command'));const selection=getSelection();selection.removeAllRanges();selection.addRange(range);}};
  $('refresh').onclick=async()=>{$('error').hidden=true;await refresh();if(detail)await loadConversation();};
  async function configRequest(body) {
    const response=await fetch('/api/fleet/configuration',{cache:'no-store',...(body?{method:'POST',headers:{'Content-Type':'application/json',...(csrf?{'X-Hey-Boss-CSRF':csrf}:{})},body:JSON.stringify(body)}:{})});
    const data=await response.json();if(!response.ok||data.ok===false)throw Error(data.error?.message||data.error||'Could not load configuration.');return data;
  }
  function confirmFleet(title,message,submitLabel='Continue',danger=false){
    return new Promise(resolve=>{
      let dialog=$('fleet-confirm-dialog');
      if(!dialog){
        dialog=document.createElement('dialog');
        dialog.id='fleet-confirm-dialog';
        dialog.className='takeover-dialog';
        dialog.innerHTML='<h2 id="fleet-confirm-title"></h2><p id="fleet-confirm-message"></p><div class="takeover-dialog-actions"><button id="fleet-confirm-cancel" class="button" type="button">Cancel</button><button id="fleet-confirm-submit" class="button primary" type="button">Continue</button></div>';
        document.body.append(dialog);
      }
      $('fleet-confirm-title').textContent=title;
      $('fleet-confirm-message').textContent=message;
      const cancelBtn=$('fleet-confirm-cancel'),submitBtn=$('fleet-confirm-submit');
      submitBtn.textContent=submitLabel;
      submitBtn.className='button '+(danger?'danger':'primary');
      const finish=ok=>{if(dialog.open)dialog.close();resolve(ok);};
      cancelBtn.onclick=()=>finish(false);
      submitBtn.onclick=()=>finish(true);
      dialog.oncancel=e=>{e.preventDefault();finish(false);};
      dialog.showModal();
      cancelBtn.focus();
    });
  }
  $('worker-filters').onclick=event=>{const b=event.target.closest('button');if(!b)return;if(b.dataset.activityFilter)activityFilter=b.dataset.activityFilter;if(last)renderWorkerBoard(last);};
  $('worker-search').value=workerSearch;
  $('worker-search').oninput=event=>{workerSearch=event.target.value;if(last)renderWorkerBoard(last);};
  function workerEditButtons(){for(const input of $('worker-form').querySelectorAll('input,textarea,select'))input.disabled=workerEditBusy;$('worker-editor-preview').disabled=workerEditBusy;$('worker-editor-save').disabled=workerEditBusy||!workerEditPreview;$('worker-editor-cancel').disabled=workerEditBusy;}
  const editFields=['name','provider','slots','intent','workspace','projects','directory','directories'];
  function workspaceLabel(enabled){return enabled===true?'Dedicated worktree':'Existing checkout';}
  let workerEditOriginal='';
  const editFingerprint=()=>JSON.stringify(editFields.map(field=>$('worker-'+field).value));
  async function openWorkerEditor(host,id){
    if(workerEditBusy)return;
    workerEditBusy=true;
    try{
      const data=await configRequest();
      const worker=data.document?.machines?.[host]?.workers?.find(w=>w.id===id);
      if(!worker)throw Error(data.error||'This worker is no longer configured. Reload the page.');
      workerEdit={host,id,worker};$('worker-scope').open=!!last?.machines?.find(m=>m.host===host)?.configuration_error?.includes(id);workerEditRevision=data.revision;workerEditPreview=undefined;
      $('worker-editor-title').textContent='Worker settings';
      $('worker-editor-context').textContent=workerLabel(worker)+' · '+host;
      $('worker-name').value=worker.config?.name||'Worker';$('worker-provider').value=worker.config?.provider||'codex';$('worker-slots').value=worker.config?.concurrency||1;$('worker-intent').value=worker.intent;
      $('worker-workspace').value=String(worker.config?.worktree_enabled===true);
      $('worker-projects').value=(worker.config?.projects||[]).join('\n');$('worker-directory').value=worker.config?.directory||'';$('worker-directories').value=Object.entries(worker.config?.directories||{}).map(([p,d])=>p+' = '+d).join('\n');
      workerEditOriginal=editFingerprint();$('worker-editor-status').textContent='Other settings are preserved. Structured edits may reformat the YAML file.';$('worker-editor-changes').replaceChildren();$('worker-editor').showModal();
    }catch(error){fail(error);}finally{workerEditBusy=false;workerEditButtons();}
  }
  async function closeWorkerEditor(event){if(workerEditBusy){event?.preventDefault();return;}if(editFingerprint()!==workerEditOriginal){event?.preventDefault();if(!(await confirmFleet('Discard unsaved worker changes?','Your unsaved changes to this worker will be lost.','Discard changes',true)))return;}workerEdit=null;$('worker-editor').close();}
  $('worker-editor-cancel').onclick=closeWorkerEditor;$('worker-editor').addEventListener('cancel',closeWorkerEditor);
  $('worker-form').oninput=()=>{workerEditPreview=undefined;$('worker-editor-status').textContent='Unsaved changes. Review before saving.';$('worker-editor-changes').replaceChildren();workerEditButtons();};
  function workerUpdate(){
    const directories={};
    for(const line of $('worker-directories').value.split('\n').map(s=>s.trim()).filter(Boolean)){const at=line.indexOf('=');if(at<1||!line.slice(at+1).trim())throw Error('Use project ID = /absolute/path for each per-project path.');const project=line.slice(0,at).trim();if(Object.hasOwn(directories,project))throw Error('Only one path is allowed per project.');directories[project]=line.slice(at+1).trim();}
    return {host:workerEdit.host,id:workerEdit.id,intent:$('worker-intent').value,config:{provider:$('worker-provider').value,name:$('worker-name').value.trim(),concurrency:Number($('worker-slots').value),worktree_enabled:$('worker-workspace').value==='true',projects:$('worker-projects').value.split('\n').map(s=>s.trim()).filter(Boolean),directory:$('worker-directory').value.trim(),directories}};
  }
  $('worker-form').onsubmit=async event=>{
    event.preventDefault();if(workerEditBusy)return;workerEditBusy=true;workerEditButtons();
    try{const update=workerUpdate();const data=await configRequest({worker_update:update,revision:workerEditRevision,save:false});workerEditPreview=update;
      const before=workerEdit.worker;const changes=[];for(const [key,label] of [['name','Name'],['provider','Agent'],['concurrency','Agent limit'],['projects','Projects'],['directory','Working directory'],['directories','Per-project paths']]){const defaults={provider:'codex',name:'Worker',concurrency:1,projects:[],directory:'',directories:{}};const old=before.config?.[key]??defaults[key];if(JSON.stringify(old)!==JSON.stringify(update.config[key]))changes.push(label+': '+(typeof old==='object'?JSON.stringify(old):old||'Automatic')+' → '+(typeof update.config[key]==='object'?JSON.stringify(update.config[key]):update.config[key]||'Automatic'));}if(before.intent!==update.intent)changes.unshift('Pickup mode: '+before.intent+' → '+update.intent);
      if((before.config?.worktree_enabled===true)!==update.config.worktree_enabled)changes.push('Workspace: '+workspaceLabel(before.config?.worktree_enabled)+' → '+workspaceLabel(update.config.worktree_enabled));
      $('worker-editor-changes').replaceChildren(...changes.map(text=>element('li','',text)));$('worker-editor-status').textContent=changes.length?(update.intent==='stop'&&before.intent!=='stop'?'Saving will stop this worker and its current agents.':'Ready to save to fleet.yaml. Machines will apply these settings automatically.'):'No changes to save.';if(!changes.length)workerEditPreview=undefined;$('worker-editor-changes').scrollIntoView({block:'nearest'});
    }catch(error){workerEditPreview=undefined;$('worker-editor-status').textContent=error.message;}finally{workerEditBusy=false;workerEditButtons();}
  };
  $('worker-editor-save').onclick=async()=>{
    if(workerEditBusy||!workerEditPreview)return;workerEditBusy=true;workerEditButtons();
    try{await configRequest({worker_update:workerEditPreview,revision:workerEditRevision,save:true});workerEditOriginal=editFingerprint();workerEditPreview=undefined;
      $('worker-save-note').textContent='Worker saved. Check its machine below for application status.';$('worker-save-note').hidden=false;await refresh();$('worker-editor').close();workerEdit=null;
    }catch(error){workerEditPreview=undefined;$('worker-editor-status').textContent=error.message;}finally{workerEditBusy=false;workerEditButtons();}
  };
  let machineEdit=null,machineBusy=false,machinePathEdited=false;
  function machineButtons(){for(const input of $('machine-form').elements)input.disabled=machineBusy;}
  async function openMachineEditor(host,action,workerId,projectId){
    if(machineBusy)return;
    machineBusy=true;
    try{
      const data=await configRequest(),machine=data.document?.machines?.[host]||(last?.machines?.some(m=>m.host===host)?{workers:[]}:null);
      if(!machine)throw Error('Machine is no longer configured. Reload the page.');
      const worker=machine.workers?.find(w=>w.id===workerId&&!w.retiring);
      if(['remove','project'].includes(action)&&!worker)throw Error('This worker is no longer configured. Reload the page.');
      if(action==='edit-project'&&!machine.projects?.[projectId])throw Error('Project is no longer configured. Reload the page.');
      machineEdit={host,action,workerId,projectId,revision:data.revision,id:HeyBossUI.requestId()};machinePathEdited=false;
      const name=last?.machines?.find(m=>m.host===host)?.hostname||host;
      $('machine-editor-context').textContent=(worker?workerLabel(worker)+' · ':'')+name;
      $('machine-editor-title').textContent=({add:'Add worker',remove:'Remove worker',project:'Add project','edit-project':'Edit checkout'})[action];
      $('machine-editor-description').textContent=({add:'This worker starts automatically. Choose its first project and how many agents can run at once.',remove:'Stop picking up tasks and let current agents finish. The worker will disappear when its work is done. Project files are kept.',project:'Add a project to this worker. Its projects share the same agent limit.','edit-project':'Update the repository URL or checkout path. Saved changes apply automatically.'})[action];
      for(const field of ['slots','project','git','workspace','path'])$('machine-'+field+'-field').hidden=!({add:['slots','project','git','workspace','path'],remove:[],project:['project','git','workspace','path'],'edit-project':['git','workspace','path']}[action].includes(field));
      const choices=new Map(projects.map(p=>[p.id,{...p}]));
      for(const configured of Object.values(data.document?.machines||{}))for(const [id,checkout] of Object.entries(configured.projects||{}))choices.set(id,{...choices.get(id),id,name:choices.get(id)?.name||projectLabel(id),git:checkout.git});
      $('machine-project').replaceChildren(new Option('Choose a project or enter a Git URL…',''));
      for(const project of [...choices.values()].filter(p=>!p.hidden_at&&!worker?.config?.projects?.includes(p.id)).sort((a,b)=>a.name.localeCompare(b.name))){
        const git=project.git||(/^[^/:\s]+(?::\d+)?\/[^\s?#]+$/.test(project.id)?'https://'+project.id+'.git':'');
        const option=new Option(project.name+(git?'':' · No Git repository'),project.id);option.dataset.git=git;option.disabled=!git;$('machine-project').append(option);
      }
      $('machine-git').required=action!=='remove';$('machine-workspace').required=action!=='remove';$('machine-path').required=false;
      $('machine-slots').value='1';$('machine-workspace').value=machine.workspace||'~/Workspace';$('machine-git').value='';$('machine-path').value='';
      if(projectId&&machine.projects?.[projectId]){const project=machine.projects[projectId];$('machine-git').value=project.git;$('machine-path').value=project.reuse_existing?'':project.path;machinePathEdited=!project.reuse_existing;$('machine-project').value=projectId;}
      $('machine-editor-status').textContent=last?.machines?.find(m=>m.host===host)?.state==='disconnected'?'This machine is offline. Changes apply when it reconnects.':'';
      $('machine-editor-save').textContent=({add:'Create worker',remove:'Remove worker',project:'Add project','edit-project':'Save & retry'})[action];
      suggestCheckout();$('machine-editor').showModal();
    }catch(error){fail(error);}finally{machineBusy=false;machineButtons();}
  }
  function suggestCheckout(){if(machinePathEdited)return;const git=$('machine-git').value.trim().replace(/\/+$|\.git$/g,'');const name=git.split(/[/:]/).pop();$('machine-path').placeholder=name?'Automatic · '+$('machine-workspace').value.replace(/\/+$/,'')+'/'+name:'Automatic';}
  $('machine-project').onchange=()=>{$('machine-git').value=$('machine-project').selectedOptions[0]?.dataset.git||'';suggestCheckout();};
  $('machine-git').oninput=()=>{$('machine-project').value='';suggestCheckout();};$('machine-workspace').oninput=suggestCheckout;$('machine-path').oninput=()=>{machinePathEdited=!!$('machine-path').value;};
  $('machine-editor-cancel').onclick=()=>{if(!machineBusy){$('machine-editor').close();machineEdit=null;}};
  $('machine-editor').addEventListener('cancel',event=>{if(machineBusy)event.preventDefault();else machineEdit=null;});
  $('machine-form').onsubmit=async event=>{
    event.preventDefault();if(machineBusy||!machineEdit)return;machineBusy=true;machineButtons();$('machine-editor-status').textContent='Saving…';
    try{
      const {host,action,id,revision,projectId,workerId}=machineEdit;let update={host,action};
      if(action==='add')Object.assign(update,{id,concurrency:Number($('machine-slots').value)});
      if(action==='remove')update.id=workerId;
      if(['add','project','edit-project'].includes(action))Object.assign(update,{git:$('machine-git').value.trim(),workspace:$('machine-workspace').value.trim(),path:$('machine-path').value.trim(),...(action==='edit-project'?{project:projectId}:action==='project'?{worker:workerId}:{})});
      await configRequest({machine_update:update,revision,save:true});
      $('worker-save-note').textContent=action==='remove'?'Worker removed. Any current agents will finish; project files are kept.':action==='project'?'Project added. It shares this worker’s agent limit.':action==='edit-project'?'Checkout saved. Retrying setup.':'Worker created. Its machine will prepare the checkout and start picking up tasks.';$('worker-save-note').hidden=false;
      if(action==='edit-project')await configRequest({retry_project:{host,project:projectId}});
      $('machine-editor').close();machineEdit=null;render(await read('/api/fleet/status'));
    }catch(error){$('machine-editor-status').textContent=error.message;}finally{machineBusy=false;machineButtons();}
  };
  $('worker-board').onclick=async event=>{
    $('error').hidden=true;
    const retry=event.target.closest('[data-retry-project]');
    if(retry){event.preventDefault();const {host,retryProject:project}=retry.dataset,key=host+':'+project;if(retryRequests.has(key))return;
      retryRequests.set(key,'Requesting retry…');if(last)render(last);
      try{await configRequest({retry_project:{host,project}});retryRequests.set(key,'Retry queued · waiting for the next setup attempt.');await refresh();}
      catch(error){retryRequests.delete(key);fail(error);}
      finally{setTimeout(()=>{retryRequests.delete(key);if(last)render(last);},5000);if(last)render(last);}return;
    }
    const capacity=event.target.closest('[data-capacity-delta]');
    if(capacity){
      event.preventDefault();if(capacityBusy)return;
      capacityBusy=true;for(const b of $('worker-board').querySelectorAll('[data-capacity-delta]'))b.disabled=true;
      try{
        const data=await configRequest(),{host,worker,capacityDelta}=capacity.dataset;
        const update=workerCapacityUpdate(data.document,host,worker,Number(capacityDelta));
        const key=host+':'+worker;capacityRequests.set(key,{limit:update.config.concurrency,saved:false});if(last)render(last);
        await configRequest({worker_update:update,revision:data.revision,save:true});
        capacityRequests.set(key,{limit:update.config.concurrency,saved:true});
        $('worker-save-note').hidden=true;
        await refresh();
      }catch(error){capacityRequests.delete(capacity.dataset.host+':'+capacity.dataset.worker);fail(error);}finally{capacityBusy=false;if(last)render(last);}return;
    }
    const machine=event.target.closest('[data-machine-action]');if(machine){event.preventDefault();await openMachineEditor(machine.dataset.host,machine.dataset.machineAction,machine.dataset.removeWorker||machine.dataset.projectWorker,machine.dataset.project);return;}
    const edit=event.target.closest('[data-edit-worker]');if(edit){event.preventDefault();await openWorkerEditor(edit.dataset.host,edit.dataset.editWorker);return;}
    const b=event.target.closest('button[data-signal]');if(!b)return;
    if(['stop','restart'].includes(b.dataset.signal)&&!(await confirmFleet((b.dataset.signal==='stop'?'Stop':'Restart')+' this worker?','This will '+(b.dataset.signal==='stop'?'stop':'restart')+' the worker and its currently running agents.',b.dataset.signal==='stop'?'Stop worker':'Restart worker',b.dataset.signal==='stop')))return;
    const signalKey=b.dataset.host+':'+b.dataset.worker;if(signalRequests.has(signalKey))return;signalRequests.add(signalKey);b.disabled=true;try{const response=await fetch('/api/fleet',{method:'POST',headers:{'Content-Type':'application/json',...(csrf?{'X-Hey-Boss-CSRF':csrf}:{})},body:JSON.stringify({kind:'signal',host:b.dataset.host,worker:b.dataset.worker,signal:b.dataset.signal,id:HeyBossUI.requestId()})});const data=await response.json();if(!response.ok||data.ok===false)throw Error(data.error?.message||data.error||'Could not apply this change.');render(await read('/api/fleet/status'));}catch(error){fail(error);}finally{signalRequests.delete(signalKey);if(last)render(last);}
  };

  addEventListener('beforeunload',event=>{if(workerEdit&&editFingerprint()!==workerEditOriginal){event.preventDefault();event.returnValue='';}});
  addEventListener('hashchange',()=>{if(route().has('find')){workerSearch=route().get('find');$('worker-search').value=workerSearch;}if(detail){keepSteerDraft();$('steer-dialog').close();$('steer-note').hidden=true;$('steering-updates').hidden=true;$('steering-list').replaceChildren();historical=null;$('session-resources').hidden=true;follow=!route().has('at');$('takeover-dialog').close();$('copy-status').textContent='';generation++;cursor=0;olderCursor=0;loading=false;loaded=false;seen.clear();$('conversation').replaceChildren();}context();});
  addEventListener('pagehide',()=>{disposed=true;generation++;});
  addEventListener('pageshow',event=>{if(event.persisted){disposed=false;loading=false;refresh();if(detail)loadConversation();}});
  (async()=>{try{
    const data=await read(mobile?'/api/agent-bootstrap':'/api/bootstrap');csrf=data.csrf;projects=data.projects||[];defaultProject=data.project||projects[0];context();
    if(mobile){$('quick-issue-open').hidden=true;$('nav-inbox').href='/';$('nav-issues').href='/#issues';}
    await refresh();
    if(!mobile){const events=new EventSource('/api/fleet/events');events.addEventListener('connected',()=>refresh());events.onmessage=()=>refresh();events.onerror=()=>{$('connection').classList.add('offline');$('connection').querySelector('span').textContent='Reconnecting…';};events.onopen=()=>{$('connection').classList.remove('offline');$('connection').querySelector('span').textContent='Connected';};}
  }catch(e){fail(e);}})();
  setInterval(()=>{if(!document.hidden&&!disposed){refresh();if(detail){if(!route().has('at'))loadConversation();const state=selected&&savedTakeover(selected);if(selected?.online&&state?.pending&&!state.error)takeOver(selected);}}},3000);
  setInterval(()=>{if(!document.hidden&&!disposed){const now=Date.now();for(const time of document.querySelectorAll('.agent-runtime[data-started-at]'))time.textContent=elapsed({started_at:Number(time.dataset.startedAt)},now);}},1000);
  document.addEventListener('visibilitychange',()=>{if(!document.hidden){refresh();if(detail)loadConversation();}});
})();
