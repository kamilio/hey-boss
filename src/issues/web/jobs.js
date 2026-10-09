"use strict";
const HeyBossJobs = (() => {
  const esc = value => String(value ?? '').replace(/[&<>"']/g,c=>({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));
  function summary(cron) {
    const [m,h,d,month,w] = cron.trim().split(/\s+/);
    if (/^\d+$/.test(m)&&/^\d+$/.test(h)&&d==='*'&&month==='*'&&['*','1-5'].includes(w)) return `${w==='*'?'Every day':'Weekdays'} at ${h.padStart(2,'0')}:${m.padStart(2,'0')}`;
    if(m==='*'&&h==='*'&&d==='*'&&month==='*'&&w==='*')return 'Every minute';
    if(m==='0'&&h==='*'&&d==='*'&&month==='*'&&w==='*')return 'Every hour';
    return `Minute ${m} · Hour ${h} · Day ${d} · Month ${month} · Weekday ${w}`;
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
    constructor(post, uuid=()=>crypto.randomUUID(), persist=()=>{}) {this.post=post;this.uuid=uuid;this.persist=persist;this.pending=null;}
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
    let boot,context,rows=[],next=null,selected=null,editor=null,generation=0,detailGeneration=0,modelGeneration=0,previewGeneration=0,previewTimer,refreshTimer,history=[],historyNext=null,busy=false,dirty=false,machines=[],catalogResult=null;
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
    const picker=new HeyBossUI.ProjectPicker({onSelect:project=>{if(leave())location.hash=new URLSearchParams({project,...(context.host?{host:context.host}:{})});}});
    function leave(){return !dirty||confirm('Discard your unsaved job changes?');}
    function pendingUI(){
      $('#jobs-retry').hidden=!mutation.pending;
      for(const el of document.querySelectorAll('[data-mutation],#job-new,#job-empty-new'))el.disabled=busy||!!mutation.pending||!context?.project;
      $('#job-retry').disabled=busy;
    }
    function renderList(){
      $('#jobs-count').textContent=rows.length?String(rows.length)+(next?' +':''):'';
      $('#jobs-status').textContent=rows.length?'':$('#jobs-deleted').checked?'No jobs in this project.':'No scheduled jobs yet.';
      $('#jobs-list').innerHTML=rows.map(job=>{const d=job.snapshot.definition;return `<a class="job-card" href="#${esc(hash(job.id))}" aria-current="${selected?.id===job.id}"><div class="job-card-top"><h2>${esc(d.name)}</h2><span class="job-badge ${job.deleted_at?'deleted':job.enabled?'':'paused'}">${job.deleted_at?'Deleted':job.enabled?'Scheduled':'Paused'}</span></div><div class="job-card-schedule">${esc(summary(d.cron))}</div><div class="job-card-zone">${esc(d.timezone)}</div><div class="job-card-model">${esc(d.harness)} / ${esc(d.model)}</div><div class="job-card-footer"><span>${job.next_at?'Next '+esc(date(job.next_at,d.timezone,true)):job.deleted_at?'History retained':'Schedule paused'}</span><span class="job-badge ${esc(job.active_run?.state||job.last_run?.state||'paused')}">${esc(job.active_run?.state||job.last_run?.state||'No runs')}</span></div></a>`;}).join('');
      $('#jobs-more').hidden=!next;
    }
    async function load(reset=true){
      const seq=++generation,ctx={...context};
      try{const value=await read({command:'list',after:reset?null:next,limit:50,include_deleted:$('#jobs-deleted').checked},ctx);if(seq!==generation)return;
        rows=reset?value.jobs:[...rows,...value.jobs];next=value.next_cursor;renderList();
      }catch(e){if(seq===generation)message(e.message);}
    }
    async function service(){
      try{const value=await post({},'/api/jobs/runtime');machines=value.machines||[];const available=availability(machines,context.project);
        $('#jobs-service').textContent=available?'Job service available':value.service_error||'No connected job service has this project checkout. Runs will wait.';$('#jobs-service').dataset.available=String(available);
      }catch(e){$('#jobs-service').textContent='Job service status unavailable · '+e.message;}
    }
    function field(id,label,value,extra=''){return `<label class="job-field ${extra.includes('wide')?'wide':''}" for="${id}">${label}<input id="${id}" value="${esc(value)}" ${extra.replace('wide','')}></label>`;}
    async function open(id){
      const seq=++detailGeneration;modelGeneration++;previewGeneration++;editor=null;dirty=false;message('');
      if(id==='new'){selected=null;renderEditor(null,'');renderList();return;}
      if(!id){selected=null;$('#job-panel').innerHTML='<div class="jobs-empty"><span data-icon="clock"></span><h2>Make room for what’s next</h2><p>Select a job to edit its schedule or explore its run history.</p></div>';HeyBossUI.icons($('#job-panel'));renderList();return;}
      $('#job-panel').innerHTML='<p class="job-help" role="status">Loading job…</p>';
      try{const {job}=await read({command:'view',id});const revision=await read({command:'revision',id,revision:job.revision});if(seq!==detailGeneration)return;
        selected=job;renderEditor(job,revision.instructions.markdown);renderList();await loadHistory();
      }catch(e){if(seq===detailGeneration){message(e.message);$('#job-panel').innerHTML='<p class="job-help">Could not load this job. Refresh to retry.</p>';}}
    }
    function renderEditor(job,markdown){
      const deleted=!!job?.deleted_at,d=job?.snapshot.definition||{name:'',cron:'0 9 * * *',timezone:Intl.DateTimeFormat().resolvedOptions().timeZone||'UTC',harness:'codex',model:''};
      editor={job,markdown};
      $('#job-panel').innerHTML=`<div class="job-panel-heading"><div><h2>${job?esc(d.name):'New job'}</h2><p class="job-panel-subtitle">${deleted?'Deleted · Instructions and history are preserved':job?'Revision '+job.revision+' · '+(job.enabled?'Schedule enabled':'Schedule paused'):'Your instructions. Your schedule.'}</p></div>${job?`<span class="job-badge ${deleted?'deleted':job.enabled?'':'paused'}">${deleted?'Deleted':job.enabled?'Scheduled':'Paused'}</span>`:''}</div>
       ${job?`<div class="job-section job-controls"><h3 class="job-section-title">RUN CONTROLS</h3><div class="job-actions">${!deleted?'<button class="button" id="job-enabled-toggle" type="button" data-mutation>'+(job.enabled?'Pause schedule':'Resume schedule')+'</button><button class="button" id="job-run" type="button" data-mutation>Run now</button>':''}<button class="button danger" id="job-stop" type="button" data-mutation hidden>Stop current run</button></div><p class="job-help" id="job-run-status">Run now also works while the schedule is paused.</p></div>`:''}
       <form id="job-form"><fieldset ${deleted?'disabled':''} style="border:0;padding:0;margin:0;min-width:0"><div class="job-fields">${field('job-name','Name',d.name,'required maxlength="256" wide')}<label class="job-field wide" for="job-project">Project<select id="job-project" ${job?'disabled':''}>${boot.projects.map(p=>`<option value="${esc(p.id)}" ${p.id===context.project?'selected':''}>${esc(p.name)}</option>`).join('')}</select>${job?'<small>A job stays with the project where it was created.</small>':''}</label><label class="job-field wide" for="job-markdown">Task instructions <small>Markdown · Used exactly as written</small><textarea id="job-markdown" required spellcheck="false" aria-label="Task instructions">${esc(markdown)}</textarea></label></div>
       <div class="job-section"><h3 class="job-section-title">SCHEDULE</h3><div class="job-fields">${field('job-cron','Cron expression',d.cron,'required aria-describedby="job-cron-help"')}${field('job-timezone','Timezone',d.timezone,'required list="job-timezones"')}<datalist id="job-timezones">${[...new Set(['UTC',...(Intl.supportedValuesOf?.('timeZone')||[])])].map(z=>`<option value="${esc(z)}"></option>`).join('')}</datalist></div><p class="job-help" id="job-cron-help">Five fields: minute · hour · day · month · weekday</p><div id="job-preview" class="job-preview" aria-live="polite">Checking schedule…</div></div>
       <div class="job-section"><h3 class="job-section-title">EXECUTION</h3><div class="job-fields"><label class="job-field" for="job-harness">Harness<select id="job-harness">${['codex','claude','pi'].map(h=>`<option ${h===d.harness?'selected':''}>${h}</option>`).join('')}</select></label><label class="job-field" for="job-model">Model<input id="job-model" list="job-models" value="${esc(d.model)}" required maxlength="256" autocomplete="off" aria-describedby="job-model-status"><datalist id="job-models"></datalist></label></div><p id="job-model-status" class="job-model-status" role="status">Discovering models…</p><button class="button" id="job-model-refresh" type="button">Refresh models</button>${!job?'<label class="job-toggle"><input id="job-enabled" type="checkbox" checked> Enable this schedule</label>':''}<p class="job-help">Overlapping occurrences are skipped. Pausing the schedule leaves the current run alone.</p></div>
       ${deleted?'':`<div class="job-actions"><button class="button primary" id="job-save" type="submit" data-mutation>${job?'Save changes':'Create job'}</button>${job?'<button class="button" id="job-reload" type="button">Reload saved</button>':''}<span class="job-spacer"></span>${job?'<button class="button danger" id="job-delete" type="button" data-mutation>Delete job</button>':''}</div>`}</fieldset></form>

       ${job?'<section class="job-section" aria-label="Run history"><div class="job-history-heading"><h3>Run history</h3><button class="icon-button" id="job-history-refresh" type="button" aria-label="Refresh run history"><span data-icon="refresh"></span></button></div><div id="job-history" aria-live="polite"></div><button class="button" id="job-history-more" type="button" hidden>Older runs</button></section>':''}`;
      HeyBossUI.icons($('#job-panel'));pendingUI();
      $('#job-form').oninput=()=>{dirty=true;};
      $('#job-form').onsubmit=save;
      for(const id of ['job-cron','job-timezone'])$('#'+id).oninput=()=>{dirty=true;clearTimeout(previewTimer);previewTimer=setTimeout(preview,300);};
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
        $('#job-preview').innerHTML=`<strong>${esc(summary(cron))}</strong><div class="job-help">${esc(zone)} · Next scheduled occurrences${selected&&!selected.enabled?' if resumed':''}</div><ol>${value.occurrences.map(ms=>`<li>${esc(date(ms,zone))}</li>`).join('')}</ol>`;
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
      const operation={command:job?'edit':'create',id:job?.id||crypto.randomUUID(),definition,markdown:$('#job-markdown').value,...(job?{if_revision:job.revision}:{enabled:$('#job-enabled').checked})};
      await act(operation,$('#job-project').value);
    }
    async function act(operation,project=context.project,retry=false){
      if(busy)return;if(dirty&&!retry&&!['create','edit','run_now','stop'].includes(operation.command)&&!leave())return;busy=true;pendingUI();message('');const ctx={...context,project};
      try{const result=retry?await mutation.retry():await mutation.send(operation,project,context.host);if(result.job)dirty=false;
        if(result.job){selected=result.job;const destination='#'+new URLSearchParams({project:result.job.project_id,...(ctx.host?{host:ctx.host}:{}),job:result.job.id});if(location.hash!==destination){location.hash=destination;return;}await open(result.job.id);}
        else if(selected)await loadHistory();
        await load();
      }catch(e){message(e.code==='conflict'?'This job changed on another device. Your draft is preserved. Use Reload saved to review the latest revision before saving again.':e.message);}
      finally{busy=false;pendingUI();}
    }
    async function loadHistory(reset=true){
      const job=selected;if(!job)return;const seq=detailGeneration;
      try{const value=await read({command:'history',id:job.id,before:reset?null:historyNext,limit:20});if(seq!==detailGeneration)return;
        history=reset?value.runs:[...history,...value.runs];historyNext=value.next_cursor;
        // The active run can be older than a page of skipped overlapping occurrences.
        const latest=await read({command:'view',id:job.id});if(seq!==detailGeneration)return;
        const active=latest.job.active_run;
        if($('#job-stop')){$('#job-stop').hidden=!active;$('#job-stop').onclick=()=>act({command:'stop',id:job.id,run_id:active.id});}
        if($('#job-run-status'))$('#job-run-status').textContent=active?(active.reason||`${active.state==='running'?'Running':'Pending'} · Stop affects this execution only.`):job.deleted_at?'This job is deleted. Saved runs remain available.':'Run now also works while the schedule is paused.';
        $('#job-history').innerHTML=history.length?`<ol class="job-history-list">${history.map(run=>`<li class="job-run"><div class="job-run-top"><span class="job-badge ${esc(run.state)}">${esc(run.state)}</span><time>${esc(date(run.scheduled_at,run.snapshot.definition.timezone,true))}</time></div>${run.reason?`<p>${esc(run.reason)}</p>`:''}<p><small>${esc(run.trigger==='manual'?'Run now':'Scheduled')} · Revision ${run.snapshot.revision} · ${esc(run.snapshot.definition.harness)} / ${esc(run.snapshot.definition.model)}</small></p><div class="job-run-links">${run.task_number?`<a href="/${mobile?'issues':''}#${esc(new URLSearchParams({project:job.project_id,issue:run.task_number,...(context.host?{host:context.host}:{})}))}">Task #${run.task_number} ↗</a>`:''}${run.session_id?`<a href="/agents/session#${esc(new URLSearchParams({project:job.project_id,host:machines.find(m=>m.node===run.machine)?.host||run.machine||'local',run:'job:'+run.id}))}">Session ↗</a>`:''}</div></li>`).join('')}</ol>`:'<p class="job-help">No runs yet. Your first execution will appear here.</p>';
        $('#job-history-more').hidden=!historyNext;pendingUI();
      }catch(e){if(seq===detailGeneration)message(e.message);}
    }
    async function navigate(){
      clearTimeout(refreshTimer);generation++;detailGeneration++;dirty=false;
      const params=new URLSearchParams(location.hash.slice(1));context={project:HeyBossUI.projectId(boot.project?.id||boot.projects[0]?.id),host:mobile?null:params.get('host')||boot.backend_host||null};
      picker.update(boot.projects,boot.projects.find(p=>p.id===context.project)||{id:context.project,name:context.project});$('#nav-jobs').setAttribute('aria-current','page');const nav=$('#nav-jobs').closest('.app-navigation');nav.scrollLeft=$('#nav-jobs').offsetLeft-nav.clientWidth/2;
      pendingUI();rows=[];next=null;renderList();if(!context.project){$('#jobs-status').textContent='Choose a project to manage jobs.';return;}
      await Promise.all([load(),service(),open(params.get('job'))]);scheduleRefresh();
    }
    function scheduleRefresh(){clearTimeout(refreshTimer);if(!document.hidden)refreshTimer=setTimeout(async()=>{if(!busy){await load();if(selected)await loadHistory();await service();}scheduleRefresh();},15000);}
    function newJob(){if(leave())location.hash=hash('new');}
    $('#job-new').onclick=newJob;$('#job-empty-new').onclick=newJob;
    $('#jobs-refresh').onclick=async()=>{message('');await load();await service();if(selected)await loadHistory();};$('#jobs-more').onclick=()=>load(false);$('#jobs-deleted').onchange=()=>load();
    $('#job-retry').onclick=()=>act(null,mutation.pending?.project,true);
    $('#jobs-list').onclick=event=>{if(event.target.closest('a')&&!leave())event.preventDefault();};
    window.addEventListener('hashchange',()=>{if(boot)navigate();});
    window.addEventListener('beforeunload',event=>{if(dirty){event.preventDefault();event.returnValue='';}});
    document.addEventListener('visibilitychange',()=>scheduleRefresh());
    HeyBossUI.icons();
    try{const response=await fetch('/api/bootstrap');boot=await response.json();if(!response.ok||!boot.ok)throw Error(boot.error?.message||boot.error||'Could not connect');await navigate();}catch(e){message(e.message);$('#jobs-status').textContent='Unable to load jobs. Reload to reconnect.';}
  }
  return {summary,modelChoices,availability,Mutation,start};
})();
if(typeof module!=='undefined')module.exports=HeyBossJobs;
if(typeof document!=='undefined')HeyBossJobs.start();
