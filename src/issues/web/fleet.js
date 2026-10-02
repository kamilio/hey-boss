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
    for (const worker of machine.workers || []) for (const run of [...(worker.runs || []), ...(worker.chiefs || [])]) {
      const id = run.project_id;
      if (!groups.has(id)) groups.set(id, {id, name: run.project_name || id || 'Project', active: [], history: [], chiefs: []});
      const entry = {run, machine, worker, online: online && worker.pid > 0};
      groups.get(id)[run.kind === 'chief' ? 'chiefs' : run.finished_at == null ? 'active' : 'history'].push(entry);
    }
  }
  return [...groups.values()].sort((a,b) => Number(b.active.length > 0) - Number(a.active.length > 0) || a.name.localeCompare(b.name));
}
function chiefState(entry, now = Date.now()) {
  if (!entry.online) return entry.worker.pid > 0 ? 'Device disconnected' : 'Worker stopped';
  if (entry.run.state === 'running') return entry.run.queued || entry.run.next_at <= now ? 'Running · 1 queued' : 'Running';
  if (entry.run.enabled === false) return 'Disabled';
  if (entry.worker.config?.enabled === false) return 'Paused';
  const minutes = Math.max(0, Math.ceil((entry.run.next_at - now) / 60000));
  if (['blocked','failed'].includes(entry.run.state)) return retryLabel({retry_at:entry.run.next_at}, now);
  return 'Waiting · ' + (minutes ? `${minutes} ${minutes === 1 ? 'minute' : 'minutes'} left` : 'Due now');
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
  if (entry.run.kind === 'chief') return entry.run.state === 'running' ? 'Running' : entry.run.state === 'idle' ? 'Completed' : ['failed','blocked'].includes(entry.run.state) ? 'Failed' : 'Stopped';
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
  if((worker.intent==='pause'||worker.intent==='drain'||worker.config?.enabled===false)&&worker.pid&&active)return {group:'working',label:'Finishing work',note:'Current agents can finish. No new tasks will start.'};
  if(worker.intent==='pause')return {group:'paused',label:'Paused',note:'Not picking up new tasks.'};
  if(worker.intent==='drain')return {group:'paused',label:'Draining',note:'Will stop after current work finishes.'};
  if(!worker.pid)return worker.intent==='running'?{group:'attention',label:'Starting',note:'Waiting for the worker to start.'}:{group:'stopped',label:'Stopped',note:'Saved configuration. Start it to pick up tasks.'};
  if(worker.config?.enabled===false)return {group:'paused',label:'Paused',note:'Not picking up new tasks.'};
  return active?{group:'working',label:'Working',note:active+' active '+(active===1?'agent':'agents')}:{group:'ready',label:'Ready',note:'Waiting for eligible issues.'};
}
function slotUsage(worker,machine,now=Date.now()) {
  const online=machine.state==='connected'&&now/1000-(machine.heartbeat||0)<=15;
  const running=online&&worker.pid>0;
  const capacity=running?(worker.config?.concurrency||1):0;
  const occupied=running?(worker.active??(worker.runs||[]).filter(r=>r.finished_at==null).length):0;
  const pickup=worker.config?.enabled&&(!worker.intent||worker.intent==='running');
  return {capacity,occupied,available:pickup?Math.max(0,capacity-occupied):0,paused:pickup?0:Math.max(0,capacity-occupied),online,running};
}
function workerScopeGroups(entries) {
  const groups=new Map();
  for(const entry of entries){
    const projects=[...new Set(entry.worker.config?.projects||[])].sort(),key=JSON.stringify(projects);
    if(!groups.has(key))groups.set(key,{key,projects,entries:[]});
    groups.get(key).entries.push(entry);
  }
  for(const group of groups.values()){
    const names=new Map(),shortIds=new Map();group.labels=new Map();
    for(const {worker:w} of group.entries){
      const name=w.config?.name||'Worker';
      names.set(name,(names.get(name)||0)+1);
      shortIds.set(w.id.slice(0,8),(shortIds.get(w.id.slice(0,8))||0)+1);
    }
    for(const {worker:w} of group.entries){
      const name=w.config?.name||'Worker',id=shortIds.get(w.id.slice(0,8))>1?w.id:w.id.slice(0,8);
      group.labels.set(w.id,names.get(name)>1||!w.config?.name?name+' · '+id:name);
    }
  }
  return [...groups.values()];
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
if (typeof module !== 'undefined') module.exports = {fleetView, elapsed, projectView, agentState, scheduledRetries, retryLabel, deviceView, assignedAgentEntry, resolveAssignedAgent, chiefState, managedFleet, workerPhase, slotUsage, workerScopeGroups, workerCapacityUpdate, workerLimitState, configurationProblems};
if (typeof document !== 'undefined') (() => {
  const $ = id => document.getElementById(id);
  const element = (tag, cls, text) => {const e=document.createElement(tag);if(cls)e.className=cls;if(text!==undefined)e.textContent=text;return e;};
  const mobile = document.documentElement.dataset.agentMobile === 'true';
  const detail = location.pathname.endsWith('/session');
  const base = '/agents';
  const route = () => {const query=new URLSearchParams(location.hash.slice(1));if(!detail&&(location.pathname==='/workers'||query.get('view')!=='conversations'))query.set('workers','');return query;};
  let projects=[], defaultProject, csrf, last, refreshing=false, disposed=false;
  let configRevision, configOriginal='', configPreview, configBusy=false;
  let workerFilter='current', workerSearch=route().get('find')||'', workerEdit, workerEditRevision, workerEditPreview, workerEditBusy=false;
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
  const picker = new HeyBossUI.ProjectPicker({onSelect(project){
    const query=detail?new URLSearchParams({view:'conversations'}):route();
    query.set('project',project);query.delete('scope');
    location.href=(detail?base:location.pathname)+'#'+query;
  }});
  function context() {
    if(!defaultProject){if(last)render(last);return;}
    const rawId=HeyBossUI.projectId(defaultProject.id);
    const matched=projects.find(p=>p.id===rawId||p.name===rawId)||defaultProject||{id:rawId,name:rawId};
    const id=matched.id;
    const query=new URLSearchParams(location.hash.slice(1));
    if(!query.get('project')||query.get('project')!==id){query.set('project',id);history.replaceState(null,'','#'+query);}
    picker.update(projects, matched);
    if(last)render(last);
  }
  const link = (entry) => base+'/session#'+new URLSearchParams({project:entry.run.project_id,host:entry.machine.host,run:entry.run.id});
  const runLabel = run => HeyBossUI.actorLabel(run.actor_id || (run.session_id ? 'codex:'+run.session_id : 'agent:unknown'), run.model);
  const stateBadge = entry => element('span','agent-state '+(entry.online&&entry.run.finished_at==null?'is-live':entry.run.state==='completed'?'is-done':'is-quiet'),agentState(entry));
  function card(entry, history=false) {
    const {run,machine}=entry;
    const a=element('a',(history?'agent-card history-card':'agent-card')+(run.state==='infrastructure_blocked'?' is-held':''));a.href=link(entry);a.dataset.focus=machine.host+':'+run.id;
    const top=element('div','agent-card-top');top.append(stateBadge(entry),element('span','location-label',machine.hostname||machine.host));
    const title=element('h3','',run.title||'Preparing your task');
    const activity=run.last_event||'';
    const preview=element('p','agent-preview',run.state==='infrastructure_blocked'?infrastructureGuidance(run):run.summary||(/^(Goal:|Codex session|\/goal)/.test(activity)?'Making progress on this task.':/^(\/bin\/|.* -lc )/.test(activity)?'Checking changes and running commands.':activity)||(entry.online?'Getting started…':'Reconnect to see the latest activity.'));
    const bottom=element('div','agent-card-bottom');bottom.append(element('span','',`Issue #${run.number}`),element('span','open-conversation',history?'Read conversation →':'Open conversation →'));
    a.append(top,element('p','agent-model',runLabel(run)),title,preview,bottom);return a;
  }
  function renderOverview(data) {
    const workersPage=route().has('workers');
    document.querySelector('.fleet-tabs').hidden=false;
    document.body.classList.toggle('workers-page',workersPage);
    const project=route().get('project'), hash=new URLSearchParams({project});
    $('workers-tab').href=base+'#'+hash;
    $('conversations-tab').href=base+'#'+new URLSearchParams({project,view:'conversations'});
    $('worker-status-view').href=base+'#'+hash;
    $('worker-config-view').href=base+'#'+new URLSearchParams({project,view:'configuration'});
    $('conversations-tab').setAttribute('aria-current',workersPage?'false':'page');
    $('worker-config').hidden=!workersPage;
    $('projects').hidden=workersPage;
    $('workers-tab').setAttribute('aria-current',workersPage?'page':'false');
    document.querySelector('.page-heading h1').textContent=workersPage?'Workers':'Agents';
    document.title=(workersPage?'Workers':'Agents')+' · Hey Boss';
    if(workersPage){
      $('overview-note').textContent='Running agents and the work happening right now.';
      $('show-all').hidden=true;
      if(data.configuration?.error){$('config-status').textContent='File error: '+data.configuration.error+' The last valid configuration is still active.';}
      else if(configRevision&&data.configuration?.revision&&configRevision!==data.configuration.revision&&!configBusy){$('config-status').textContent='The file changed elsewhere. Reload before saving; your edits are still here.';configPreview=undefined;$('config-save').disabled=true;}
      renderWorkerBoard(data);$('device-settings').hidden=true;return;
    }
    $('show-all').textContent='All projects';
    $('show-all').href=base+'#'+new URLSearchParams({project,view:'conversations',scope:'all'});
    const focus=document.activeElement?.dataset.focus;
    const open=new Set([...$('projects').querySelectorAll('details[open]')].map(d=>d.dataset.section));
    const filter=route().get('scope')==='all'?null:route().get('project');
    const groups=projectView(data).filter(p=>!filter||p.id===filter||p.name===filter);
    $('show-all').hidden=!filter;
    $('overview-note').textContent=groups.some(p=>p.active.length)?'A little closer to done. See what’s moving.':'Your projects, and the work behind them.';
    const sections=groups.map(group=>{
      const section=element('section','project-section');
      const holds=scheduledRetries(group);
      const heading=element('div','project-heading');const title=element('div');
      title.append(element('h2','',group.name),element('p','',group.active.length?group.active.some(e=>e.online)?'In progress':'Waiting for a connection':holds.length?'Scheduled retries':'Recent work'));
      const issues=element('a','project-issues','View issues →');issues.href=(mobile?'/#issues&':'/#')+new URLSearchParams({project:group.id});
      heading.append(title,issues);section.append(heading);
      for(const entry of group.chiefs){
        const {run,machine,worker}=entry;
        const chief=element('section','chief-panel');
        const top=element('div','chief-heading');
        top.append(element('h3','','Chief'),element('span','agent-state '+(entry.online&&run.state==='running'?'is-live':'is-quiet'),chiefState(entry)));
        const owner=element('p','chief-owner',(worker.config?.name||'Worker '+worker.id.slice(0,8))+' · '+(machine.hostname||machine.host));
        chief.append(top,element('p','agent-model',runLabel(run)),owner);
        if(run.started_at!=null){
          const last=element('div','chief-last-pass');
          const outcome=run.state==='running'?'Current pass':run.state==='idle'?'Last pass · Completed':['failed','blocked'].includes(run.state)?'Last pass · Failed':'Last pass · '+run.state;
          const time=element('time','',new Date(run.finished_at??run.started_at).toLocaleString(undefined,{month:'short',day:'numeric',hour:'numeric',minute:'2-digit'}));time.dateTime=new Date(run.finished_at??run.started_at).toISOString();
          last.append(element('span','',outcome),time);chief.append(last);
          if(run.summary||run.last_event)chief.append(element('p','agent-preview',run.summary||run.last_event));
        }
        if(run.session_id){const a=element('a','chief-conversation',run.state==='running'?'Open conversation →':'Read last conversation →');a.href=link(entry);a.dataset.focus=machine.host+':'+run.id;chief.append(a);}
        section.append(chief);
      }
      if(holds.length){section.append(element('h3','agents-section-label','Scheduled retries'));const held=element('div','agent-grid');for(const entry of holds)held.append(card(entry,true));section.append(held);}
      if(group.active.length||group.chiefs.length)section.append(element('h3','agents-section-label','Active agents'));
      if(group.active.length){const grid=element('div','agent-grid');for(const entry of group.active)grid.append(card(entry));section.append(grid);}
      if(!group.active.length&&group.chiefs.length)section.append(element('p','chief-owner','No active issue agents.'));
      if(group.history.length){const history=element('details','project-history');history.dataset.section=group.id;history.append(element('summary','','Completed & earlier conversations'));const past=element('div','agent-grid');for(const entry of group.history)past.append(card(entry,true));history.append(past);history.open=open.has(group.id)||(!group.active.length&&!group.chiefs.length&&!holds.length);section.append(history);}
      return section;
    });
    if(!sections.length){const empty=element('section','agents-empty');empty.append(element('div','empty-orbit','✧'),element('h2','','Room for your next idea'),element('p','','When an agent picks up a task, its conversation will appear here.'));sections.push(empty);}
    $('projects').replaceChildren(...sections);
    if(focus)[...$('projects').querySelectorAll('[data-focus]')].find(e=>e.dataset.focus===focus)?.focus({preventScroll:true});
    renderDevices(data);
  }
  const projectLabel=id=>projects.find(p=>p.id===id)?.name||id.replace(/^named:/,'').split('/').pop();
  const workerLabel=w=>w.config?.name&&!/^Worker(?: \d+)?$/.test(w.config.name)?w.config.name:(w.config?.projects||[]).map(projectLabel).join(' + ')||'Worker '+w.id.slice(0,8);
  const scopeLabel=group=>group.projects.map(projectLabel).join(' + ')||'All projects';
  const projectHasWorker=(machine,id)=>(machine.workers||[]).some(w=>!w.retiring&&(!(w.config?.projects||[]).length||w.config.projects.includes(id)));
  function runtime(run){
    const time=element('time','agent-runtime',elapsed(run));time.hidden=!time.textContent;time.title='Agent runtime';
    if(!time.hidden&&run.finished_at==null)time.dataset.startedAt=run.started_at;
    return time;
  }
  function renderFleetActivity(data) {
    const board=$('worker-board'),focus=document.activeElement?.dataset.focus;
    const known=new Set([...board.querySelectorAll('details')].map(d=>d.dataset.key));
    const expanded=new Set([...board.querySelectorAll('details[open]')].map(d=>d.dataset.key));
    const devices=deviceView(managedFleet(data));
    const entries=devices.flatMap(device=>[...device.live,...device.saved].map(worker=>({device,worker,usage:slotUsage(worker,device.machine),phase:workerPhase(worker,device.machine)})));
    renderFleetAttention(data,devices);
    const relevant=entries.filter(e=>e.worker.pid>0||e.worker.intent==='running'||e.worker.intent==='drain');
    const filters=[['all','All'],['busy','Working'],['available','Available'],['attention','Needs attention']];
    $('worker-filters').replaceChildren(...filters.map(([value,label])=>{const b=element('button','worker-filter',label);b.type='button';b.dataset.activityFilter=value;b.setAttribute('aria-pressed',String(activityFilter===value));return b;}));
    const search=workerSearch.trim().toLowerCase();
    const matches=e=>(activityFilter==='all'||activityFilter==='busy'&&e.usage.occupied>0||activityFilter==='available'&&e.usage.available>0||activityFilter==='attention'&&e.phase.group==='attention')&&(!search||[workerLabel(e.worker),e.worker.id,e.device.machine.host,e.device.machine.hostname,...(e.worker.config?.projects||[]),...(e.worker.runs||[]).filter(r=>r.finished_at==null).flatMap(r=>[r.title,r.summary,r.last_event])].join(' ').toLowerCase().includes(search));
    const activity=run=>{
      const last=(run.last_event||'').replace(/\s+/g,' ').trim();
      if(!last||/^(reasoning|tool|message|assistant|thinking)$/i.test(last))return run.summary||'Working on this task';
      if(/^(\/bin\/|.* -lc )/.test(last))return 'Running a command';
      if(/^(Goal:|Codex session|\/goal)/.test(last))return run.summary||'Working on this task';
      return last;
    };
    function task(run,entry){
      const a=element('a','activity-task');a.href=link({machine:entry.device.machine,run});a.dataset.focus=entry.device.machine.host+':task:'+run.id;
      const meta=element('span','activity-task-meta');meta.append(element('span','',run.project_name||projectLabel(run.project_id||'')),element('span','',run.number?'#'+run.number:'Organizer'),element('span','agent-model',runLabel(run)));
      const text=element('span','activity-task-copy');text.append(element('strong','',run.title||'Organizing project'),element('span','',activity(run)));
      const state=element('span','activity-task-state',agentState({run,worker:entry.worker,machine:entry.device.machine,online:entry.device.online}));
      a.append(meta,text,state,runtime(run),element('span','activity-task-arrow','↗'));return a;
    }
    function worker(entry,label,showLimit){
      const {worker:w,device:d,usage:u,phase}=entry;
      const runs=(w.runs||[]).filter(r=>r.finished_at==null),chiefs=(w.chiefs||[]).filter(r=>r.state==='running');
      const row=element('details','activity-worker');row.dataset.worker=w.id;row.dataset.key=d.machine.host+':worker:'+w.id;row.open=expanded.has(row.dataset.key);
      const summary=element('summary','activity-worker-summary');summary.dataset.focus=row.dataset.key;
      const identity=element('span','activity-worker-name');identity.append(element('strong','',label));
      const slots=showLimit?capacityControls(d.machine,w):element('span','activity-worker-slots',d.online?u.occupied+' running':'Last known');
      const current=element('span','activity-worker-current');
      if(runs.length){const line=element('span','activity-current-line');line.append(element('span','activity-current-title',(runs[0].number?'#'+runs[0].number+' · ':'')+(runs[0].title||'Working')+(runs.length>1?' · +'+(runs.length-1)+' more':'')),runtime(runs[0]));current.append(line,element('span','activity-current-note',activity(runs[0])));}
      else current.append(element('span','activity-current-note',u.occupied?'Working · task details unavailable':chiefs.length?'Organizing project queue':u.available?'Waiting for a task':phase.note));
      summary.append(identity,element('span','worker-state is-'+phase.group,phase.label),slots,current);row.append(summary);
      const detail=element('div','activity-worker-detail');
      if(runs.length){const tasks=element('div','activity-tasks');for(const run of runs)tasks.append(task(run,entry));detail.append(tasks);}
      if(u.occupied>runs.length)detail.append(element('p','activity-note',(u.occupied-runs.length)+' more running agents; task details are unavailable.'));
      if(phase.group==='attention')detail.append(element('p','activity-note activity-problem',phase.note));
      if(chiefs.length){const organizers=element('details','activity-organizers');organizers.dataset.key=row.dataset.key+':organizers';organizers.open=expanded.has(organizers.dataset.key);organizers.append(element('summary','','Organizers · outside agent slots'));for(const chief of chiefs)organizers.append(task(chief,entry));detail.append(organizers);}
      const footer=element('div','activity-worker-footer');
      footer.append(element('span','',u.running?[u.available?u.available+' available':null,u.paused?u.paused+' paused':null].filter(Boolean).join(' · '):(w.config?.projects||[]).map(projectLabel).join(', ')));
      const edit=element('a','','Worker settings →');edit.href='/workers#view=configuration&find='+encodeURIComponent(w.id);edit.dataset.focus=row.dataset.key+':settings';footer.append(edit);detail.append(footer,removalActions(d.machine,w));row.append(detail);return row;
    }
    const sections=[];
    for(const d of devices.sort((a,b)=>b.active-a.active||Number(b.online)-Number(a.online))){
      const own=entries.filter(e=>e.device===d),shown=relevant.filter(e=>e.device===d&&matches(e));
      const pendingProjects=Object.fromEntries(Object.entries(d.machine.projects||{}).filter(([id])=>!projectHasWorker(d.machine,id)&&(activityFilter==='all'||activityFilter==='attention'&&d.machine.configuration_error)&&(!search||[id,projectLabel(id),d.machine.host,d.machine.hostname].join(' ').toLowerCase().includes(search))));
      if(!shown.length&&!Object.keys(pendingProjects).length&&(search||activityFilter!=='all'))continue;
      const total=own.reduce((n,e)=>{for(const k of ['capacity','occupied','available','paused'])n[k]+=e.usage[k];return n;},{capacity:0,occupied:0,available:0,paused:0});
      const section=element('details','activity-machine');section.dataset.host=d.machine.host;section.dataset.key='machine:'+d.machine.host;section.open=known.has(section.dataset.key)?expanded.has(section.dataset.key):true;
      const heading=element('summary','activity-machine-heading');heading.dataset.focus=section.dataset.key;
      const name=element('h2','',d.machine.host==='local'?'This machine':d.machine.hostname||d.machine.host);
      heading.append(name,element('span','machine-connection '+(d.online?'is-online':'is-offline'),d.online?'Connected':'Offline'),element('span','activity-machine-usage',d.online?total.occupied+' agents running · '+total.available+' available'+(total.paused?' · '+total.paused+' paused':''):'Last known activity'));section.append(heading);section.append(machineControls(d.machine));
      if(d.machine.configuration_error)section.append(configurationRecovery(d.machine,expanded));
      section.append(machineProjects({...d.machine,projects:pendingProjects}));
      shown.sort((a,b)=>b.usage.occupied-a.usage.occupied||Number(b.phase.group==='attention')-Number(a.phase.group==='attention')||workerLabel(a.worker).localeCompare(workerLabel(b.worker)));
      for(const group of workerScopeGroups(shown)){
        const scope=element('details','worker-scope-group');scope.dataset.key=section.dataset.key+':scope:'+group.key;scope.open=expanded.has(scope.dataset.key);
        const summary=element('summary','worker-scope-heading');summary.dataset.focus=scope.dataset.key;
        const usage=group.entries.reduce((n,e)=>{n.occupied+=e.usage.occupied;n.capacity+=e.usage.capacity;return n;},{occupied:0,capacity:0});
        const current=group.entries.flatMap(e=>(e.worker.runs||[]).filter(r=>r.finished_at==null));
        summary.append(element('h3','',scopeLabel(group)),group.entries.length===1?capacityControls(d.machine,group.entries[0].worker):element('span','scope-worker-count','Agent limit: '+group.entries.reduce((n,e)=>n+workerLimitState(e.worker,d.machine).limit,0)),element('span','scope-usage',d.online?usage.occupied+' '+(usage.occupied===1?'agent':'agents')+' running':'Last known'));
        const preview=current.length?(current[0].number?'#'+current[0].number+' · ':'')+(current[0].title||'Working')+(current.length>1?' · +'+(current.length-1)+' more tasks':''):usage.occupied?'Working · task details unavailable':group.entries.some(e=>e.phase.group==='attention')?'Needs attention':group.entries.some(e=>(e.worker.chiefs||[]).some(c=>c.state==='running'))?'Organizing project queue':group.entries.some(e=>e.usage.available)?'Waiting for a task':'Pickup paused';
        const taskPreview=element('span','scope-current-task');taskPreview.append(element('span','scope-current-title',preview));if(current.length)taskPreview.append(runtime(current[0]));summary.append(taskPreview);scope.append(summary);
        const labels=element('div','activity-columns');labels.append(element('span','','Worker'),element('span','','Status'),element('span','',group.entries.length===1?'Agents':'Agent limit'),element('span','','Current task · latest activity'));scope.append(labels);
        for(const entry of group.entries)scope.append(worker(entry,group.labels.get(entry.worker.id),group.entries.length>1));section.append(scope);
      }
      if(!shown.length&&!Object.keys(pendingProjects).length)section.append(element('p','activity-note','No running workers.'));
      const inactive=own.filter(e=>!relevant.includes(e)).length;
      if(inactive){const a=element('a','activity-inactive',inactive+' paused or stopped workers →');a.href='/workers#view=configuration&find='+encodeURIComponent(d.machine.host);section.append(a);}sections.push(section);
    }
    board.replaceChildren(...(sections.length?sections:[element('p','activity-note','No workers match this filter.')]));
    if(focus)[...board.querySelectorAll('[data-focus]')].find(e=>e.dataset.focus===focus)?.focus({preventScroll:true});
  }

  function machineControls(machine) {
    const bar=element('div','machine-controls');
    const add=element('button','button small','+ Add project');add.type='button';Object.assign(add.dataset,{machineAction:'project',host:machine.host,focus:machine.host+':project'});
    bar.append(add);
    const retiring=(machine.workers||[]).filter(w=>w.retiring&&w.pid>0);
    if(retiring.length)bar.append(element('span','machine-draining',retiring.length+' finishing'));
    return bar;
  }
  const capacityRequests=new Map(),retryRequests=new Map();
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
    const name=workerLabel(worker),duplicate=(machine.workers||[]).filter(w=>workerLabel(w)===name).length>1;
    const label=name+(duplicate?' · '+worker.id.slice(0,8):'')+' on '+(machine.hostname||machine.host);count.setAttribute('aria-label','Agent limit for '+label);
    for(const [delta,text] of [[-1,'−'],[0,String(limit)],[1,'+']]){
      const item=element(delta?'button':'span',delta?'worker-step':'worker-count',text);
      if(delta){item.type='button';Object.assign(item.dataset,{capacityDelta:delta,worker:worker.id,host:machine.host,focus:machine.host+':'+worker.id+':capacity:'+delta});item.setAttribute('aria-label',(delta>0?'Increase':'Decrease')+' agent limit for '+label);item.disabled=capacityBusy||worker.retiring||delta<0&&limit<=1||delta>0&&limit>=1024;}
      count.append(item);
    }
    controls.append(count,element('span','capacity-caption','limit'));
    const status=pending?(pending.saved?'Pending':'Saving…'):state.status;
    if(status){const note=element('span','capacity-status',status);note.setAttribute('role','status');note.title=pending?'Saving the requested agent limit.':state.note;controls.append(note);}
    return controls;
  }
  function machineProjects(machine){
    const panel=element('div','machine-projects');
    for(const [id,project] of Object.entries(machine.projects||{})){
      const row=element('div','machine-project');const copy=element('div');copy.append(element('strong','',projectLabel(id)),element('code','',project.resolved_path||project.path),element('small','',project.git));
      row.append(copy);panel.append(row);
      if((machine.workers||[]).some(w=>!w.retiring)){const add=element('button','button small','Assign');add.type='button';Object.assign(add.dataset,{machineAction:'project',host:machine.host,project:id});add.setAttribute('aria-label','Assign '+projectLabel(id)+' to a worker');row.append(add);}
      const worker=element('button','button small','Add worker');worker.type='button';Object.assign(worker.dataset,{machineAction:'add',host:machine.host,project:id});worker.setAttribute('aria-label','Add worker for '+projectLabel(id));row.append(worker);
    }
    return panel;
  }
  function removalActions(machine,worker){
    const actions=element('div','device-actions');
    if(!worker.retiring){const remove=element('button','button small','Remove worker');remove.type='button';Object.assign(remove.dataset,{machineAction:'remove',host:machine.host,removeWorker:worker.id});actions.append(remove);}
    else if(worker.pid>0){actions.append(element('span','device-note','Finishing current work before removal.'));const kill=element('button','button small danger','Kill now');kill.type='button';Object.assign(kill.dataset,{signal:'stop',worker:worker.id,host:machine.host});actions.append(kill);}
    return actions;
  }
  function renderFleetAttention(data,devices){
    const previous=$('fleet-attention').querySelector('details');
    const problems=devices.filter(d=>!d.online||d.machine.configuration_error);
    if(!problems.length&&!data.configuration?.error){$('fleet-attention').replaceChildren();return;}
    const health=element('details','fleet-health');health.open=!!previous?.open;
    health.append(element('summary','',data.configuration?.error?'Configuration file needs attention':problems.length+' '+(problems.length===1?'machine needs':'machines need')+' attention'));
    health.append(element('p','',data.configuration?.error||problems.map(d=>(d.machine.hostname||d.machine.host)+': '+(!d.online?'offline; activity is last known':'new settings have not applied')).join(' · ')));
    $('fleet-attention').replaceChildren(health);
  }

  function renderWorkerBoard(data) {
    const configuring=route().get('view')==='configuration';
    document.querySelector('.page-heading h1').textContent=configuring?'Workers & settings':'Worker activity';
    document.title=(configuring?'Worker settings':'Worker activity')+' · Hey Boss';
    $('overview-note').textContent=configuring?'Manage saved workers across your machines.':'Running agents and agent limits by project.';
    $('worker-view-help').hidden=true;
    document.body.classList.add('workers-page');
    $('worker-status-view').setAttribute('aria-current',configuring?'false':'page');
    $('worker-config-view').setAttribute('aria-current',configuring?'page':'false');
    $('config-editor').hidden=!configuring;
    if(!configuring){renderFleetActivity(data);return;}
    const devices=deviceView(managedFleet(data));
    const entries=devices.flatMap(device=>[...device.live,...device.saved].map(worker=>({device,worker,phase:workerPhase(worker,device.machine)})));
    const count=group=>entries.filter(e=>e.phase.group===group).length;
    renderFleetAttention(data,devices);
    $('worker-view-help').textContent=configuring?'Edit a worker’s settings below. Changes save to the supervisor’s fleet.yaml and apply automatically.':'Workers are grouped by machine. Expand a row for its tasks and controls. Stopped workers are kept out of the way.';
    const filters=[['current','Current',entries.length-count('stopped')],['attention','Needs attention',count('attention')],['paused','Paused',count('paused')],['stopped','Stopped',count('stopped')],['all','All',entries.length]];
    $('worker-filters').replaceChildren(...filters.map(([value,label,total])=>{const b=element('button','worker-filter',label+' '+total);b.type='button';b.dataset.filter=value;b.setAttribute('aria-pressed',String(workerFilter===value));return b;}));
    const board=$('worker-board'),focus=document.activeElement?.dataset.focus;
    const expanded=new Set([...board.querySelectorAll('details[open]')].map(d=>d.dataset.key));
    const search=workerSearch.trim().toLowerCase();
    const matches=e=>(workerFilter==='all'||workerFilter==='current'&&e.phase.group!=='stopped'||e.phase.group===workerFilter)&&(!search||[workerLabel(e.worker),e.worker.id,e.device.machine.host,e.device.machine.hostname,...(e.worker.config?.projects||[])].join(' ').toLowerCase().includes(search));
    function row(entry,label) {
      const {worker:w,device:d,phase}=entry,m=d.machine;
      const r=element('details','worker-record');r.dataset.key=m.host+':'+w.id;r.dataset.worker=w.id;r.open=expanded.has(r.dataset.key);
      const summary=element('summary','worker-record-summary');
      const identity=element('div','worker-record-identity');identity.append(element('strong','',label));
      const state=element('span','worker-state is-'+phase.group,phase.label);
      const slots=capacityControls(m,w);
      summary.append(identity,state,slots);
      if(configuring&&!w.retiring){const b=element('button','button small','Edit');b.type='button';Object.assign(b.dataset,{editWorker:w.id,host:m.host,focus:m.host+':'+w.id+':edit'});b.setAttribute('aria-label','Edit '+workerLabel(w)+' on '+(m.hostname||m.host));summary.append(b);}
      else summary.append(element('span','worker-expand','Details'));
      r.append(summary);
      const body=element('div','worker-record-body');body.append(element('p',phase.group==='attention'?'worker-problem':'device-note',phase.note));
      const facts=element('dl','worker-facts');
      for(const [key,value] of [['Desired state',({running:'Pick up tasks',pause:'Paused',stop:'Stopped',drain:'Drain and stop'})[w.intent]||'Unknown'],['Worker ID',w.id],['Working directory',w.config?.directory||'Discovered per project'],...Object.entries(w.config?.directories||{}).map(([p,v])=>[projectLabel(p),v])]){facts.append(element('dt','',key),element('dd','',value));}body.append(facts);
      const tasks=element('div','worker-tasks');
      for(const run of [...(w.runs||[]),...(w.chiefs||[])].filter(r=>r.kind==='chief'?r.state==='running':r.finished_at==null)){const a=element('a','worker-task');a.href=link({machine:m,run});a.append(element('strong','',run.title||run.project_name||'Chief'),element('span','',run.summary||run.last_event||'Working'),element('small','',elapsed(run)));tasks.append(a);}
      if(tasks.childNodes.length)body.append(element('h4','','Current tasks'),tasks);
      const actions=element('div','device-actions');
      for(const [signal,label] of w.retiring?[]:w.pid>0?[[w.config?.enabled?'pause':'resume',w.config?.enabled?'Pause pickup':'Resume pickup'],['restart','Restart worker'],['stop','Stop worker']]:[['resume','Start worker']]){const b=element('button','button small',label);b.type='button';Object.assign(b.dataset,{signal,worker:w.id,host:m.host,focus:m.host+':'+w.id+':'+signal});actions.append(b);}
      if(!w.retiring&&!configuring){const b=element('button','button small','Edit settings');b.type='button';Object.assign(b.dataset,{editWorker:w.id,host:m.host});actions.append(b);}body.append(actions,removalActions(m,w));
      for(const signal of (data.signals||[]).filter(s=>s.host===m.host&&s.worker===w.id&&!['acknowledged','superseded'].includes(s.state)))body.append(element('p','device-note',signal.signal+': '+signal.state));
      r.append(body);return r;
    }
    const sections=[];
    for(const d of devices.sort((a,b)=>Number(!b.online||!!b.machine.configuration_error)-Number(!a.online||!!a.machine.configuration_error))){
      const own=entries.filter(e=>e.device===d),shown=own.filter(matches);
      const stopped=workerFilter==='current'&&!search?own.filter(e=>e.phase.group==='stopped'):[];
      if(!shown.length&&!stopped.length&&(search||workerFilter!=='current'))continue;
      const m=d.machine,section=element('section','worker-machine');section.dataset.host=m.host;
      const heading=element('div','worker-machine-heading');const title=element('div');title.append(element('h2','',m.hostname||m.host),element('p','',m.host==='local'?'Supervisor · owns fleet.yaml':m.host));
      const status=element('div','machine-status');status.append(element('span','machine-connection '+(d.online?'is-online':'is-offline'),d.online?'Connected':'Offline'),element('span','',!d.online?'Changes waiting for connection':m.configuration_error?'Configuration needs fixing':m.desired_revision===m.applied_revision?'Configuration applied':'Applying configuration'));heading.append(title,status);section.append(heading);
      const metrics=element('p','machine-metrics',d.online?d.live.length+' running workers · '+d.active+' active agents · '+own.filter(e=>!e.worker.retiring).length+' configured workers':own.length+' configured workers · last seen '+(m.heartbeat?new Date(m.heartbeat*1000).toLocaleTimeString([],{hour:'2-digit',minute:'2-digit'}):'unknown'));section.append(metrics,machineControls(m),machineProjects(m));
      if(m.configuration_error)section.append(configurationRecovery(m,expanded));
      const rank={attention:0,working:1,ready:2,paused:3,stopped:4};shown.sort((a,b)=>rank[a.phase.group]-rank[b.phase.group]||workerLabel(a.worker).localeCompare(workerLabel(b.worker)));
      for(const group of workerScopeGroups([...shown,...stopped])){
        const scope=element('details','worker-scope-group settings-scope');scope.dataset.key=m.host+':settings-scope:'+group.key;scope.open=expanded.has(scope.dataset.key)||!!search;
        const summary=element('summary','worker-scope-heading');summary.dataset.focus=scope.dataset.key;summary.append(element('h3','',scopeLabel(group)));
        const template=group.entries.find(e=>!e.worker.retiring)?.worker;
        if(template){const add=element('button','button small','Add worker configuration');add.type='button';Object.assign(add.dataset,{machineAction:'add',host:m.host,templateWorker:template.id,focus:m.host+':add:'+group.key});summary.append(add);}scope.append(summary);
        const current=group.entries.filter(e=>!stopped.includes(e)),archived=group.entries.filter(e=>stopped.includes(e));
        if(current.length){const labels=element('div','worker-column-labels');labels.append(element('span','','Worker'),element('span','','Status'),element('span','','Agent limit'),element('span','',''));scope.append(labels);for(const entry of current)scope.append(row(entry,group.labels.get(entry.worker.id)));}
        if(archived.length){const archive=element('details','stopped-workers');archive.dataset.key=scope.dataset.key+':stopped';archive.open=expanded.has(archive.dataset.key);archive.append(element('summary','',archived.length+' stopped '+(archived.length===1?'worker':'workers')));for(const entry of archived)archive.append(row(entry,group.labels.get(entry.worker.id)));scope.append(archive);}section.append(scope);
      }
      if(!shown.length&&!stopped.length)section.append(element('p','worker-empty','No workers configured.'));
      sections.push(section);
    }
    board.replaceChildren(...(sections.length?sections:[element('p','worker-empty','No workers match this filter.')]));
    if(focus)[...board.querySelectorAll('[data-focus]')].find(e=>e.dataset.focus===focus)?.focus({preventScroll:true});
  }

  function renderDevices(data) {
    const workersPage=route().has('workers');
    const open=workersPage||$('device-settings').open;
    const savedOpen=new Set([...$('device-list').querySelectorAll('details[open]')].map(d=>d.dataset.host));
    const project=workersPage||route().get('scope')==='all'?null:route().get('project');
    const devices=deviceView(workersPage?managedFleet(data):data,project);
    const projectName=id=>projects.find(p=>p.id===id)?.name||id.replace(/^named:/,'');
    $('device-help').textContent=(project?'Workers that can pick up tasks for '+projectName(project)+'.':'Workers across all projects.')+' Each worker manages its own agent slots. Controls below affect that worker, including all its projects.';
    const controls=[];
    function workerRow(worker,device) {
      const {machine,online}=device, running=worker.pid>0;
      const active=worker.active??(worker.runs||[]).filter(r=>r.finished_at==null).length;
      const name=worker.config?.name;
      const label=name&&!/^Worker \d+$/.test(name)?name:'Worker '+worker.id.slice(0,8);
      const row=element('div','device-row');row.dataset.worker=worker.id;
      const info=element('div','worker-info');
      const heading=element('div','worker-heading');heading.append(element('strong','worker-name',label));
      if(name&&!/^Worker \d+$/.test(name))heading.append(element('span','worker-id',worker.id.slice(0,8)));
      const state=!online?(running?'Last seen running':'Offline'):!running&&worker.retry_at>Date.now()/1000?'Retrying':worker.intent==='drain'?'Draining':worker.intent==='pause'?'Paused':!running?(worker.intent==='running'?'Starting':'Stopped'):!worker.config?.enabled?(active?'Draining':'Paused'):(active?'Working':'Idle');
      heading.append(element('span','worker-state',state));info.append(heading);
      const ids=worker.config?.projects||[];
      info.append(element('p','worker-projects',ids.length?ids.map(projectName).join(', '):'All projects'));
      const directories=Object.entries(worker.config?.directories||{});
      if(directories.length){
        for(const [project,path] of directories){
          const cwd=element('p','worker-directory');cwd.append(element('span','','Working directory · '+projectName(project)),element('code','',path));info.append(cwd);
        }
      }else{
        const cwd=element('p','worker-directory');cwd.append(element('span','','Working directory'),element('code','',worker.config?.directory||'Discovered per project'));info.append(cwd);
      }
      info.append(element('p','worker-capacity',running?(online?'':'Last known: ')+active+' active '+(active===1?'agent':'agents')+' · '+(worker.config?.concurrency||1)+' '+((worker.config?.concurrency||1)===1?'slot':'slots'):'No running agents'));
      row.append(info);
      const buttons=element('div','device-actions');
      const actions=worker.retiring?[]:running?[[worker.config?.enabled?'pause':'resume',worker.config?.enabled?'Pause pickup':'Resume pickup'],['restart','Restart worker'],['stop','Stop worker']]:[['resume','Start worker']];
      const hints={pause:'Finish current agents, then pause new task pickup.',resume:'Enable task pickup and start this worker if needed.',restart:'Stop this worker’s current agents and restart the worker.',stop:'Stop this worker and its current agents. It stays stopped until started again.'};
      for(const [action,text] of actions){
        const b=element('button','button small',text);b.type='button';b.title=hints[action];Object.assign(b.dataset,{signal:action,host:machine.host,worker:worker.id,focus:machine.host+':'+worker.id+':'+action});b.setAttribute('aria-label',text+' · '+label+' on '+(machine.hostname||machine.host));buttons.append(b);
      }
      row.append(buttons);
      if(workersPage){
        if(worker.error)row.append(element('p','device-note',worker.error));
        if(online&&running&&!active&&worker.config?.enabled&&worker.eligible===0)row.append(element('p','device-note','Waiting for eligible issues.'));
        const tasks=element('div','worker-tasks');
        for(const run of [...(worker.runs||[]),...(worker.chiefs||[])].filter(r=>r.kind==='chief'?r.state==='running':r.finished_at==null)){
          const task=element('a','worker-task');task.href=link({machine,run});
          task.append(element('strong','',run.title||run.project_name||'Chief'),element('span','',run.summary||run.last_event||'Working'),element('small','',elapsed(run)));tasks.append(task);
        }
        if(tasks.childNodes.length)row.append(tasks);
      }
      if(!online)row.append(element('p','device-note','Device disconnected. Actions will wait for it to reconnect.'));
      for(const signal of (data.signals||[]).filter(s=>s.host===machine.host&&s.worker===worker.id&&s.state!=='acknowledged'))row.append(element('p','device-note',`${signal.signal}: ${signal.state}${machine.state==='connected'?'':' · waiting for connection'}`));
      return row;
    }
    for(const device of devices){
      const {machine,online,live,saved,active,capacity}=device;
      const section=element('section','device-group');section.dataset.host=machine.host;
      const heading=element('div','device-heading');heading.append(element('h3','',machine.hostname||machine.host),element('span','device-connection',online?'Connected':'Disconnected'));section.append(heading);
      section.append(element('p','device-summary',online?live.length+' running '+(live.length===1?'worker':'workers')+' · '+active+' active '+(active===1?'agent':'agents')+' / '+capacity+' slots':'Showing last known worker state'));
      if(machine.configuration_error)section.append(element('p','error','Configuration: '+machine.configuration_error));
      else if(machine.desired_revision)section.append(element('p','device-summary',machine.desired_revision===machine.applied_revision?'Configuration applied':online?'Applying saved configuration…':'Saved · waiting for connection'));
      for(const worker of live)section.append(workerRow(worker,device));
      if(workersPage){for(const worker of saved)section.append(workerRow(worker,device));}
      else if(saved.length){const past=element('details','saved-workers');past.dataset.host=machine.host;past.open=savedOpen.has(machine.host);past.append(element('summary','',saved.length+' '+(online?'stopped':'last seen stopped')+' '+(saved.length===1?'worker':'workers')));for(const worker of saved)past.append(workerRow(worker,device));section.append(past);}
      if(!live.length&&!saved.length)section.append(element('p','device-note','No workers configured.'));
      controls.push(section);
    }
    const focus=document.activeElement?.dataset.focus;
    $('device-list').replaceChildren(...controls);$('device-settings').open=open;
    if(focus)[...$('device-list').querySelectorAll('[data-focus]')].find(e=>e.dataset.focus===focus)?.focus({preventScroll:true});
    $('device-settings').hidden=!workersPage&&(mobile||!controls.length);
  }
  function renderDetail(data) {
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
    selected=projectView(data).flatMap(p=>[...p.active,...p.history,...p.chiefs]).find(e=>resource?.entity==='agent'&&(e.machine.host===resource.host||e.machine.hostname===resource.host)&&e.run.id===resource.id);
    if(!selected&&resource?.entity==='agent'&&resource.project){
      selected=historical||{machine:(data.machines||[]).find(m=>m.host===resource.host||m.hostname===resource.host)||{host:resource.host,state:'disconnected'},run:{id:resource.id,project_id:resource.project,title:'Saved creator conversation',finished_at:1,state:'completed',standalone:true},online:false};
    }
    $('back').href=base+'#'+new URLSearchParams({project:route().get('project'),view:'conversations'});
    if(!selected){const issueNum=Number(route().get('issue')),proj=route().get('project');if(issueNum>0&&proj){$('session-issue').hidden=false;$('session-issue').href=(mobile?'/project-resource#':'/#')+new URLSearchParams({project:proj,issue:issueNum});$('session-issue').textContent='Issue #'+issueNum+' ↗';}$('session-title').textContent='Conversation unavailable';$('session-status').textContent=assignment?'No recorded conversation for this assignment is in recent activity. Return to Agents to browse available conversations.':'This agent is no longer in recent activity.';$('conversation-empty').textContent='The saved session could not be loaded.';$('takeover-open').hidden=true;$('steer-open').hidden=true;$('steering-updates').hidden=true;$('resume-panel').hidden=true;$('takeover-note').hidden=true;return;}
    const {run,machine}=selected;
    document.title=(run.title||'Conversation')+' · Hey Boss';
    $('session-title').textContent=run.kind==='chief'?'Chief · '+(run.project_name||'Organizing project'):run.title||'Preparing your task';
    $('session-context').textContent=runLabel(run)+' · '+(run.project_name||'Project')+' · '+(machine.hostname||machine.host);
    $('session-state').replaceChildren(stateBadge(selected));
    $('session-issue').hidden=!run.number;
    $('session-issue').href=(mobile?'/project-resource#':'/#')+new URLSearchParams({project:run.project_id,issue:run.number});$('session-issue').textContent='Issue #'+run.number+' ↗';
    $('session-status').textContent=run.state==='infrastructure_blocked'?infrastructureGuidance(run):!selected.online&&run.finished_at==null?'Device disconnected. Showing the conversation loaded so far.':run.finished_at!=null?'This conversation has ended.':'Live conversation · updates as the agent works';
    renderTakeover();
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
    button.hidden=selected.run.kind==='chief'||selected.run.standalone||Boolean(state?.stopped)||(runEnded()&&!state?.pending);
    button.disabled=takeoverBusy||steerBusy||!selected.online;
    button.textContent=state?.pending?'Check takeover':'Take over';
    $('steer-open').hidden=selected.run.kind==='chief'||selected.run.standalone||runEnded()||Boolean(state?.pending||state?.stopped)||selected.run.stop_requested===true;
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
  async function refresh() {
    if(refreshing)return;refreshing=true;
    try{render(await read('/api/fleet/status'));$('error').hidden=true;}catch(e){fail(e);if(last)render(last);}finally{refreshing=false;}
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
  $('refresh').onclick=async()=>{await refresh();if(detail)await loadConversation();};
  async function configRequest(body) {
    const response=await fetch('/api/fleet/configuration',{cache:'no-store',...(body?{method:'POST',headers:{'Content-Type':'application/json',...(csrf?{'X-Hey-Boss-CSRF':csrf}:{})},body:JSON.stringify(body)}:{})});
    const data=await response.json();if(!response.ok||data.ok===false)throw Error(data.error?.message||data.error||'Could not load configuration.');return data;
  }
  function configButtons(){const loaded=configRevision!==undefined;$('config-text').disabled=configBusy||!loaded;$('config-reload').disabled=configBusy;$('config-validate').disabled=configBusy||!loaded;$('config-save').disabled=configBusy||configPreview!==$('config-text').value||configOriginal===$('config-text').value;}
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
      dialog.onclick=e=>{if(e.target===dialog)finish(false);};
      dialog.oncancel=e=>{e.preventDefault();finish(false);};
      dialog.showModal();
      cancelBtn.focus();
    });
  }
  async function loadConfig(){
    if(configBusy)return;
    if(configRevision&&$('config-text').value!==configOriginal&&!(await confirmFleet('Discard unsaved YAML edits?','Reloading will replace your unsaved YAML edits with the saved file from the supervisor.','Discard and reload',true)))return;
    configBusy=true;configButtons();$('config-status').textContent='Loading configuration…';
    try{const data=await configRequest();configRevision=data.revision;configOriginal=data.text;$('config-text').value=data.text;configPreview=undefined;$('config-source').textContent=data.source;$('config-changes').replaceChildren();$('config-status').textContent=data.error?'File error: '+data.error:'Loaded from the supervisor. Changes are applied after saving.';}catch(error){$('config-status').textContent=error.message;}finally{configBusy=false;configButtons();}
  }
  $('worker-filters').onclick=event=>{const b=event.target.closest('button');if(!b)return;if(b.dataset.activityFilter)activityFilter=b.dataset.activityFilter;else if(b.dataset.filter)workerFilter=b.dataset.filter;if(last)renderWorkerBoard(last);};
  $('worker-search').value=workerSearch;if(workerSearch)workerFilter='all';
  $('worker-search').oninput=event=>{workerSearch=event.target.value;if(last)renderWorkerBoard(last);};
  function workerEditButtons(){for(const input of $('worker-form').querySelectorAll('input,textarea,select'))input.disabled=workerEditBusy;$('worker-editor-preview').disabled=workerEditBusy;$('worker-editor-save').disabled=workerEditBusy||!workerEditPreview;$('worker-editor-cancel').disabled=workerEditBusy;}
  const editFields=['name','provider','slots','intent','projects','directory','directories'];
  let workerEditOriginal='';
  const editFingerprint=()=>JSON.stringify(editFields.map(field=>$('worker-'+field).value));
  async function openWorkerEditor(host,id){
    if(workerEditBusy)return;
    if(configRevision&&$('config-text').value!==configOriginal){fail(Error('Save or reload your unsaved YAML changes before editing a worker.'));return;}
    workerEditBusy=true;
    try{
      const data=await configRequest();
      const worker=data.document?.machines?.[host]?.workers?.find(w=>w.id===id);
      if(!worker)throw Error(data.error||'This worker is no longer configured. Reload the page.');
      workerEdit={host,id,worker};$('worker-scope').open=!!last?.machines?.find(m=>m.host===host)?.configuration_error?.includes(id);workerEditRevision=data.revision;workerEditPreview=undefined;
      $('worker-editor-title').textContent='Edit '+workerLabel(worker);
      $('worker-editor-context').textContent=host+' · '+id;
      $('worker-name').value=worker.config?.name||'Worker';$('worker-provider').value=worker.config?.provider||'codex';$('worker-slots').value=worker.config?.concurrency||1;$('worker-intent').value=worker.intent;
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
    return {host:workerEdit.host,id:workerEdit.id,intent:$('worker-intent').value,config:{provider:$('worker-provider').value,name:$('worker-name').value.trim(),concurrency:Number($('worker-slots').value),projects:$('worker-projects').value.split('\n').map(s=>s.trim()).filter(Boolean),directory:$('worker-directory').value.trim(),directories}};
  }
  $('worker-form').onsubmit=async event=>{
    event.preventDefault();if(workerEditBusy)return;workerEditBusy=true;workerEditButtons();
    try{const update=workerUpdate();const data=await configRequest({worker_update:update,revision:workerEditRevision,save:false});workerEditPreview=update;
      const before=workerEdit.worker;const changes=[];for(const [key,label] of [['name','Name'],['provider','Agent'],['concurrency','Agent limit'],['projects','Projects'],['directory','Working directory'],['directories','Per-project paths']]){const defaults={provider:'codex',name:'Worker',concurrency:1,projects:[],directory:'',directories:{}};const old=before.config?.[key]??defaults[key];if(JSON.stringify(old)!==JSON.stringify(update.config[key]))changes.push(label+': '+(typeof old==='object'?JSON.stringify(old):old||'Automatic')+' → '+(typeof update.config[key]==='object'?JSON.stringify(update.config[key]):update.config[key]||'Automatic'));}if(before.intent!==update.intent)changes.unshift('Pickup mode: '+before.intent+' → '+update.intent);
      $('worker-editor-changes').replaceChildren(...changes.map(text=>element('li','',text)));$('worker-editor-status').textContent=changes.length?(update.intent==='stop'&&before.intent!=='stop'?'Saving will stop this worker and its current agents.':'Ready to save to fleet.yaml. Machines will apply these settings automatically.'):'No changes to save.';if(!changes.length)workerEditPreview=undefined;$('worker-editor-changes').scrollIntoView({block:'nearest'});
    }catch(error){workerEditPreview=undefined;$('worker-editor-status').textContent=error.message;}finally{workerEditBusy=false;workerEditButtons();}
  };
  $('worker-editor-save').onclick=async()=>{
    if(workerEditBusy||!workerEditPreview)return;workerEditBusy=true;workerEditButtons();
    try{const saved=await configRequest({worker_update:workerEditPreview,revision:workerEditRevision,save:true});workerEditOriginal=editFingerprint();workerEditPreview=undefined;
      if(configRevision){configRevision=saved.revision;configOriginal=saved.text;$('config-text').value=saved.text;configPreview=undefined;configButtons();}
      $('worker-save-note').textContent='Worker saved. Check its machine below for application status.';$('worker-save-note').hidden=false;await refresh();$('worker-editor').close();workerEdit=null;
    }catch(error){workerEditPreview=undefined;$('worker-editor-status').textContent=error.message;}finally{workerEditBusy=false;workerEditButtons();}
  };
  let machineEdit=null,machineBusy=false,machinePathEdited=false;
  function machineButtons(){for(const input of $('machine-form').elements)input.disabled=machineBusy;}
  async function openMachineEditor(host,action,workerId,projectId,templateId){
    if(machineBusy)return;
    if(configRevision&&$('config-text').value!==configOriginal){fail(Error('Save or reload your unsaved YAML changes first.'));return;}
    machineBusy=true;
    try{
      const data=await configRequest(),machine=data.document?.machines?.[host]||(last?.machines?.some(m=>m.host===host)?{workers:[]}:null);
      if(!machine)throw Error('Machine is no longer configured. Reload the page.');
      const template=machine.workers?.find(w=>w.id===templateId&&!w.retiring);
      const scope=template?JSON.stringify([...(template.config?.projects||[])].sort()):null;
      const workers=(machine.workers||[]).filter(w=>!w.retiring&&(!scope||JSON.stringify([...(w.config?.projects||[])].sort())===scope));
      if(action==='edit-project'&&!machine.projects?.[projectId])throw Error('Project is no longer configured. Reload the page.');
      machineEdit={host,action,projectId,revision:data.revision,id:HeyBossUI.requestId()};machinePathEdited=false;
      const name=last?.machines?.find(m=>m.host===host)?.hostname||host;
      $('machine-editor-context').textContent=name;
      $('machine-editor-title').textContent=({add:'Add a worker',remove:'Remove a worker',project:'Add a project','edit-project':'Edit checkout'})[action];
      $('machine-editor-description').textContent=({add:'Choose its settings. The new worker will start picking up tasks.',remove:'This worker will stop picking up tasks and finish its current work. You can kill it immediately while it is finishing.',project:'Set up a checkout on this machine and choose which worker can use it.','edit-project':'Update the repository URL or checkout path. Saved changes apply automatically.'})[action];
      for(const field of ['template','slots','worker','git','workspace','path'])$('machine-'+field+'-field').hidden=!({add:['template','slots'],remove:['worker'],project:['worker','git','workspace','path'],'edit-project':['git','workspace','path']}[action].includes(field));
      $('machine-git').required=['project','edit-project'].includes(action);$('machine-workspace').required=['project','edit-project'].includes(action);$('machine-path').required=false;
      const options=(select,empty)=>{select.replaceChildren();if(empty){const option=element('option','',empty);option.value='';select.append(option);}for(const worker of workers){const option=element('option','',workerLabel(worker)+' · '+worker.id.slice(0,12));option.value=worker.id;select.append(option);}};
      options($('machine-template'),template?null:Object.keys(machine.projects||{}).length?'New settings · machine projects':'New settings · all projects');options($('machine-worker'),action==='project'?'Machine only · assign later':null);
      if(action==='project'&&!workers.length){$('machine-worker-field').hidden=true;$('machine-editor-description').textContent='Set up the checkout, then add a worker and choose its agent limit.';}
      if(template){$('machine-template').value=template.id;$('machine-slots-field').hidden=true;$('machine-editor-context').textContent=scopeLabel({projects:template.config?.projects||[]})+' · '+name;}
      if(action==='add'&&projectId){$('machine-template-field').hidden=true;$('machine-template').value='';$('machine-editor-context').textContent=projectLabel(projectId)+' · '+name;}
      if(workerId)$('machine-worker').value=workerId;
      $('machine-slots').value='1';$('machine-workspace').value=machine.workspace||'~/Workspace';$('machine-git').value='';$('machine-path').value='';
      if(projectId&&machine.projects?.[projectId]){const project=machine.projects[projectId];$('machine-git').value=project.git;$('machine-path').value=project.reuse_existing?'':project.path;machinePathEdited=!project.reuse_existing;}
      $('machine-editor-status').textContent=last?.machines?.find(m=>m.host===host)?.state==='disconnected'?'This machine is offline. Changes apply when it reconnects.':'';
      $('machine-editor-save').textContent=({add:'Add worker',remove:'Finish & remove',project:'Save project','edit-project':'Save & retry'})[action];
      suggestCheckout();$('machine-editor').showModal();
    }catch(error){fail(error);}finally{machineBusy=false;machineButtons();}
  }
  $('machine-template').onchange=()=>{$('machine-slots-field').hidden=!!$('machine-template').value;};
  function suggestCheckout(){if(machinePathEdited)return;const git=$('machine-git').value.trim().replace(/\/+$|\.git$/g,'');const name=git.split(/[/:]/).pop();$('machine-path').placeholder=name?'Automatic · '+$('machine-workspace').value.replace(/\/+$/,'')+'/'+name:'Automatic';}
  $('machine-git').oninput=suggestCheckout;$('machine-workspace').oninput=suggestCheckout;$('machine-path').oninput=()=>{machinePathEdited=!!$('machine-path').value;};
  $('machine-editor-cancel').onclick=()=>{if(!machineBusy){$('machine-editor').close();machineEdit=null;}};
  $('machine-editor').addEventListener('cancel',event=>{if(machineBusy)event.preventDefault();else machineEdit=null;});
  $('machine-form').onsubmit=async event=>{
    event.preventDefault();if(machineBusy||!machineEdit)return;machineBusy=true;machineButtons();$('machine-editor-status').textContent='Saving…';
    try{
      const {host,action,id,revision,projectId}=machineEdit;let update={host,action};
      if(action==='add')Object.assign(update,{id,concurrency:Number($('machine-slots').value),...(projectId?{project:projectId}:$('machine-template').value?{template:$('machine-template').value}:{})});
      if(action==='remove')update.id=$('machine-worker').value;
      if(['project','edit-project'].includes(action))Object.assign(update,{git:$('machine-git').value.trim(),workspace:$('machine-workspace').value.trim(),path:$('machine-path').value.trim(),...(action==='edit-project'?{project:projectId}:{worker:$('machine-worker').value})});
      const saved=await configRequest({machine_update:update,revision,save:true});
      if(configRevision){configRevision=saved.revision;configOriginal=saved.text;$('config-text').value=saved.text;configPreview=undefined;configButtons();}
      $('worker-save-note').textContent=action==='remove'?'Worker is finishing its current work. Expand it to kill it now.':['project','edit-project'].includes(action)?'Project saved. Its machine will prepare the checkout and apply worker settings.':'Worker added. Its machine will start it automatically.';$('worker-save-note').hidden=false;
      if(action==='project'){
        const machine=saved.document?.machines?.[host],project=Object.entries(machine?.projects||{}).find(([,p])=>p.git===update.git)?.[0];
        if(project&&!projectHasWorker(machine,project)){
          $('worker-save-note').textContent='Project saved. Add a worker to start agents. ';
          const next=element('button','button small','Add worker');next.type='button';next.onclick=()=>openMachineEditor(host,'add',null,project);$('worker-save-note').append(next);
        }
      }
      if(action==='edit-project')await configRequest({retry_project:{host,project:projectId}});
      $('machine-editor').close();machineEdit=null;await refresh();
    }catch(error){$('machine-editor-status').textContent=error.message;}finally{machineBusy=false;machineButtons();}
  };
  $('worker-board').onclick=async event=>{
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
      if(configRevision&&$('config-text').value!==configOriginal){fail(Error('Save or reload your unsaved YAML changes first.'));return;}
      capacityBusy=true;for(const b of $('worker-board').querySelectorAll('[data-capacity-delta]'))b.disabled=true;
      try{
        const data=await configRequest(),{host,worker,capacityDelta}=capacity.dataset;
        const update=workerCapacityUpdate(data.document,host,worker,Number(capacityDelta));
        const key=host+':'+worker;capacityRequests.set(key,{limit:update.config.concurrency,saved:false});if(last)render(last);
        const saved=await configRequest({worker_update:update,revision:data.revision,save:true});
        capacityRequests.set(key,{limit:update.config.concurrency,saved:true});
        if(configRevision){configRevision=saved.revision;configOriginal=saved.text;$('config-text').value=saved.text;configPreview=undefined;configButtons();}
        $('worker-save-note').hidden=true;
        await refresh();
      }catch(error){capacityRequests.delete(capacity.dataset.host+':'+capacity.dataset.worker);fail(error);}finally{capacityBusy=false;if(last)render(last);}return;
    }
    const machine=event.target.closest('[data-machine-action]');if(machine){event.preventDefault();await openMachineEditor(machine.dataset.host,machine.dataset.machineAction,machine.dataset.removeWorker,machine.dataset.project,machine.dataset.templateWorker);return;}
    const edit=event.target.closest('[data-edit-worker]');if(edit){event.preventDefault();await openWorkerEditor(edit.dataset.host,edit.dataset.editWorker);return;}
    const b=event.target.closest('button[data-signal]');if(!b)return;
    if(['stop','restart'].includes(b.dataset.signal)&&!(await confirmFleet((b.dataset.signal==='stop'?'Stop':'Restart')+' this worker?','This will '+(b.dataset.signal==='stop'?'stop':'restart')+' the worker and its currently running agents.',b.dataset.signal==='stop'?'Stop worker':'Restart worker',b.dataset.signal==='stop')))return;
    b.disabled=true;try{const response=await fetch('/api/fleet',{method:'POST',headers:{'Content-Type':'application/json',...(csrf?{'X-Hey-Boss-CSRF':csrf}:{})},body:JSON.stringify({kind:'signal',host:b.dataset.host,worker:b.dataset.worker,signal:b.dataset.signal,id:HeyBossUI.requestId()})});const data=await response.json();if(!response.ok||data.ok===false)throw Error(data.error?.message||data.error||'Could not apply this change.');await refresh();}catch(error){fail(error);}finally{b.disabled=false;}
  };

  $('config-editor').addEventListener('toggle',()=>{if($('config-editor').open&&configRevision===undefined)loadConfig();});
  $('config-reload').onclick=loadConfig;
  $('config-text').oninput=()=>{configPreview=undefined;$('config-changes').replaceChildren();$('config-status').textContent=$('config-text').value===configOriginal?'No unsaved changes.':'Unsaved changes. Preview before saving.';configButtons();};
  async function submitConfig(save){
    if(configBusy||!configRevision)return;const text=$('config-text').value;
    configBusy=true;configButtons();$('config-status').textContent=save?'Saving configuration…':'Checking configuration…';
    try{const data=await configRequest({text,revision:configRevision,save});
      if(save){configRevision=data.revision;configOriginal=data.text;configPreview=undefined;$('config-changes').replaceChildren();$('config-status').textContent='Saved. Machines will apply this configuration automatically.';await refresh();}
      else{configPreview=text;$('config-changes').replaceChildren(...(data.changes||[]).map(change=>element('li','',({add:'Add',update:'Update',drain:'Drain and remove'})[change.action]+' '+change.worker+' on '+change.host)));$('config-status').textContent=data.changes?.length?'Valid YAML. Review the changes below.':'Valid YAML. No worker changes.';}
    }catch(error){configPreview=undefined;$('config-status').textContent=error.message;}finally{configBusy=false;configButtons();}
  }
  $('config-validate').onclick=()=>submitConfig(false);$('config-save').onclick=()=>submitConfig(true);
  addEventListener('beforeunload',event=>{if(configRevision&&$('config-text').value!==configOriginal||workerEdit&&editFingerprint()!==workerEditOriginal){event.preventDefault();event.returnValue='';}});
  $('device-list').onclick=async event=>{
    const b=event.target.closest('button[data-signal]');if(!b)return;
    if(b.dataset.signal==='stop'&&!(await confirmFleet('Stop agents on this device?','Currently running agents on this device will stop. Their saved conversations will remain available.','Stop agents',true)))return;
    b.disabled=true;
    try{const response=await fetch('/api/fleet',{method:'POST',headers:{'Content-Type':'application/json','X-Hey-Boss-CSRF':csrf},body:JSON.stringify({kind:'signal',host:b.dataset.host,worker:b.dataset.worker,signal:b.dataset.signal,id:HeyBossUI.requestId()})});const data=await response.json();if(!response.ok||data.ok===false)throw Error(data.error?.message||data.error||'Could not apply this change.');await refresh();}catch(e){fail(e);}finally{b.disabled=false;}
  };
  addEventListener('hashchange',()=>{if(route().has('find')){workerSearch=route().get('find');$('worker-search').value=workerSearch;workerFilter='all';}if(detail){keepSteerDraft();$('steer-dialog').close();$('steer-note').hidden=true;$('steering-updates').hidden=true;$('steering-list').replaceChildren();historical=null;$('session-resources').hidden=true;follow=!route().has('at');$('takeover-dialog').close();$('copy-status').textContent='';generation++;cursor=0;olderCursor=0;loading=false;loaded=false;seen.clear();$('conversation').replaceChildren();}context();});
  addEventListener('pagehide',()=>{disposed=true;generation++;});
  addEventListener('pageshow',event=>{if(event.persisted){disposed=false;loading=false;refresh();if(detail)loadConversation();}});
  (async()=>{try{
    const data=await read(mobile?'/api/agent-bootstrap':'/api/bootstrap');csrf=data.csrf;projects=data.projects||[];defaultProject=data.project||projects[0];context();
    if(mobile){$('quick-issue-open').hidden=true;$('nav-inbox').href='/';$('nav-issues').href='/#issues';$('nav-mindmaps').hidden=true;}
    await refresh();
    if(!mobile){const events=new EventSource('/api/fleet/events');events.addEventListener('connected',()=>refresh());events.onmessage=()=>refresh();events.onerror=()=>{$('connection').classList.add('offline');$('connection').querySelector('span').textContent='Reconnecting…';};events.onopen=()=>{$('connection').classList.remove('offline');$('connection').querySelector('span').textContent='Connected';};}
  }catch(e){fail(e);}})();
  setInterval(()=>{if(!document.hidden&&!disposed){refresh();if(detail){if(!route().has('at'))loadConversation();const state=selected&&savedTakeover(selected);if(selected?.online&&state?.pending&&!state.error)takeOver(selected);}}},3000);
  setInterval(()=>{if(!document.hidden&&!disposed){const now=Date.now();for(const time of document.querySelectorAll('.agent-runtime[data-started-at]'))time.textContent=elapsed({started_at:Number(time.dataset.startedAt)},now);}},1000);
  document.addEventListener('visibilitychange',()=>{if(!document.hidden){refresh();if(detail)loadConversation();}});
})();
