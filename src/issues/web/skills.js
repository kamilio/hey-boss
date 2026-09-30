"use strict";
function library(data) {
  const entries = new Map();
  for (const machine of data.machines || []) for (const source of machine.copies || []) {
    const key = `${source.scope}:${source.name}`;
    if (!entries.has(key)) entries.set(key, {key, name:source.name, scope:source.scope, copies:[], versions:[], machines:[]});
    const skill = entries.get(key);
    const copy = {...source, host:machine.host, hostname:machine.hostname || machine.host, stale:machine.state === 'attention'};
    skill.copies.push(copy);
    if (!skill.versions.some(v=>v.digest===copy.digest)) skill.versions.push(copy);
    if (!skill.machines.includes(machine.host)) skill.machines.push(machine.host);
  }
  return [...entries.values()].sort((a,b)=>a.name.localeCompare(b.name)||a.scope.localeCompare(b.scope));
}
function needsAttention(skill) { return skill.versions.length>1 || skill.copies.some(c=>c.stale || c.warnings?.length); }
function visibleSkills(skills, query, filter, selected) {
  query = query.trim().toLocaleLowerCase();
  return skills.filter(s=>(filter==='project'?s.scope==='project':s.scope==='global') &&
    (filter!=='selected'||selected.has(s.name)) && (filter!=='attention'||needsAttention(s)) &&
    `${s.name} ${s.copies.map(c=>`${c.description} ${c.host} ${c.hostname}`).join(' ')}`.toLocaleLowerCase().includes(query));
}
function unresolved(skills, selected, choices) {
  return skills.filter(s=>s.scope==='global'&&selected.has(s.name)&&
    (choices[s.name]?!s.versions.some(v=>v.digest===choices[s.name]):s.versions.length>1)).map(s=>s.name);
}
if(typeof module!=='undefined') module.exports={library,visibleSkills,unresolved};
if(typeof document!=='undefined') (()=>{
  const $=s=>document.querySelector(s), esc=s=>String(s??'').replace(/[&<>"']/g,c=>({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c])), icon=HeyBossUI.icon;
  let data={machines:[],selected:[],choices:{}}, skills=[], selected=new Set(), choices={}, active='', filter='all', csrf='', dirty=false, pending=false, timer, previewDigest='', maxWords=400;
  const labels={codex:'Codex',claude:'Claude',agents:'Shared agents',project:'Repository',library:'Saved library'};
  function fail(e){$('#skills-error').textContent=e.message;$('#skills-error').hidden=false;}
  async function request(body){
    const response=await fetch('/api/skills',{method:body?'POST':'GET',headers:body?{'Content-Type':'application/json','X-Hey-Boss-CSRF':csrf}:{},body:body?JSON.stringify(body):undefined,signal:AbortSignal.timeout(15000)});
    const value=await response.json();if(!response.ok||value.ok===false)throw Error(value.error?.message||value.error||'Could not reach skill manager');return value;
  }
  function use(value,reset=false){
    data=value;skills=library(data);
    if(reset||!dirty){selected=new Set(data.selected);choices={...data.choices};maxWords=data.max_words||400;}
    if(!skills.some(s=>s.key===active))active=skills[0]?.key||'';
    render();
    clearTimeout(timer);if(data.busy)timer=setTimeout(poll,1200);
  }
  async function poll(){try{use(await request());}catch(e){fail(e);timer=setTimeout(poll,4000);}}
  const current = skill => skill.versions.find(v=>v.digest===choices[skill.name]) || (skill.versions.length===1?skill.versions[0]:null);
  function changes(){dirty=true;$('#skills-error').hidden=true;render();}
  function render(){
    const focus=document.activeElement;
    const restore=['select','choose','open','preview'].find(key=>focus?.dataset?.[key]);
    const focusValue=restore?focus.dataset[restore]:null;
    const global=skills.filter(s=>s.scope==='global'), busy=pending||data.busy;
    $('#skills-main').classList.toggle('is-busy',!!busy);
    $('#skills-summary').innerHTML=[[global.length,'skills discovered'],[global.filter(s=>selected.has(s.name)).length,'selected to sync'],[global.filter(needsAttention).length,'need attention'],[(data.machines||[]).filter(m=>m.scanned_at).length,'machines scanned']].map(([n,label])=>`<div><strong>${n}</strong><span>${label}</span></div>`).join('');
    const machines=data.machines||[], attention=machines.filter(m=>m.state==='attention'||m.errors?.length).length;
    $('#machine-summary').textContent=busy?'Scanning or syncing…':`${machines.length} known${attention?` · ${attention} need attention`:''}`;
    $('#skills-machines').innerHTML=machines.map(m=>`<article class="machine-card"><div>${icon('monitor')}<strong>${esc(m.hostname||m.host)}</strong><span class="skill-pill ${m.state==='attention'?'amber':'green'}">${m.state==='synced'?'Synced':m.state==='online'?'Scanned':'Needs attention'}</span></div><p>${esc(m.host==='local'?'This machine':m.host)} · ${(m.copies||[]).length} copies${m.scanned_at?` · ${esc(new Date(m.scanned_at*1000).toLocaleTimeString([],{hour:'2-digit',minute:'2-digit'}))}`:''}</p>${m.error?`<p class="machine-error">${esc(m.error)} · Last known copies are kept.</p>`:''}${m.errors?.length?`<details><summary>${m.errors.length} skipped item(s)</summary>${m.errors.map(e=>`<p class="machine-error">${esc(e)}</p>`).join('')}</details>`:''}</article>`).join('')||'<p>No machine inventory yet. Scan to discover your skills.</p>';
    renderList();renderDetail();
    const conflicts=unresolved(skills,selected,choices), count=global.filter(s=>selected.has(s.name)).length;
    $('#selection-summary').textContent=`${count} skill${count===1?'':'s'} selected${dirty?' · Unsaved changes':''}`;
    $('#skills-status').textContent=conflicts.length?`Choose a version for ${conflicts.length} selected skill${conflicts.length===1?'':'s'} before distributing.`:data.message||'Scan your machines to build the library.';
    $('#skills-distribute').disabled=!!busy||!skills.length||!!conflicts.length;
    $('#skills-scan').disabled=!!busy;
    $('#skills-reset').hidden=!dirty;$('#skills-reset').disabled=!!busy;
    if(restore)document.querySelector(`[data-${restore}="${CSS.escape(focusValue)}"]`)?.focus({preventScroll:true});
  }
  function renderList(){
    const visible=visibleSkills(skills,$('#skills-search').value,filter,selected);
    $('#library-count').textContent=visible.length;
    $('#skills-filters').querySelectorAll('button').forEach(b=>b.setAttribute('aria-pressed',String(b.dataset.filter===filter)));
    $('#skills-list').innerHTML=visible.map(s=>{
      const version=current(s)||s.versions[0], warnings=(version.warnings||[]).length;
      return `<div class="skill-row ${active===s.key?'is-active':''} ${selected.has(s.name)&&s.scope==='global'?'is-selected':''}">${s.scope==='global'?`<input type="checkbox" data-select="${esc(s.name)}" aria-label="Distribute ${esc(s.name)}" ${selected.has(s.name)?'checked':''} ${s.name==='hey-boss'||pending||data.busy?'disabled':''}>`:`<span class="skill-project-icon">${icon('folder')}</span>`}<button class="skill-open" data-open="${esc(s.key)}" aria-current="${active===s.key}"><span class="skill-row-title">${esc(s.name)}${s.name==='hey-boss'?'<span class="skill-required">CORE</span>':''}</span><span class="skill-row-description">${esc(version.description||'No description yet')}</span><span class="skill-row-meta">${s.machines.length} machine${s.machines.length===1?'':'s'}<i></i>${s.versions.length>1?`<span class="amber-text">${s.versions.length} versions</span>`:'1 version'}${warnings?`<i></i><span class="amber-text">${warnings} lint</span>`:''}${s.copies.some(c=>c.stale)?'<i></i><span class="amber-text">Last seen</span>':''}</span></button><span class="skill-row-arrow">${icon('arrow-right')}</span></div>`;
    }).join('')||`<div class="skills-empty">${icon(skills.length?'search':'instructions')}<h3>${skills.length?'No matching skills':'Your library starts here'}</h3><p>${skills.length?'Try another search or filter.':'Scan all machines to discover your existing skills.'}</p></div>`;
  }
  function renderDetail(){
    const skill=skills.find(s=>s.key===active);if(!skill){$('#skills-detail').innerHTML='<div class="skills-empty"><h2>Select a skill</h2><p>Scan your machines to build the library.</p></div>';return;}
    const chosen=current(skill), preview=skill.versions.find(v=>v.digest===previewDigest)||chosen||skill.versions[0], warnings=preview.warnings||[];
    const copies=skill.copies.filter(c=>c.digest===preview.digest), hasAgent=a=>copies.some(c=>c.agent===a||(a==='codex'&&c.agent==='agents'));
    $('#skills-detail').innerHTML=`<div class="skill-detail-heading"><div class="skill-mark">${icon('instructions')}</div><div><span class="skills-eyebrow">${skill.scope==='project'?'PROJECT SKILL · AUDIT ONLY':'SKILL INSPECTOR'}</span><h2>${esc(skill.name)}</h2></div><span class="skill-pill ${chosen?'green':'amber'}">${chosen?'Version chosen':'Choose a version'}</span></div><p class="skill-description">${esc(preview.description||'Add a clear description to help agents discover this skill.')}</p><div class="skill-targets"><span class="agent-target ${hasAgent('codex')?'present':''}">${icon('code')}Codex <small>${hasAgent('codex')?'found':'not found'}</small></span><span class="agent-target ${hasAgent('claude')?'present':''}">${icon('spark')}Claude <small>${hasAgent('claude')?'found':'not found'}</small></span><span class="skill-size">${preview.word_count||0} words · ${preview.text.split('\n').length} lines</span></div>
    <section class="skill-section"><div class="skill-section-title"><h3>Source versions <span>${skill.versions.length}</span></h3><span>Choose what goes everywhere</span></div>${skill.versions.length>1?'<p class="skill-conflict-note">Different copies found. Preview each version, then choose the one to distribute.</p>':''}<div class="skill-versions">${skill.versions.map((v,i)=>{
      const locations=skill.copies.filter(c=>c.digest===v.digest);return `<div class="skill-version ${preview.digest===v.digest?'is-previewing':''}"><button class="version-preview" data-preview="${esc(v.digest)}" aria-pressed="${preview.digest===v.digest}"><span class="version-symbol">${String(i+1).padStart(2,'0')}</span><span><strong>${esc([...new Set(locations.map(c=>c.host==='local'?'This machine':c.hostname))].join(', '))}</strong><small>${esc([...new Set(locations.map(c=>labels[c.agent]||c.agent))].join(' · '))} · ${v.word_count||0} words${locations.every(c=>c.stale)?' · Last seen':''}</small></span></button>${skill.scope==='global'?`<button class="version-choose ${chosen?.digest===v.digest?'chosen':''}" data-choose="${esc(v.digest)}" ${pending||data.busy?'disabled':''} aria-label="Choose version ${i+1} of ${esc(skill.name)}">${chosen?.digest===v.digest?`${icon('check')}Chosen`:'Use this'}</button>`:''}</div>`;
    }).join('')}</div></section>
    <section class="skill-section"><div class="skill-section-title"><h3>Compatibility check <span class="${warnings.length?'amber-text':'green-text'}">${warnings.length?`${warnings.length} findings`:'No findings'}</span></h3><span>Codex + Claude</span></div>${warnings.length?`<div class="skill-findings">${warnings.map(w=>`<div class="skill-finding">${icon('warning')}<div><strong>${esc(({metadata:'Skill metadata',too_long:'Keep it short',agent_specific:'Agent-specific setting',machine_path:'Machine-specific path',agent_tool:'Agent-specific tool',broken_reference:'Missing reference'})[w.kind]||w.kind)}</strong><p>${esc(w.message)}</p></div>${w.line?`<button data-line="${w.line}" title="Show source line ${w.line}">L${w.line}</button>`:''}</div>`).join('')}</div>`:`<div class="skill-clean">${icon('check')}No portability issues found by the static checks.</div>`}<p class="skill-lint-note">Checks cover metadata, length, local paths, tool names, and linked files. Agent tools and commands still need to be available.</p></section>
    <details class="skill-source" ${previewDigest?'open':''}><summary><span>${icon('code')}SKILL.md <span class="source-version">Version ${skill.versions.findIndex(v=>v.digest===preview.digest)+1}</span></span><span>Read source</span></summary><pre tabindex="0" aria-label="Skill source">${preview.text.split('\n').map((line,i)=>`<span id="skill-line-${i+1}" data-number="${i+1}">${esc(line)||' '}</span>`).join('')}</pre><p class="skill-source-path">${esc(preview.host)} · ${esc(preview.path)}</p></details>
    <details class="skill-policy"><summary>${icon('settings')}Lint policy <span>${maxWords} words</span></summary><label>Main instruction budget <input id="skill-word-budget" type="number" min="50" max="2000" step="50" value="${maxWords}" ${pending||data.busy?'disabled':''}> words</label><p>References and scripts don’t count toward the instruction budget. Applied when you distribute.</p></details>`;
  }
  $('#skills-search').oninput=renderList;
  $('#skills-filters').onclick=e=>{const b=e.target.closest('[data-filter]');if(b){filter=b.dataset.filter;renderList();}};
  $('#skills-list').onclick=e=>{const b=e.target.closest('[data-open]');if(b){active=b.dataset.open;previewDigest='';history.replaceState(null,'',`#skill=${encodeURIComponent(active)}`);renderList();renderDetail();if(innerWidth<850)$('#skills-detail').scrollIntoView({behavior:'smooth',block:'start'});}};
  $('#skills-list').onchange=e=>{const name=e.target.dataset.select;if(name){e.target.checked?selected.add(name):selected.delete(name);changes();}};
  $('#skills-detail').onclick=e=>{
    const choose=e.target.closest('[data-choose]'), preview=e.target.closest('[data-preview]'), line=e.target.closest('[data-line]');
    if(choose){const s=skills.find(s=>s.key===active);choices[s.name]=choose.dataset.choose;previewDigest=choose.dataset.choose;changes();}
    if(preview){previewDigest=preview.dataset.preview;renderDetail();document.querySelector(`[data-preview="${CSS.escape(previewDigest)}"]`)?.focus({preventScroll:true});}
    if(line){$('.skill-source').open=true;const target=$(`#skill-line-${line.dataset.line}`);target?.scrollIntoView({behavior:'smooth',block:'center'});target?.classList.add('highlight');}
  };
  $('#skills-detail').onchange=e=>{if(e.target.id==='skill-word-budget'){if(!e.target.reportValidity())return;maxWords=Number(e.target.value);dirty=true;$('#skills-reset').hidden=false;$('#selection-summary').textContent+=' · Unsaved policy';}};
  $('#skills-reset').onclick=()=>{dirty=false;previewDigest='';poll();};
  async function action(kind){
    if(pending||data.busy)return;pending=true;$('#skills-error').hidden=true;render();
    try{const value=await request({action:kind,revision:data.revision,selected:[...selected],choices,max_words:maxWords});if(kind==='distribute')dirty=false;use(value,kind==='distribute');}
    catch(e){fail(e);if(!data.busy)timer=setTimeout(poll,1200);}
    finally{pending=false;render();}
  }
  $('#skills-scan').onclick=()=>action('scan');$('#skills-distribute').onclick=()=>action('distribute');
  document.addEventListener('keydown',e=>{if(e.key==='/'&&!e.metaKey&&!e.ctrlKey&&!/INPUT|TEXTAREA|SELECT/.test(document.activeElement.tagName)){e.preventDefault();$('#skills-search').focus();}});
  window.addEventListener('beforeunload',e=>{if(dirty){e.preventDefault();e.returnValue='';}});
  async function start(){
    HeyBossUI.icons();$('#nav-skills')?.setAttribute('aria-current','page');
    // Skills span the fleet, so the project selector links back to project work.
    const bootResponse=await fetch('/api/bootstrap'),boot=await bootResponse.json();
    if(!bootResponse.ok)throw Error(boot.error?.message||'Could not connect');csrf=boot.csrf;
    const picker=new HeyBossUI.ProjectPicker({onSelect:id=>location.href='/#'+new URLSearchParams({project:id})});picker.update(boot.projects||[],null);
    $('#project-name').textContent='All projects';
    active=new URLSearchParams(location.hash.slice(1)).get('skill')||'';
    use(await request(),true);if(!data.busy)await action('scan');
  }
  start().catch(fail);
})();
