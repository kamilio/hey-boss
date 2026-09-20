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
  const seconds = Math.max(0, Math.floor(((run.finished_at ?? now) - (run.started_at ?? now)) / 1000));
  const hours = Math.floor(seconds / 3600), minutes = Math.floor(seconds / 60) % 60;
  return `${hours ? hours + "h" : ""}${hours ? String(minutes).padStart(2, "0") : minutes}m${String(seconds % 60).padStart(2, "0")}s`;
}
// An agent belongs to its task's project, regardless of its machine or scheduler.
function projectView(data, now = Date.now()) {
  const groups = new Map();
  for (const machine of data.machines || []) {
    const online = machine.state === 'connected' && now / 1000 - (machine.heartbeat || 0) <= 15;
    for (const worker of machine.workers || []) for (const run of worker.runs || []) {
      const id = run.project_id;
      if (!groups.has(id)) groups.set(id, {id, name: run.project_name || id || 'Project', active: [], history: []});
      const entry = {run, machine, worker, online: online && worker.pid > 0};
      groups.get(id)[run.finished_at == null ? 'active' : 'history'].push(entry);
    }
  }
  return [...groups.values()].sort((a,b) => Number(b.active.length > 0) - Number(a.active.length > 0) || a.name.localeCompare(b.name));
}
function agentState(entry) {
  if (!entry.online && entry.run.finished_at == null) return 'Last seen';
  return ({running:'Working',reserved:'Starting',starting:'Starting',completed:'Completed',blocked:'Needs attention',needs_input:'Needs your answer',approval_required:'Needs approval',interrupted:'Interrupted',failed:'Needs attention',stopped:'Stopped',cancelled:'Stopped',unclaimed:'Not started',timed_out:'Interrupted'})[entry.run.state] || 'Working';
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
function deviceView(data, project, now = Date.now()) {
  return (data.machines || []).map(machine => {
    const workers = (machine.workers || []).filter(w => !project || !w.config?.projects?.length || w.config.projects.includes(project));
    const online = machine.state === 'connected' && now / 1000 - (machine.heartbeat || 0) <= 15;
    const live = workers.filter(w => w.pid > 0), saved = workers.filter(w => !(w.pid > 0));
    return {machine, online, live, saved,
      active: online ? live.reduce((n,w) => n + (w.runs || []).filter(r => r.finished_at == null).length, 0) : 0,
      capacity: online ? live.reduce((n,w) => n + (w.config?.concurrency || 1), 0) : 0};
  }).filter(d => !project || d.live.length || d.saved.length);
}
if (typeof module !== 'undefined') module.exports = {fleetView, elapsed, projectView, agentState, deviceView, assignedAgentEntry};
if (typeof document !== 'undefined') (() => {
  const $ = id => document.getElementById(id);
  const element = (tag, cls, text) => {const e=document.createElement(tag);if(cls)e.className=cls;if(text!==undefined)e.textContent=text;return e;};
  const mobile = document.documentElement.dataset.agentMobile === 'true';
  const detail = location.pathname.endsWith('/session');
  const base = '/agents';
  const route = () => new URLSearchParams(location.hash.slice(1));
  let projects=[], defaultProject, csrf, last, refreshing=false, disposed=false;
  let cursor=0, olderCursor=0, loading=false, loaded=false, generation=0, follow=true, selected;
  const seen = new Set();
  let takeoverBusy=false, takeoverTarget;
  const takeovers=new Map();
  const takeoverKey=entry=>entry.machine.host+':'+entry.run.id;
  function savedTakeover(entry) {
    const key=takeoverKey(entry);
    if(!takeovers.has(key)){try{const saved=JSON.parse(sessionStorage.getItem('takeover:'+key));if(saved)takeovers.set(key,saved);}catch{}}
    return takeovers.get(key);
  }
  function saveTakeover(entry,value) {const key=takeoverKey(entry);takeovers.set(key,value);try{sessionStorage.setItem('takeover:'+key,JSON.stringify(value));}catch{}}
  HeyBossUI.icons();
  const picker = new HeyBossUI.ProjectPicker({onSelect(project){location.href=base+'#'+new URLSearchParams({project});}});
  function context() {
    if(!defaultProject)return;
    const id=route().get('project')||defaultProject.id;
    picker.update(projects, projects.find(p=>p.id===id)||defaultProject);
    if(last)render(last);
  }
  const link = (entry) => base+'/session#'+new URLSearchParams({project:entry.run.project_id,host:entry.machine.host,run:entry.run.id});
  const stateBadge = entry => element('span','agent-state '+(entry.online&&entry.run.finished_at==null?'is-live':entry.run.state==='completed'?'is-done':'is-quiet'),agentState(entry));
  function card(entry, history=false) {
    const {run,machine}=entry;
    const a=element('a',history?'agent-card history-card':'agent-card');a.href=link(entry);a.dataset.focus=machine.host+':'+run.id;
    const top=element('div','agent-card-top');top.append(stateBadge(entry),element('span','location-label',machine.hostname||machine.host));
    const title=element('h3','',run.title||'Preparing your task');
    const activity=run.last_event||'';
    const preview=element('p','agent-preview',run.summary||(/^(Goal:|Codex session|\/goal)/.test(activity)?'Making progress on this task.':/^(\/bin\/|.* -lc )/.test(activity)?'Checking changes and running commands.':activity)||(entry.online?'Getting started…':'Reconnect to see the latest activity.'));
    const bottom=element('div','agent-card-bottom');bottom.append(element('span','',`Issue #${run.number}`),element('span','open-conversation',history?'Read conversation →':'Open conversation →'));
    a.append(top,title,preview,bottom);return a;
  }
  function renderOverview(data) {
    const focus=document.activeElement?.dataset.focus;
    const open=new Set([...$('projects').querySelectorAll('details[open]')].map(d=>d.dataset.section));
    const filter=route().get('project');
    const groups=projectView(data).filter(p=>!filter||p.id===filter);
    $('show-all').hidden=!filter;
    $('overview-note').textContent=groups.some(p=>p.active.length)?'A little closer to done. See what’s moving.':'Your projects, and the work behind them.';
    const sections=groups.map(group=>{
      const section=element('section','project-section');
      const heading=element('div','project-heading');const title=element('div');
      title.append(element('h2','',group.name),element('p','',group.active.length?group.active.some(e=>e.online)?'In progress':'Waiting for a connection':'Recent work'));
      const issues=element('a','project-issues','View issues →');issues.href=(mobile?'/#issues&':'/#')+new URLSearchParams({project:group.id});
      heading.append(title,issues);section.append(heading);
      const grid=element('div','agent-grid');for(const entry of group.active)grid.append(card(entry));section.append(grid);
      if(group.history.length){const history=element('details','project-history');history.dataset.section=group.id;history.append(element('summary','','Completed & earlier conversations'));const past=element('div','agent-grid');for(const entry of group.history)past.append(card(entry,true));history.append(past);history.open=open.has(group.id);section.append(history);}
      return section;
    });
    if(!sections.length){const empty=element('section','agents-empty');empty.append(element('div','empty-orbit','✧'),element('h2','','Room for your next idea'),element('p','','When an agent picks up a task, its conversation will appear here.'));sections.push(empty);}
    $('projects').replaceChildren(...sections);
    if(focus)[...$('projects').querySelectorAll('[data-focus]')].find(e=>e.dataset.focus===focus)?.focus({preventScroll:true});
    renderDevices(data);
  }
  function renderDevices(data) {
    const open=$('device-settings').open;
    const savedOpen=new Set([...$('device-list').querySelectorAll('details[open]')].map(d=>d.dataset.host));
    const project=route().get('project');
    const devices=deviceView(data,project);
    const projectName=id=>projects.find(p=>p.id===id)?.name||id.replace(/^named:/,'');
    $('device-help').textContent=(project?'Workers that can pick up tasks for '+projectName(project)+'.':'Workers across all projects.')+' Each worker manages its own agent slots. Controls below affect that worker, including all its projects.';
    const controls=[];
    function workerRow(worker,device) {
      const {machine,online}=device, running=worker.pid>0;
      const active=(worker.runs||[]).filter(r=>r.finished_at==null).length;
      const name=worker.config?.name;
      const label=name&&!/^Worker \d+$/.test(name)?name:'Worker '+worker.id.slice(0,8);
      const row=element('div','device-row');row.dataset.worker=worker.id;
      const info=element('div','worker-info');
      const heading=element('div','worker-heading');heading.append(element('strong','worker-name',label));
      if(name&&!/^Worker \d+$/.test(name))heading.append(element('span','worker-id',worker.id.slice(0,8)));
      const state=!online?(running?'Last seen running':'Last seen stopped'):!running?'Stopped':!worker.config?.enabled?(active?'Draining':'Paused'):(active?'Working':'Idle');
      heading.append(element('span','worker-state',state));info.append(heading);
      const ids=worker.config?.projects||[];
      info.append(element('p','worker-projects',ids.length?ids.map(projectName).join(', '):'All projects'));
      const cwd=element('p','worker-directory');cwd.append(element('span','','Working directory'),element('code','',worker.config?.directory||'Not recorded'));info.append(cwd);
      info.append(element('p','worker-capacity',running?(online?'':'Last known: ')+active+' active '+(active===1?'agent':'agents')+' · '+(worker.config?.concurrency||1)+' '+((worker.config?.concurrency||1)===1?'slot':'slots'):'No running agents'));
      row.append(info);
      const buttons=element('div','device-actions');
      const actions=running?[[worker.config?.enabled?'pause':'resume',worker.config?.enabled?'Pause pickup':'Resume pickup'],['restart','Restart worker'],['stop','Stop worker']]:[['resume','Start worker']];
      const hints={pause:'Finish current agents, then pause new task pickup.',resume:'Enable task pickup and start this worker if needed.',restart:'Stop this worker’s current agents and restart the worker.',stop:'Stop this worker and its current agents. It stays stopped until started again.'};
      for(const [action,text] of actions){
        const b=element('button','button small',text);b.type='button';b.title=hints[action];Object.assign(b.dataset,{signal:action,host:machine.host,worker:worker.id,focus:machine.host+':'+worker.id+':'+action});b.setAttribute('aria-label',text+' · '+label+' on '+(machine.hostname||machine.host));buttons.append(b);
      }
      row.append(buttons);
      if(!online)row.append(element('p','device-note','Device disconnected. Actions will wait for it to reconnect.'));
      for(const signal of (data.signals||[]).filter(s=>s.host===machine.host&&s.worker===worker.id&&s.state!=='acknowledged'))row.append(element('p','device-note',`${signal.signal}: ${signal.state}${machine.state==='connected'?'':' · waiting for connection'}`));
      return row;
    }
    for(const device of devices){
      const {machine,online,live,saved,active,capacity}=device;
      const section=element('section','device-group');section.dataset.host=machine.host;
      const heading=element('div','device-heading');heading.append(element('h3','',machine.hostname||machine.host),element('span','device-connection',online?'Connected':'Disconnected'));section.append(heading);
      section.append(element('p','device-summary',online?live.length+' running '+(live.length===1?'worker':'workers')+' · '+active+' active '+(active===1?'agent':'agents')+' / '+capacity+' slots':'Showing last known worker state'));
      for(const worker of live)section.append(workerRow(worker,device));
      if(saved.length){const past=element('details','saved-workers');past.dataset.host=machine.host;past.open=savedOpen.has(machine.host);past.append(element('summary','',saved.length+' '+(online?'stopped':'last seen stopped')+' '+(saved.length===1?'worker':'workers')));for(const worker of saved)past.append(workerRow(worker,device));section.append(past);}
      if(!live.length&&!saved.length)section.append(element('p','device-note','No workers configured.'));
      controls.push(section);
    }
    const focus=document.activeElement?.dataset.focus;
    $('device-list').replaceChildren(...controls);$('device-settings').open=open;
    if(focus)[...$('device-list').querySelectorAll('[data-focus]')].find(e=>e.dataset.focus===focus)?.focus({preventScroll:true});
    $('device-settings').hidden=mobile||!controls.length;
  }
  function renderDetail(data) {
    const assignment = route().get('agent');
    if (assignment) {
      const entry = assignedAgentEntry(data, route());
      if (entry) history.replaceState(null, '', link(entry));
    }
    const resource=HeyBossRoutes.resolve();
    selected=projectView(data).flatMap(p=>[...p.active,...p.history]).find(e=>resource?.entity==='agent'&&e.machine.host===resource.host&&e.run.id===resource.id);
    $('back').href=base+(route().get('project')?'#'+new URLSearchParams({project:route().get('project')}):'');
    if(!selected){$('session-title').textContent='Conversation unavailable';$('session-status').textContent=assignment?'No recorded conversation for this assignment is in recent activity. Return to Agents to browse available conversations.':'This agent is no longer in recent activity.';$('takeover-open').hidden=true;$('resume-panel').hidden=true;$('takeover-note').hidden=true;return;}
    const {run,machine}=selected;
    document.title=(run.title||'Conversation')+' · Hey Boss';
    $('session-title').textContent=run.title||'Preparing your task';
    $('session-context').textContent=(run.project_name||'Project')+' · '+(machine.hostname||machine.host);
    $('session-state').replaceChildren(stateBadge(selected));
    $('session-issue').href=(mobile?'/#issues&':'/#')+new URLSearchParams({project:run.project_id,issue:run.number});$('session-issue').textContent='Issue #'+run.number+' ↗';
    $('session-status').textContent=!selected.online&&run.finished_at==null?'Device disconnected. Showing the conversation loaded so far.':run.finished_at!=null?'This conversation has ended.':'Live conversation · updates as the agent works';
    renderTakeover();
    if(!loaded&&!loading)loadConversation();
  }
  function render(data) {last=data;if(detail)renderDetail(data);else renderOverview(data);}
  async function read(path) {
    const response=await fetch(path,{cache:'no-store'});const data=await response.json();
    if(!response.ok||data.ok===false)throw Error(data.error?.message||data.error||'Could not connect. Try again.');return data;
  }
  function fail(error) {$('error').textContent=error.message;$('error').hidden=false;}
  function renderTakeover() {
    const state=savedTakeover(selected), button=$('takeover-open');
    button.hidden=Boolean(state?.stopped)||(runEnded()&&!state?.pending);
    button.disabled=takeoverBusy||!selected.online;
    button.textContent=state?.pending?'Check takeover':'Take over';
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
      const label=element('div','message-author',item.role==='user'?'You':'Codex');const body=element('div','message-body');
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
      const query={host:selected.machine.host,run:selected.run.id,cursor};
      if(earlier)query.before=olderCursor;else if(initial)query.latest=1;
      const oldHeight=document.documentElement.scrollHeight,oldScroll=scrollY;
      const data=await read('/api/fleet/conversation?'+new URLSearchParams(query));
      if(token!==generation||disposed)return;
      const entries=data.messages.filter(m=>!seen.has(m.id));
      const fragment=document.createDocumentFragment();
      for(const item of entries){seen.add(item.id);fragment.append(message(item));}
      if(earlier)$('conversation').prepend(fragment);else $('conversation').append(fragment);
      if(!earlier)cursor=data.cursor;loaded=true;
      if(initial||earlier){olderCursor=data.older_cursor||0;$('load-earlier').hidden=!data.has_earlier;}
      if(earlier)scrollTo(0,oldScroll+document.documentElement.scrollHeight-oldHeight);
      $('conversation-empty').hidden=seen.size>0;
      $('conversation-empty').textContent=data.availability==='waiting'?(selected.run.finished_at!=null?'Saved history is unavailable on this device.':'The agent is getting started. Its saved conversation will appear here.'):'No messages yet. This page will update as the agent works.';
      if(!earlier&&follow&&entries.length){$('conversation-end').scrollIntoView({behavior:'instant',block:'end'});}
      $('jump-live').hidden=follow||!seen.size;
      if(data.has_more&&!earlier)setTimeout(()=>loadConversation(),0);
    }catch(e){fail(e);}finally{if(token===generation){loading=false;$('load-earlier').disabled=false;$('conversation').setAttribute('aria-busy','false');}}
  }
  $('overview-page').hidden=detail;$('session-page').hidden=!detail;
  if(detail){document.body.classList.add('conversation-page');$('load-earlier').onclick=()=>{follow=false;loadConversation(true);};$('jump-live').onclick=()=>{follow=true;$('conversation-end').scrollIntoView({behavior:'smooth',block:'end'});$('jump-live').hidden=true;};addEventListener('scroll',()=>{follow=$('conversation-end').getBoundingClientRect().bottom<=innerHeight+160;$('jump-live').hidden=follow||!seen.size;},{passive:true});}
  $('takeover-open').onclick=()=>{
    if(!selected)return;
    if(savedTakeover(selected)?.pending){takeOver(selected);return;}
    takeoverTarget=selected;
    $('takeover-task').textContent='Issue #'+selected.run.number+' · '+(selected.run.title||'Preparing your task')+' · '+(selected.machine.hostname||selected.machine.host);
    $('takeover-dialog').showModal();$('takeover-cancel').focus();
  };
  $('takeover-cancel').onclick=()=>$('takeover-dialog').close();
  $('takeover-confirm').onclick=()=>{const entry=takeoverTarget;$('takeover-dialog').close();if(entry)takeOver(entry);};
  $('resume-copy').onclick=async()=>{const command=$('resume-command').textContent;try{await navigator.clipboard.writeText(command);$('copy-status').textContent='Command copied.';}catch{$('copy-status').textContent='Select the command and copy it with your keyboard.';const range=document.createRange();range.selectNodeContents($('resume-command'));const selection=getSelection();selection.removeAllRanges();selection.addRange(range);}};
  $('refresh').onclick=async()=>{await refresh();if(detail)await loadConversation();};
  $('device-list').onclick=async event=>{
    const b=event.target.closest('button[data-signal]');if(!b)return;
    if(b.dataset.signal==='stop'&&!confirm('Stop agents on this device? Their saved conversations will remain available.'))return;
    b.disabled=true;
    try{const response=await fetch('/api/fleet',{method:'POST',headers:{'Content-Type':'application/json','X-Hey-Boss-CSRF':csrf},body:JSON.stringify({kind:'signal',host:b.dataset.host,worker:b.dataset.worker,signal:b.dataset.signal,id:crypto.randomUUID()})});const data=await response.json();if(!response.ok||data.ok===false)throw Error(data.error?.message||data.error||'Could not apply this change.');await refresh();}catch(e){fail(e);}finally{b.disabled=false;}
  };
  addEventListener('hashchange',()=>{if(detail){$('takeover-dialog').close();$('copy-status').textContent='';generation++;cursor=0;olderCursor=0;loading=false;loaded=false;seen.clear();$('conversation').replaceChildren();}context();});
  addEventListener('pagehide',()=>{disposed=true;generation++;});
  addEventListener('pageshow',event=>{if(event.persisted){disposed=false;loading=false;refresh();if(detail)loadConversation();}});
  (async()=>{try{
    const data=await read(mobile?'/api/agent-bootstrap':'/api/bootstrap');csrf=data.csrf;projects=data.projects||[];defaultProject=data.project||projects[0];context();
    if(mobile){$('quick-issue-open').hidden=true;$('nav-inbox').href='/';$('nav-issues').href='/#issues';$('nav-mindmaps').hidden=true;}
    await refresh();
    if(!mobile){const events=new EventSource('/api/fleet/events');events.addEventListener('connected',()=>refresh());events.onmessage=()=>refresh();events.onerror=()=>{$('connection').classList.add('offline');$('connection').querySelector('span').textContent='Reconnecting…';};events.onopen=()=>{$('connection').classList.remove('offline');$('connection').querySelector('span').textContent='Connected';};}
  }catch(e){fail(e);}})();
  setInterval(()=>{if(!document.hidden&&!disposed){refresh();if(detail){loadConversation();const state=selected&&savedTakeover(selected);if(selected?.online&&state?.pending&&!state.error)takeOver(selected);}}},3000);
  document.addEventListener('visibilitychange',()=>{if(!document.hidden){refresh();if(detail)loadConversation();}});
})();
