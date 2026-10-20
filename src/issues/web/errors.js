'use strict';
(async()=>{
  const $=s=>document.querySelector(s), esc=s=>String(s??'').replace(/[&<>"']/g,c=>({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));
  let boot,project,rows=[],enabled=false,generation=0,busy=false;
  const drafts=new Map();
  const alert=message=>{$('#errors-alert').textContent=message;$('#errors-alert').hidden=!message;};
  const time=t=>t?new Date(t).toLocaleString():'No retained samples';
  const picker=new HeyBossUI.ProjectPicker({onSelect:id=>{location.hash=new URLSearchParams({project:id});}});
  async function api(action,extra={}){
    return HeyBossUI.post('/api/errors',{action,project,...extra},boot,{fallback:'Error inbox request failed'});
  }
  function render(){
    const filtered=rows.filter(r=>r.title.toLowerCase().includes($('#error-search').value.toLowerCase()));
    $('#families').textContent=rows.length;
    $('#collection').textContent=enabled?'Collection on':'Enable collection';$('#collection').setAttribute('aria-pressed',String(enabled));
    $('#error-rows').innerHTML=filtered.map(r=>`<article class="issue-row"><span class="issue-state" data-icon="code"></span><div class="issue-row-main"><div class="issue-title-line"><button class="issue-title error-title" data-id="${esc(r.id)}" aria-haspopup="dialog">${esc(r.title)}</button></div><div class="issue-meta"><span>Last seen ${esc(time(r.last_seen))}</span><span>${r.total.toLocaleString()} total</span>${r.issue?`<span>Task #${r.issue}</span>`:''}</div></div><span class="error-count" aria-label="${r.count_24h.toLocaleString()} observed in the last 24 hours">${r.count_24h.toLocaleString()}<small>last 24h</small></span>${r.issue?`<a class="button error-row-action" href="/#${esc(new URLSearchParams({project,issue:r.issue}))}">Open task</a>`:`<button type="button" class="button error-row-action" data-create="${esc(r.id)}" aria-haspopup="dialog">Create task</button>`}</article>`).join('');
    HeyBossUI.icons();
    $('#errors-status').textContent=filtered.length?`Showing ${filtered.length} of up to 200 groups` : enabled?'No matching errors. The collector will send new observations here.':'Collection is off. Enable it for this project to accept events.';
    $('#error-rows').querySelectorAll('[data-id]').forEach(button=>button.onclick=()=>show(button.dataset.id));
    $('#error-rows').querySelectorAll('[data-create]').forEach(button=>button.onclick=()=>show(button.dataset.create,true));
  }
  async function show(id,editPrompt=false){
    const seq=++generation;
    $('#error-title').textContent=rows.find(r=>r.id===id)?.title||'Error details';
    $('#error-detail').textContent='Loading evidence…';
    if(!$('#error-dialog').open)$('#error-dialog').showModal();
    try{
      const d=await api('detail',{id});if(seq!==generation||!$('#error-dialog').open)return;
      const group=rows.find(r=>r.id===id);
      $('#error-detail').innerHTML=`<p class="error-meta">${(group?.count_24h??0).toLocaleString()} in the last 24 hours · ${(group?.previous_24h??0).toLocaleString()} in the previous 24 hours · ${d.total.toLocaleString()} total observed occurrences · ${d.samples.length} recent samples</p><div class="error-actions">${d.issue?`<a class="button primary" href="/#${esc(new URLSearchParams({project,issue:d.issue}))}">Open task #${d.issue}</a>`:`<div class="error-task-editor"><label class="field-label" for="error-task-prompt">Task prompt</label><textarea id="error-task-prompt" class="text-input" maxlength="32000" rows="3" spellcheck="false"></textarea><span class="error-task-settings">Default configurable in Project settings → Error task.</span></div><p id="error-task-status" role="alert" hidden></p><button class="button primary" id="promote">Create task</button>`}</div>${d.samples.map((s,i)=>`<details ${i===0?'open':''}><summary>${esc(s.source)} · ${esc(time(s.timestamp))}</summary><dl><dt>Release</dt><dd>${esc(s.release||'Unknown')}</dd><dt>Reference</dt><dd>${esc(s.request_ref||'Unavailable')}</dd><dt>Event</dt><dd>${esc(s.event_id)}</dd><dt>Occurrences</dt><dd>${s.count}</dd></dl><pre tabindex="0">${esc(s.message)}${s.stack?'\n\n'+esc(s.stack):''}</pre></details>`).join('')}`;
      if($('#promote')){
        const key=project+':'+id, input=$('#error-task-prompt'), button=$('#promote');
        input.value=drafts.get(key)??d.task_prompt;
        button.disabled=!input.value.trim();
        if(editPrompt)input.focus();
        input.oninput=()=>{drafts.set(key,input.value);button.disabled=!input.value.trim();};
        button.onclick=()=>mutate(async()=>{
          const selectedProject=project, prompt=input.value;
          input.disabled=true;
          try{
            await api('promote',{id,prompt});drafts.delete(key);
            if(project!==selectedProject)return;
            const stillOpen=$('#error-dialog').open;
            await load();if(stillOpen&&$('#error-dialog').open)await show(id);
          }catch(error){
            if(input.isConnected){$('#error-task-status').textContent=error.message;$('#error-task-status').hidden=false;}
            else throw error;
          }finally{input.disabled=false;}
        });
      }
    }catch(e){if(seq===generation&&$('#error-dialog').open)$('#error-detail').textContent=e.message;}
  }
  async function mutate(fn){if(busy)return;busy=true;alert('');document.querySelectorAll('#errors-main button').forEach(b=>b.disabled=true);try{await fn();}catch(e){alert(e.message);}finally{busy=false;document.querySelectorAll('#errors-main button').forEach(b=>b.disabled=false);}}
  async function load(){const seq=++generation;const result=await api('list');if(seq!==generation)return;rows=result.groups;enabled=result.enabled;render();}
  async function navigate(){
    generation++;$('#error-dialog').close();alert('');project=HeyBossUI.projectId(boot.project?.id||boot.projects[0]?.id);
    picker.update(boot.projects,boot.projects.find(p=>p.id===project)||{id:project,name:project});
    try{await load();}catch(e){alert(e.message);$('#errors-status').textContent='Could not load errors. Use Refresh to retry.';}
  }
  $('#close-error').onclick=()=>$('#error-dialog').close();
  $('#error-search').oninput=render;$('#refresh').onclick=()=>mutate(load);$('#collection').onclick=()=>mutate(async()=>{await api('enable',{enabled:!enabled});await load();});
  window.addEventListener('hashchange',()=>{if(boot)navigate();});
  try{const response=await fetch('/api/bootstrap',{signal:AbortSignal.timeout(30000)});boot=await response.json();if(!response.ok||!boot.ok)throw Error(boot.error?.message||'Could not connect');HeyBossUI.icons();await navigate();}catch(e){alert(e.message);$('#errors-status').textContent='Reload to reconnect.';}
})();
