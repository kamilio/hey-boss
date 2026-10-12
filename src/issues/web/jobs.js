"use strict";
const HeyBossJobs = (() => {
  const esc = value => String(value ?? '').replace(/[&<>"']/g,c=>({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));
  function summary(cron) {
    const [m,h,d,month,w] = cron.trim().split(/\s+/);
    if (/^\d+$/.test(m)&&/^\d+$/.test(h)&&d==='*'&&month==='*'&&['*','1-5','1'].includes(w)) return `${w==='*'?'Every day':w==='1'?'Mondays':'Weekdays'} at ${h.padStart(2,'0')}:${m.padStart(2,'0')}`;
    if(m==='*'&&h==='*'&&d==='*'&&month==='*'&&w==='*')return 'Every minute';
    if(m==='0'&&h==='*'&&d==='*'&&month==='*'&&w==='*')return 'Every hour';
    return `Minute ${m} · Hour ${h} · Day ${d} · Month ${month} · Weekday ${w}`;
  }
  function scheduleFields(cron) {
    const [minute,hour,day,month,week]=cron.trim().split(/\s+/);
    if(cron==='0 * * * *')return {frequency:'hourly',time:'09:00'};
    const frequency=/^\d+$/.test(minute)&&Number(minute)<60&&/^\d+$/.test(hour)&&Number(hour)<24&&day==='*'&&month==='*'?({'*':'daily','1-5':'weekdays','1':'weekly'}[week]||'custom'):'custom';
    return {frequency,time:frequency==='custom'?'09:00':hour.padStart(2,'0')+':'+minute.padStart(2,'0')};
  }
  function scheduleCron(frequency,time) {
    if(frequency==='hourly')return '0 * * * *';
    const [hour,minute]=time.split(':').map(Number);
    return minute+' '+hour+' * * '+({daily:'*',weekdays:'1-5',weekly:'1'}[frequency]);
  }
  function scheduleState(job) {return job.deleted_at?'Deleted':job.enabled?'Enabled':'Disabled';}
  function filterJobs(jobs,filter,query) {
    const search=query.trim().toLowerCase();
    return jobs.filter(job=>(filter==='all'||scheduleState(job).toLowerCase()===filter)&&[job.snapshot.definition.name,job.snapshot.definition.model,summary(job.snapshot.definition.cron)].some(value=>value.toLowerCase().includes(search)));
  }
  function modelChoices(catalog, saved) {
    const choices=(catalog?.models||[]).map(m=>({value:[m.selection.route,m.selection.id].filter(Boolean).join('/'),label:m.label+(m.advertised?'':' · Not advertised'),advertised:m.advertised}));
    if(saved&&!choices.some(m=>m.value===saved))choices.unshift({value:saved,label:saved+' · Not advertised',advertised:false});
    return choices;
  }
  function availability(machines, project, harness, now=Date.now()) {
    return machines.some(m=>m.state==='connected'&&now/1000-(m.heartbeat||0)<15&&m.jobs?.available&&m.jobs.checkouts?.[project]&&(!harness||m.jobs.runtimes?.[harness]?.available));
  }
  class Mutation {
    constructor(post, uuid=()=>HeyBossUI.requestId(), persist=()=>{}) {this.post=post;this.uuid=uuid;this.persist=persist;this.pending=null;}
    async send(operation, project, host) {
      if(this.pending)throw Error('Retry the unconfirmed request before making another change.');
      this.pending={project,operation:{action:'job',operation},request_id:this.uuid(),...(host?{host}:{})};this.persist(this.pending);
      return this.retry();
    }
    async retry() {
      const body=this.pending;
      if(!body)throw Error('No request to retry.');
      try {const result=await this.post(body);this.pending=null;this.persist(null);return result;}
      catch(e){if(['conflict','invalid_input','not_found','forbidden'].includes(e.code)){this.pending=null;this.persist(null);}throw e;}
    }
  }
  async function start() {
    const $=s=>document.querySelector(s), mobile=document.documentElement.dataset.issueMobile==='true';
    let boot,context,rows=[],next=null,selected=null,editor=null,generation=0,detailGeneration=0,modelGeneration=0,previewGeneration=0,previewTimer,refreshTimer,history=[],historyNext=null,busy=false,dirty=false,machines=[],catalogResult=null,filter='all';
    const date=(ms,zone,short=false)=>ms==null?'—':new Date(ms).toLocaleString(undefined,{timeZone:zone,month:'short',day:'numeric',...(short?{}:{year:'numeric'}),hour:'numeric',minute:'2-digit',timeZoneName:'short'});
    const message=text=>{$('#jobs-error').textContent=text||'';$('#jobs-error').hidden=!text;};
    const persist=value=>{try{value?sessionStorage.setItem('jobs-pending',JSON.stringify(value)):sessionStorage.removeItem('jobs-pending');}catch{}};
    async function post(body,path='/api/action',reconnect=true) {
      const response=await fetch(path,{method:'POST',headers:{'Content-Type':'application/json',...(!mobile?{'X-Hey-Boss-CSRF':boot.csrf}:{})},body:JSON.stringify(body),signal:AbortSignal.timeout(60000)});
      const value=await response.json();
      if(response.status===403&&!mobile&&reconnect){const fresh=await(await fetch('/api/bootstrap')).json();if(fresh.ok&&fresh.actor?.id===boot.actor?.id){boot.csrf=fresh.csrf;return post(body,path,false);}}
      if(!response.ok||value.ok===false){const e=Error(value.error?.message||value.error||'Could not complete the request');e.code=value.error?.code;throw e;}
      return value;
    }
    const mutation=new Mutation(body=>post(body),undefined,persist);
    try{mutation.pending=JSON.parse(sessionStorage.getItem('jobs-pending')||'null');}catch{}
    const read=(operation,ctx=context)=>post({project:ctx.project,operation:{action:'job',operation},...(ctx.host?{host:ctx.host}:{})});
    const hash=id=>new URLSearchParams({project:context.project,...(context.host?{host:context.host}:{}),...(id?{job:id}:{})});
    let calendarProject=null;
    const calendar=HeyBossCalendar.mount({root:$('#jobs-calendar'),read,edit:id=>{if(leave())location.hash=hash(id);},links:run=>`${run.task_number?`<a href="/${mobile?'issues':''}#${esc(new URLSearchParams({project:run.snapshot.project_id,issue:run.task_number,...(context.host?{host:context.host}:{})}))}">Task #${run.task_number} ↗</a>`:''}${run.session_id?`<a href="/agents/session#${esc(new URLSearchParams({project:run.snapshot.project_id,host:machines.find(m=>m.node===run.machine)?.host||run.machine||'local',run:'job:'+run.id}))}">Session ↗</a>`:''}`});
    const picker=new HeyBossUI.ProjectPicker({onSelect:project=>{if(leave())location.hash=new URLSearchParams({project,...(context.host?{host:context.host}:{}),...(!$('#jobs-calendar').hidden?{view:'calendar'}:{})});}});
    function leave(){return !dirty||confirm('Discard your unsaved job changes?');}
    function pendingUI(){
      $('#jobs-retry').hidden=!mutation.pending;
      for(const el of document.querySelectorAll('[data-mutation],#job-new,#job-empty-new'))el.disabled=busy||!!mutation.pending||!context?.project;
      $('#job-retry').disabled=busy;
    }
    function renderList(){
      const focus=document.activeElement?.closest('[data-job-toggle]'),focusedId=focus?.dataset.jobToggle;
      $('#jobs-count').textContent=rows.length?String(rows.length)+(next?' +':''):'';
      for(const button of document.querySelectorAll('[data-filter]')){
        button.setAttribute('aria-pressed',String(button.dataset.filter===filter));
        button.querySelector('span').textContent=filterJobs(rows,button.dataset.filter,'').length+(next?' +':'');
      }
      const visible=filterJobs(rows,filter,$('#jobs-search').value);
      $('#jobs-status').textContent=visible.length?'':rows.length?(next?'No matches in loaded jobs. Show more jobs to keep searching.':'No jobs match this view.'):$('#jobs-deleted').checked?'No jobs in this project.':'No jobs yet. Create one to schedule recurring work.';
      $('#jobs-list').innerHTML=visible.map(job=>{
        const d=job.snapshot.definition,state=scheduleState(job),run=job.active_run||job.last_run;
        const outcome=run?({succeeded:'Succeeded',failed:'Failed',pending:'Waiting',running:'Running',cancelled:'Stopped',skipped:'Skipped'}[run.state]||run.state):'Not run yet';
        return '<article class="job-row '+state.toLowerCase()+'"><a class="job-card" href="#'+esc(hash(job.id))+'" aria-current="'+(selected?.id===job.id)+'"><div class="job-identity"><span class="job-icon" data-icon="clock"></span><div><h2>'+esc(d.name)+'</h2><p>'+esc(d.harness)+' <span aria-hidden="true">·</span> '+esc(d.model)+'</p></div></div><div class="job-schedule"><strong>'+esc(summary(d.cron))+'</strong><span>'+esc(d.timezone)+'</span><small>'+(job.enabled&&job.next_at?'Next '+esc(date(job.next_at,d.timezone,true)):job.deleted_at?'History retained':'No upcoming runs')+'</small></div><div class="job-last-run"><span class="job-outcome '+esc(run?.state||'none')+'">'+esc(outcome)+'</span><small>'+(run?esc(date(run.started_at||run.scheduled_at,d.timezone,true)):'—')+'</small></div></a><div class="job-row-status">'+(job.deleted_at?'<span class="job-state deleted">Deleted</span>':'<button class="job-switch" type="button" role="switch" aria-checked="'+job.enabled+'" aria-label="'+esc((job.enabled?'Disable ':'Enable ')+d.name)+'" data-job-toggle="'+esc(job.id)+'" data-mutation><span class="job-switch-track" aria-hidden="true"></span><span>'+state+'</span></button>')+'</div></article>';
      }).join('');
      $('#jobs-more').hidden=!next;HeyBossUI.icons($('#jobs-list'));pendingUI();
      if(focusedId){const control=[...document.querySelectorAll('[data-job-toggle]')].find(el=>el.dataset.jobToggle===focusedId);control?.focus({preventScroll:true});}
    }
    async function load(reset=true){
      const seq=++generation,ctx={...context};
      try{const value=await read({command:'list',after:reset?null:next,limit:50,include_deleted:$('#jobs-deleted').checked},ctx);if(seq!==generation)return;
        rows=reset?value.jobs:[...rows,...value.jobs];next=value.next_cursor;renderList();
      }catch(e){if(seq===generation)message(e.message);}
    }
    async function service(){
      try{const value=await post({},'/api/jobs/runtime');machines=value.machines||[];const available=availability(machines,context.project);
        $('#jobs-service').textContent=available?'Ready to run':value.service_error||'No connected job service has this project checkout. Runs will wait.';$('#jobs-service').dataset.available=String(available);
      }catch(e){$('#jobs-service').textContent='Job service status unavailable · '+e.message;}
    }
    function field(id,label,value,extra=''){return `<label class="job-field ${extra.includes('wide')?'wide':''}" for="${id}">${label}<input id="${id}" value="${esc(value)}" ${extra.replace('wide','')}></label>`;}
    async function open(id){
      const seq=++detailGeneration;modelGeneration++;previewGeneration++;editor=null;dirty=false;message('');
      if(id==='new'){selected=null;renderEditor(null,'');renderList();return;}
      if(!id){selected=null;$('#job-panel').innerHTML='';renderList();return;}
      $('#job-panel').innerHTML='<p class="job-help" role="status">Loading job…</p>';
      try{const {job}=await read({command:'view',id});const revision=await read({command:'revision',id,revision:job.revision});if(seq!==detailGeneration)return;
        selected=job;renderEditor(job,revision.instructions.markdown);renderList();await loadHistory();
      }catch(e){if(seq===detailGeneration){message(e.message);$('#job-panel').innerHTML='<p class="job-help">Could not load this job. Refresh to retry.</p>';}}
    }
    function renderEditor(job,markdown){
      const deleted=!!job?.deleted_at,d=job?.snapshot.definition||{name:'',cron:'0 9 * * *',timezone:Intl.DateTimeFormat().resolvedOptions().timeZone||'UTC',harness:'codex',model:''};
      editor={job,markdown};
      $('#job-panel').innerHTML=`<div class="job-panel-heading"><div><h2 id="job-panel-title">${job?esc(d.name):'New job'}</h2><p class="job-panel-subtitle">${deleted?'Deleted · Instructions and history are preserved':job?'Revision '+job.revision+' · '+(job.enabled?'Runs '+esc(summary(d.cron).toLowerCase()):'No upcoming runs'):'Choose what runs and when.'}</p></div>${job?`<span class="job-badge ${deleted?'deleted':job.enabled?'enabled':'disabled'}">${deleted?'Deleted':job.enabled?'Enabled':'Disabled'}</span>`:''}</div>
       ${job?`<div class="job-section job-controls"><h3 class="job-section-title">RUN CONTROLS</h3><div class="job-actions">${!deleted?'<button class="job-switch" id="job-enabled-toggle" type="button" role="switch" aria-checked="'+job.enabled+'" aria-label="'+(job.enabled?'Disable job':'Enable job')+'" data-mutation><span class="job-switch-track" aria-hidden="true"></span>'+(job.enabled?'Enabled':'Disabled')+'</button><button class="button" id="job-run" type="button" data-mutation>Run now</button>':''}<button class="button danger" id="job-stop" type="button" data-mutation hidden>Stop current run</button></div><p class="job-help" id="job-run-status">Run now also works while the schedule is disabled.</p></div>`:''}
       <form id="job-form"><fieldset ${deleted?'disabled':''} style="border:0;padding:0;margin:0;min-width:0"><div class="job-fields">${field('job-name','Name',d.name,'required maxlength="256" wide')}<label class="job-field wide" for="job-project" ${job?'hidden':''}>Project<select id="job-project" ${job?'disabled':''}>${boot.projects.map(p=>`<option value="${esc(p.id)}" ${p.id===context.project?'selected':''}>${esc(p.name)}</option>`).join('')}</select>${job?'<small>A job stays with the project where it was created.</small>':''}</label><label class="job-field wide" for="job-markdown">Instructions <small>Used exactly as written</small><textarea id="job-markdown" required spellcheck="false" aria-label="Task instructions">${esc(markdown)}</textarea></label></div>
       <div class="job-section"><h3 class="job-section-title">Schedule</h3><div class="job-fields"><label class="job-field" for="job-frequency">Frequency<select id="job-frequency"><option value="hourly">Every hour</option><option value="daily">Every day</option><option value="weekdays">Weekdays</option><option value="weekly">Every Monday</option><option value="custom">Custom schedule</option></select></label>${field('job-timezone','Timezone',d.timezone,'required list="job-timezones"')}<label class="job-field" for="job-time" id="job-time-field">Time<input id="job-time" type="time"></label><label class="job-field wide" for="job-cron" id="job-cron-field">Cron expression<input id="job-cron" value="${esc(d.cron)}" required aria-describedby="job-cron-help"><small id="job-cron-help">Minute · hour · day · month · weekday</small></label></div><datalist id="job-timezones">${[...new Set(['UTC',...(Intl.supportedValuesOf?.('timeZone')||[])])].map(z=>`<option value="${esc(z)}"></option>`).join('')}</datalist></div><div id="job-preview" class="job-preview" aria-live="polite">Checking schedule…</div></div>
       <div class="job-section"><h3 class="job-section-title">Agent</h3><div class="job-fields"><label class="job-field" for="job-harness">Harness<select id="job-harness">${['codex','claude','pi'].map(h=>`<option ${h===d.harness?'selected':''}>${h}</option>`).join('')}</select></label><label class="job-field" for="job-model">Model<input id="job-model" list="job-models" value="${esc(d.model)}" required maxlength="256" autocomplete="off" aria-describedby="job-model-status"><datalist id="job-models"></datalist></label></div><p id="job-model-status" class="job-model-status" role="status">Discovering models…</p><button class="button" id="job-model-refresh" type="button">Refresh models</button>${!job?'<label class="job-toggle"><input id="job-enabled" type="checkbox" checked> Enable this schedule</label>':''}<p class="job-help">Overlapping occurrences are skipped. Disabling a job leaves its current run alone.</p></div>
       ${deleted?'':`<div class="job-actions"><button class="button primary" id="job-save" type="submit" data-mutation>${job?'Save changes':'Create job'}</button>${job?'<button class="button" id="job-reload" type="button">Reload saved</button>':''}<span class="job-spacer"></span>${job?'<button class="button danger" id="job-delete" type="button" data-mutation>Delete job</button>':''}</div>`}</fieldset></form>

       ${job?'<section class="job-section" aria-label="Run history"><div class="job-history-heading"><h3>Run history</h3><button class="icon-button" id="job-history-refresh" type="button" aria-label="Refresh run history"><span data-icon="refresh"></span></button></div><div id="job-history" aria-live="polite"></div><button class="button" id="job-history-more" type="button" hidden>Older runs</button></section>':''}`;
      HeyBossUI.icons($('#job-panel'));pendingUI();
      $('#job-form').oninput=()=>{dirty=true;};
      $('#job-form').onsubmit=save;
      for(const id of ['job-cron','job-timezone'])$('#'+id).oninput=()=>{dirty=true;clearTimeout(previewTimer);previewTimer=setTimeout(preview,300);};
      const frequency=$('#job-frequency'),time=$('#job-time'),fields=scheduleFields(d.cron);
      frequency.value=fields.frequency;time.value=fields.time;
      const scheduleVisibility=()=>{$('#job-cron-field').hidden=frequency.value!=='custom';$('#job-time-field').hidden=['custom','hourly'].includes(frequency.value);time.required=!$('#job-time-field').hidden;};
      const updateSchedule=()=>{dirty=true;scheduleVisibility();if(frequency.value!=='custom'&&time.value)$('#job-cron').value=scheduleCron(frequency.value,time.value);preview();};
      frequency.onchange=updateSchedule;time.oninput=updateSchedule;scheduleVisibility();
      $('#job-harness').onchange=()=>{dirty=true;discover();};
      $('#job-model-refresh').onclick=discover;
      $('#job-model').oninput=()=>{dirty=true;if(catalogResult)renderModels(catalogResult);};
      if(job){$('#job-reload')?.addEventListener('click',()=>{if(leave())open(job.id);});$('#job-delete')?.addEventListener('click',()=>{if(confirm('Delete this job? Its schedule will be disabled and run history retained. An active run must be stopped separately.'))act({command:'delete',id:job.id,if_revision:job.revision});});
        $('#job-enabled-toggle')?.addEventListener('click',()=>act({command:'set_enabled',id:job.id,if_revision:job.revision,enabled:!job.enabled}));
        $('#job-run')?.addEventListener('click',()=>act({command:'run_now',id:job.id}));
        $('#job-history-refresh').onclick=()=>loadHistory();$('#job-history-more').onclick=()=>loadHistory(false);
      }
      if(job?.active_run&&$('#job-stop')){$('#job-stop').hidden=false;$('#job-stop').onclick=()=>act({command:'stop',id:job.id,run_id:job.active_run.id});}
      if($('#job-history'))$('#job-history').textContent='Loading run history…';
      preview();if(!deleted)discover();else $('#job-model-status').textContent='Saved execution selection';
    }
    async function preview(){
      const seq=++previewGeneration,cron=$('#job-cron')?.value,zone=$('#job-timezone')?.value;if(!cron||!zone){if($('#job-preview'))$('#job-preview').textContent='Enter a cron expression and timezone.';return;}
      const after=Date.now();
      try{const value=await read({command:'preview',cron,timezone:zone,after,through:after+366*9*86400000,limit:3});if(seq!==previewGeneration||!$('#job-preview'))return;
        $('#job-preview').innerHTML=`<strong>${esc(summary(cron))}</strong><div class="job-help">${esc(zone)} · Next scheduled occurrences${selected&&!selected.enabled?' if enabled':''}</div><ol>${value.occurrences.map(ms=>`<li>${esc(date(ms,zone))}</li>`).join('')}</ol>`;
        if(!value.occurrences.length)$('#job-preview').innerHTML+='<p>No occurrences in the next nine years.</p>';
      }catch(e){if(seq===previewGeneration&&$('#job-preview'))$('#job-preview').textContent=e.message;}
    }
    async function discover(){
      const seq=++modelGeneration,harness=$('#job-harness').value,saved=$('#job-model').value;
      catalogResult=null;$('#job-model-status').textContent='Discovering '+harness+' models…';$('#job-models').innerHTML='';
      try{const value=await post({provider:harness,configured:saved?[saved]:[]},'/api/jobs/runtime');if(seq!==modelGeneration||!$('#job-model'))return;
        catalogResult=value;renderModels(value);
      }catch(e){if(seq===modelGeneration&&$('#job-model-status'))$('#job-model-status').textContent=e.message+' The current model is preserved.';}
    }
    function renderModels(value){
      const saved=$('#job-model').value,harness=$('#job-harness').value,options=modelChoices(value.catalog,saved);
      $('#job-models').innerHTML=options.map(m=>`<option value="${esc(m.value)}">${esc(m.label)}</option>`).join('');
      const unlisted=saved&&!options.some(m=>m.value===saved&&m.advertised);
      $('#job-model-status').textContent=[value.catalog?.error,unlisted?'Current model is not advertised. Its exact ID is preserved.':'Choose a discovered model or enter an exact custom ID.',`Discovered on ${value.machine}.`,availability(value.machines||[],context.project,harness)?'':'No connected service currently advertises this project and harness; runs may wait.'].filter(Boolean).join(' ');
    }
    async function save(event){
      event.preventDefault();if(busy||mutation.pending)return;
      const job=editor.job,definition={name:$('#job-name').value,cron:$('#job-cron').value,timezone:$('#job-timezone').value,harness:$('#job-harness').value,model:$('#job-model').value};
      const operation={command:job?'edit':'create',id:job?.id||HeyBossUI.requestId(),definition,markdown:$('#job-markdown').value,...(job?{if_revision:job.revision}:{enabled:$('#job-enabled').checked})};
      await act(operation,$('#job-project').value);
    }
    async function act(operation,project=context.project,retry=false){
      if(busy)return;const command=retry?mutation.pending?.operation?.operation?.command:operation?.command,focusedToggle=document.activeElement?.dataset?.jobToggle;if(dirty&&!retry&&!['create','edit','run_now','stop'].includes(operation.command)&&!leave())return;busy=true;pendingUI();message('');const ctx={...context,project};
      try{const result=retry?await mutation.retry():await mutation.send(operation,project,context.host);if(result.job)dirty=false;
        if(result.job&&(selected?.id===result.job.id||['create','edit'].includes(command))){selected=result.job;const destination='#'+new URLSearchParams({project:result.job.project_id,...(ctx.host?{host:ctx.host}:{}),job:result.job.id});if(location.hash!==destination){location.hash=destination;return;}await open(result.job.id);}
        else if(selected)await loadHistory();
        await load();
      }catch(e){message(e.code==='conflict'?(editor?'This job changed on another device. Your draft is preserved. Use Reload saved to review the latest revision before saving again.':'This job changed on another device. Refresh jobs before trying again.'):e.message);}
      finally{busy=false;pendingUI();if(focusedToggle){const control=[...document.querySelectorAll('[data-job-toggle]')].find(el=>el.dataset.jobToggle===focusedToggle);(control||document.querySelector('[data-filter][aria-pressed="true"]'))?.focus({preventScroll:true});}}
    }
    async function loadHistory(reset=true){
      const job=selected;if(!job)return;const seq=detailGeneration;
      try{const value=await read({command:'history',id:job.id,before:reset?null:historyNext,limit:20});if(seq!==detailGeneration)return;
        history=reset?value.runs:[...history,...value.runs];historyNext=value.next_cursor;
        // The active run can be older than a page of skipped overlapping occurrences.
        const latest=await read({command:'view',id:job.id});if(seq!==detailGeneration)return;
        if(latest.job.revision!==job.revision&&!dirty){await open(job.id);return;}
        const active=latest.job.active_run;
        if($('#job-stop')){$('#job-stop').hidden=!active;$('#job-stop').onclick=()=>act({command:'stop',id:job.id,run_id:active.id});}
        if($('#job-run-status'))$('#job-run-status').textContent=active?(active.reason||`${active.state==='running'?'Running':'Pending'} · Stop affects this execution only.`):job.deleted_at?'This job is deleted. Saved runs remain available.':'Run now also works while the schedule is disabled.';
        $('#job-history').innerHTML=history.length?`<ol class="job-history-list">${history.map(run=>`<li class="job-run"><div class="job-run-top"><span class="job-badge ${esc(run.state)}">${esc(run.state)}</span><time>${esc(date(run.scheduled_at,run.snapshot.definition.timezone,true))}</time></div>${run.reason?`<p>${esc(run.reason)}</p>`:''}<p><small>${esc(run.trigger==='manual'?'Run now':'Scheduled')} · Revision ${run.snapshot.revision} · ${esc(run.snapshot.definition.harness)} / ${esc(run.snapshot.definition.model)}</small></p><div class="job-run-links">${run.task_number?`<a href="/${mobile?'issues':''}#${esc(new URLSearchParams({project:job.project_id,issue:run.task_number,...(context.host?{host:context.host}:{})}))}">Task #${run.task_number} ↗</a>`:''}${run.session_id?`<a href="/agents/session#${esc(new URLSearchParams({project:job.project_id,host:machines.find(m=>m.node===run.machine)?.host||run.machine||'local',run:'job:'+run.id}))}">Session ↗</a>`:''}</div></li>`).join('')}</ol>`:'<p class="job-help">No runs yet. Your first execution will appear here.</p>';
        $('#job-history-more').hidden=!historyNext;pendingUI();
      }catch(e){if(seq===detailGeneration)message(e.message);}
    }
    async function navigate(){
      clearTimeout(refreshTimer);generation++;detailGeneration++;dirty=false;
      const params=new URLSearchParams(location.hash.slice(1));context={project:HeyBossUI.projectId(boot.project?.id||boot.projects[0]?.id),host:mobile?null:params.get('host')||boot.backend_host||null};
      const calendarView=params.get('view')==='calendar';
      $('#jobs-calendar').hidden=!calendarView;$('.jobs-layout').hidden=calendarView;
      $('#jobs-view-list').setAttribute('aria-pressed',String(!calendarView));$('#jobs-view-calendar').setAttribute('aria-pressed',String(calendarView));
      if(calendarView&&context.project){const identity=JSON.stringify(context);calendar.show(calendarProject!==identity);calendarProject=identity;}else calendar.hide();
      picker.update(boot.projects,boot.projects.find(p=>p.id===context.project)||{id:context.project,name:context.project});$('#nav-jobs').setAttribute('aria-current','page');const nav=$('#nav-jobs').closest('.app-navigation');nav.scrollLeft=$('#nav-jobs').offsetLeft-nav.clientWidth/2;
      const dialog=$('#job-dialog'),hasJob=!!params.get('job'),wasOpen=dialog.open,previousJob=selected?.id;
      $('#jobs-feedback').append($('#jobs-error'),$('#jobs-retry'));
      if(hasJob){$('#job-panel').innerHTML='<h2 id="job-panel-title">Loading job…</h2>';$('#job-dialog-feedback').append($('#jobs-error'),$('#jobs-retry'));if(!dialog.open)dialog.showModal();}
      else if(dialog.open)dialog.close();
      pendingUI();rows=[];next=null;renderList();if(!context.project){$('#jobs-status').textContent='Choose a project to manage jobs.';return;}
      await Promise.all([load(),service(),open(params.get('job'))]);
      if(wasOpen&&!hasJob){const link=[...document.querySelectorAll('.job-card')].find(el=>new URLSearchParams(el.hash.slice(1)).get('job')===previousJob);(link||$('#job-new')).focus({preventScroll:true});}
      scheduleRefresh();
    }
    function scheduleRefresh(){clearTimeout(refreshTimer);if(!document.hidden)refreshTimer=setTimeout(async()=>{if(!busy){await load();if(selected)await loadHistory();await service();calendar.refresh();}scheduleRefresh();},15000);}
    function newJob(){if(leave())location.hash=hash('new');}
    $('#job-new').onclick=newJob;
    const closeJob=()=>{if(leave())location.hash=hash();};
    $('#job-close').onclick=closeJob;$('#job-dialog').addEventListener('cancel',event=>{event.preventDefault();closeJob();});
    $('#job-dialog').addEventListener('click',event=>{if(event.target===$('#job-dialog')){const r=event.target.getBoundingClientRect();if(event.clientX<r.left||event.clientX>r.right||event.clientY<r.top||event.clientY>r.bottom)closeJob();}});
    $('#jobs-search').oninput=renderList;
    for(const button of document.querySelectorAll('[data-filter]'))button.onclick=()=>{filter=button.dataset.filter;renderList();};
    $('#jobs-view-list').onclick=()=>{if(leave())location.hash=hash(selected?.id);};
    $('#jobs-view-calendar').onclick=()=>{if(leave()){const params=hash();params.set('view','calendar');location.hash=params;}};
    $('#jobs-refresh').onclick=async()=>{message('');await load();await service();if(selected)await loadHistory();};$('#jobs-more').onclick=()=>load(false);$('#jobs-deleted').onchange=()=>load();
    $('#job-retry').onclick=()=>act(null,mutation.pending?.project,true);
    $('#jobs-list').onclick=event=>{const toggle=event.target.closest('[data-job-toggle]');if(toggle){const job=rows.find(j=>j.id===toggle.dataset.jobToggle);if(job&&!job.deleted_at)act({command:'set_enabled',id:job.id,if_revision:job.revision,enabled:!job.enabled});}else if(event.target.closest('a')&&!leave())event.preventDefault();};
    window.addEventListener('hashchange',()=>{if(boot)navigate();});
    window.addEventListener('beforeunload',event=>{if(dirty){event.preventDefault();event.returnValue='';}});
    document.addEventListener('visibilitychange',()=>scheduleRefresh());
    HeyBossUI.icons();
    try{const response=await fetch('/api/bootstrap');boot=await response.json();if(!response.ok||!boot.ok)throw Error(boot.error?.message||boot.error||'Could not connect');await navigate();}catch(e){message(e.message);$('#jobs-status').textContent='Unable to load jobs. Reload to reconnect.';}
  }
  return {summary,modelChoices,availability,Mutation,scheduleState,filterJobs,scheduleFields,scheduleCron,start};
})();
if(typeof module!=='undefined')module.exports=HeyBossJobs;
if(typeof document!=='undefined')HeyBossJobs.start();
