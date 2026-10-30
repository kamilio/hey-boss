"use strict";
function library(data) {
  const entries = new Map();
  for (const machine of data.machines || []) for (const source of machine.copies || []) {
    const key = `${source.scope}:${source.name}`;
    if (!entries.has(key)) entries.set(key, {key, name:source.name, scope:source.scope, copies:[], versions:[], machines:[]});
    const skill = entries.get(key);
    const copy = {...source, host:machine.host, hostname:machine.host === 'local' ? 'This machine' : machine.host, stale:machine.state === 'attention'};
    skill.copies.push(copy);
    if (!skill.versions.some(v=>v.digest===copy.digest)) skill.versions.push(copy);
    if (!skill.machines.includes(machine.host)) skill.machines.push(machine.host);
  }
  for (const name of data.disabled || []) {
    const key = `global:${name}`;
    if (!entries.has(key)) entries.set(key, {key,name,scope:'global',copies:[],versions:[],machines:[]});
    entries.get(key).disabled = true;
  }
  return [...entries.values()].sort((a,b)=>{
    const rank = n => n === 'AGENTS.md' ? 0 : n === 'hey-boss' ? 1 : 2;
    return rank(a.name) - rank(b.name) || a.name.localeCompare(b.name) || a.scope.localeCompare(b.scope);
  });
}
function needsAttention(skill, machines) {
  if (skill.disabled) return skill.copies.length > 0 || (machines || []).some(m=>m.error);
  if (Array.isArray(machines) && machines.length > 1) {
    return !machineCoverage(skill, machines).allGreen || skill.copies.some(c => c.stale);
  }
  return skill.versions.length > 1 || skill.copies.some(c => c.stale);
}
function visibleSkills(skills, query, filter, selected, machines) {
  query = query.trim().toLocaleLowerCase();
  return skills.filter(s=>(filter==='project'?s.scope==='project':s.scope==='global') &&
    (filter!=='selected'||(!s.disabled&&selected.has(s.name))) && (filter!=='disabled'||s.disabled) && (filter!=='attention'||needsAttention(s, machines)) &&
    `${s.name} ${s.copies.map(c=>`${c.description} ${c.host} ${c.hostname}`).join(' ')}`.toLocaleLowerCase().includes(query));
}
function unresolved(skills, selected, choices) {
  return skills.filter(s=>!s.disabled&&s.scope==='global'&&selected.has(s.name)&&
    (choices[s.name]?!s.versions.some(v=>v.digest===choices[s.name]):s.versions.length>1)).map(s=>s.name);
}
function computeDiff(oldText, newText, contextLines = 3) {
  const a = String(oldText ?? '').replace(/\r\n/g, '\n').split('\n');
  const b = String(newText ?? '').replace(/\r\n/g, '\n').split('\n');
  if (a.length === 1 && a[0] === '' && oldText === '') a.length = 0;
  if (b.length === 1 && b[0] === '' && newText === '') b.length = 0;
  const n = a.length, m = b.length;
  const dp = Array.from({length: n + 1}, () => new Int32Array(m + 1));
  for (let i = n - 1; i >= 0; i--) {
    for (let j = m - 1; j >= 0; j--) {
      dp[i][j] = a[i] === b[j] ? dp[i + 1][j + 1] + 1 : Math.max(dp[i + 1][j], dp[i][j + 1]);
    }
  }
  const raw = [];
  let i = 0, j = 0, oldNo = 1, newNo = 1, additions = 0, deletions = 0;
  while (i < n && j < m) {
    if (a[i] === b[j]) {
      raw.push({type: 'same', text: a[i], oldLine: oldNo++, newLine: newNo++});
      i++; j++;
    } else if (dp[i + 1][j] >= dp[i][j + 1]) {
      raw.push({type: 'del', text: a[i], oldLine: oldNo++, newLine: null});
      deletions++; i++;
    } else {
      raw.push({type: 'add', text: b[j], oldLine: null, newLine: newNo++});
      additions++; j++;
    }
  }
  while (i < n) { raw.push({type: 'del', text: a[i++], oldLine: oldNo++, newLine: null}); deletions++; }
  while (j < m) { raw.push({type: 'add', text: b[j++], oldLine: null, newLine: newNo++}); additions++; }
  const changedIdx = [];
  raw.forEach((line, idx) => { if (line.type !== 'same') changedIdx.push(idx); });
  if (!changedIdx.length) return {additions: 0, deletions: 0, hunks: [], lines: raw};
  const ranges = [];
  for (const idx of changedIdx) {
    const start = Math.max(0, idx - contextLines);
    const end = Math.min(raw.length - 1, idx + contextLines);
    if (ranges.length && start <= ranges[ranges.length - 1][1] + 1) {
      ranges[ranges.length - 1][1] = Math.max(ranges[ranges.length - 1][1], end);
    } else {
      ranges.push([start, end]);
    }
  }
  const hunks = ranges.map(([start, end]) => {
    const slice = raw.slice(start, end + 1);
    const oldLines = slice.filter(l => l.oldLine != null);
    const newLines = slice.filter(l => l.newLine != null);
    const oldStart = oldLines.length ? oldLines[0].oldLine : 1;
    const newStart = newLines.length ? newLines[0].newLine : 1;
    return {
      header: `@@ -${oldStart},${oldLines.length} +${newStart},${newLines.length} @@`,
      lines: slice
    };
  });
  return {additions, deletions, hunks, lines: raw};
}
function getMarkdownFiles(copy) {
  if (Array.isArray(copy?.markdown_files) && copy.markdown_files.length) return copy.markdown_files;
  const primary = copy?.name === 'AGENTS.md' ? 'AGENTS.md' : 'SKILL.md';
  return [{path: primary, text: String(copy?.text ?? '')}];
}
function diffFiles(baseCopy, compareCopy) {
  const baseMap = new Map(getMarkdownFiles(baseCopy).map(f => [f.path, f.text]));
  const compMap = new Map(getMarkdownFiles(compareCopy).map(f => [f.path, f.text]));
  const allPaths = [...new Set([...baseMap.keys(), ...compMap.keys()])];
  return allPaths.map(path => {
    const oldText = baseMap.get(path) ?? '';
    const newText = compMap.get(path) ?? '';
    const diff = computeDiff(oldText, newText);
    const status = !baseMap.has(path) ? 'added' : !compMap.has(path) ? 'removed' : (diff.additions || diff.deletions) ? 'modified' : 'unchanged';
    return {path, status, ...diff};
  });
}
const shortHost = host => String(host || '').replace(/\.local$/i, '') || 'local';
const escapeSkillHtml = value => String(value ?? '').replace(/[&<>"']/g, c => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));
function rolloutFeedback(data) {
  const failures = (data.machines || []).filter(machine => machine.error);
  const failed = !data.busy && (data.error || failures.length);
  const message = data.error || data.message || (data.busy ? 'Updating your machines…' : '');
  if (!message && !failed) return '';
  return `<div class="skill-rollout-feedback ${failed ? 'has-error' : ''}" role="${failed ? 'alert' : 'status'}">
    <p>${escapeSkillHtml(message)}</p>
    ${!data.busy ? failures.map(machine => `<p><strong>${escapeSkillHtml(shortHost(machine.host))}:</strong> ${escapeSkillHtml(machine.error)}</p>`).join('') : ''}
    ${failed ? '<button type="button" class="button small" data-refresh-inventory>Refresh versions</button>' : ''}
  </div>`;
}
function machineCoverage(skill, machines) {
  const known = [...new Set((machines || []).map(m => typeof m === 'string' ? m : m?.host).filter(Boolean))];
  const present = [...new Set(skill?.machines || [])];
  const missing = known.filter(h => !present.includes(h));
  const onAllMachines = known.length > 1 && missing.length === 0 && present.length >= known.length;
  const inSync = (skill?.versions || []).length <= 1;
  const attention = (machines || []).filter(m => m?.error || m?.state === 'attention').map(m => m.host);
  const allGreen = onAllMachines && inSync && !attention.length;
  const tone = allGreen
    ? 'green'
    : (!inSync || (known.length > 1 && present.length <= 1) ? 'red' : 'orange');
  const tags = allGreen
    ? [{ label: 'All machines', host: 'all', tone: 'green', isAll: true, title: `Present and unified on all ${known.length} machines (${present.map(shortHost).join(', ')})` }]
    : present.map(h => ({
        label: shortHost(h),
        host: h,
        tone,
        isAll: false,
        title: attention.includes(h) ? `Could not verify ${h}; review the rollout results` : missing.length
          ? `On ${h} · missing on ${missing.map(shortHost).join(', ')}`
          : (!inSync ? `On ${h} · multiple versions across machines` : `Only on ${h}`)
      }));
  return { known, present, missing, attention, onAllMachines, inSync, allGreen, tone, tags };
}
if(typeof module!=='undefined') module.exports={library,visibleSkills,unresolved,computeDiff,diffFiles,getMarkdownFiles,machineCoverage,shortHost,rolloutFeedback};
if(typeof document!=='undefined') (()=>{
  const $=s=>document.querySelector(s), esc=s=>String(s??'').replace(/[&<>"']/g,c=>({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c])), icon=HeyBossUI.icon;
  let data={machines:[],selected:[],choices:{}}, skills=[], selected=new Set(), choices={}, active='', filter='all', csrf='', dirty=false, pending=false, timer, previewDigest='', compareDigest='', maxWords=400;
  let activeFile='', editingMarkdown=false, editorDraft='', codeMirrorInstance=null, editorScriptPromise=null, deletePromptSkill='';
  let actionError='', actorId='', selectionRevision=0;
  let editorContext=null, editorStatus='', skipDraftRestore=false;
  const draftMemory=new Map();
  const draftKey=(skill,file)=>'hey-boss-skill-draft:'+JSON.stringify([actorId,skill,file]);
  function readDraft(key){
    if(draftMemory.has(key))return draftMemory.get(key);
    try{const value=JSON.parse(localStorage.getItem(key));if(typeof value?.content==='string'&&typeof value?.baseDigest==='string'){draftMemory.set(key,value);return value;}}catch{}
    return null;
  }
  function writeDraft(key,value){
    draftMemory.set(key,value);
    try{localStorage.setItem(key,JSON.stringify(value));return true;}catch{return false;}
  }
  function removeDraft(key){draftMemory.delete(key);try{localStorage.removeItem(key);}catch{}}
  function showEditorStatus(message){
    editorStatus=message;
    const el=$('[data-editor-status]');if(el)el.textContent=message;
  }
  function keepDraft(){
    if(!editingMarkdown||!editorContext)return;
    editorDraft=currentEditorContent();
    const stored=writeDraft(editorContext.key,{content:editorDraft,baseDigest:editorContext.baseDigest});
    showEditorStatus(stored?'Draft saved in this browser':'Draft is only in this tab · browser storage unavailable');
  }
  function beginEditor(skill,copy,file){
    const key=draftKey(skill.key,file.path),draft=readDraft(key);
    editorContext={key,skill:skill.key,file:file.path,baseDigest:draft?.baseDigest||copy.digest};
    editorDraft=draft?.content??file.text;
    editorStatus=draft?'Draft restored from this browser':'Changes are saved as a browser draft';
    editingMarkdown=true;skipDraftRestore=false;
  }
  const labels={codex:'Codex',claude:'Claude',agents:'Shared agents',project:'Repository',library:'Saved library'};
  function renderMachineTags(skill, includeMissing=false){
    const cov = machineCoverage(skill, data.machines);
    const pills = cov.tags.map(t => `<span class="machine-tag is-${esc(t.tone)}" title="${esc(t.title)}">${esc(t.label)}</span>`).join('');
    const conflictPill = !cov.inSync ? `<span class="machine-tag is-red" title="Different content across machines">${skill.versions.length} versions · Diff</span>` : '';
    const missingPill = includeMissing && cov.missing.length ? `<span class="machine-tag is-missing" title="Not installed on ${esc(cov.missing.join(', '))}">Missing: ${esc(cov.missing.map(shortHost).join(', '))}</span>` : '';
    return `<span class="machine-tags">${pills}${conflictPill}${missingPill}</span>`;
  }
  function openDeleteSkillModal(skillName, trigger){
    const skill=skills.find(s=>s.name===skillName&&s.scope==='global')||{name:skillName,copies:[],machines:[]};
    deletePromptSkill=skill.name;
    let dialog=$('#skill-delete-dialog');
    if(!dialog){
      dialog=document.createElement('dialog');
      dialog.id='skill-delete-dialog';
      dialog.className='skill-delete-dialog';
      dialog.setAttribute('aria-labelledby','skill-delete-title');
      dialog.setAttribute('aria-describedby','skill-delete-description');
      document.body.append(dialog);
    }
    const copyCount=skill.copies?.length||1;
    const machineNames=(skill.machines||[]).map(shortHost).join(', ')||'connected machines';
    dialog.innerHTML=`<div class="skill-delete-dialog-head">
      <span class="skill-delete-dialog-icon">${icon('trash')||icon('alert')||'×'}</span>
      <div>
        <span class="skills-eyebrow">REMOVE SKILL</span>
        <h2 id="skill-delete-title">Delete skill from all machines?</h2>
      </div>
    </div>
    <div class="skill-delete-target">
      <strong class="skill-delete-name">${esc(skill.name)}</strong>
      <span class="skill-delete-meta">${copyCount} ${copyCount===1?'copy':'copies'} · ${esc(machineNames)}</span>
    </div>
    <p id="skill-delete-description" class="skill-delete-body">This removes <strong>${esc(skill.name)}</strong> from Codex, Claude Code, and shared agent skill folders across all connected machines.</p>
    <p class="skill-delete-hint">A timestamped backup copy is automatically saved in <code>.hey-boss/skill-backups</code> on each machine.</p>
    <p id="skill-delete-error" class="skill-delete-error" role="alert" hidden></p>
    <div class="skill-delete-actions">
      <button type="button" class="button" id="skill-delete-cancel" data-delete-cancel autofocus>Cancel</button>
      <button type="button" class="button danger" id="skill-delete-confirm" data-delete-confirm="${esc(skill.name)}" ${pending||data.busy?'disabled':''}>Delete skill</button>
    </div>`;
    const closeDialog=()=>{
      deletePromptSkill='';
      if(typeof dialog.close==='function'&&dialog.open) dialog.close();
      else dialog.removeAttribute('open');
      if(trigger?.isConnected) trigger.focus({preventScroll:true});
    };
    const cancelBtn=dialog.querySelector('[data-delete-cancel]');
    const confirmBtn=dialog.querySelector('[data-delete-confirm]');
    if(cancelBtn) cancelBtn.onclick=closeDialog;
    if(confirmBtn) confirmBtn.onclick=async()=>{
      if(pending)return;
      cancelBtn.disabled=true;
      confirmBtn.disabled=true;
      confirmBtn.textContent='Deleting…';
      editingMarkdown=false;
      await action('delete',{skill:skill.name});
      closeDialog();
    };
    dialog.oncancel=e=>{
      if(pending){e.preventDefault();return;}
      deletePromptSkill='';
      if(trigger?.isConnected) setTimeout(()=>trigger.focus({preventScroll:true}),0);
    };
    if(typeof dialog.showModal==='function'){
      if(!dialog.open) dialog.showModal();
    }else{
      dialog.setAttribute('open','');
    }
    cancelBtn?.focus({preventScroll:true});
  }
  function renderFeedback(){document.querySelectorAll('[data-rollout-feedback]').forEach(el=>{el.innerHTML=rolloutFeedback({...data,busy:pending||data.busy,message:pending?'Sending request…':data.message,error:actionError});});}
  function fail(e){actionError=e.message;$('#skills-error').textContent=e.message;$('#skills-error').hidden=false;renderFeedback();}
  function loadEditorBundle(){
    if(window.HeyBossArtifactEditor)return Promise.resolve();
    return editorScriptPromise ||= new Promise((resolve,reject)=>{
      const s=document.createElement('script');
      s.src='/artifact-editor.js';
      s.onload=resolve;
      s.onerror=()=>{editorScriptPromise=null;s.remove();reject(Error('Could not load Markdown editor'));};
      document.head.appendChild(s);
    });
  }
  async function request(body){
    const payload=body?JSON.stringify(body):undefined;
    for(let attempt=0;attempt<2;attempt++){
      const sentToken=csrf;
      const response=await fetch('/api/skills',{method:body?'POST':'GET',headers:body?{'Content-Type':'application/json','X-Hey-Boss-CSRF':sentToken}:{},body:payload,signal:AbortSignal.timeout(15000)});
      const value=await response.json();
      // A rejected CSRF check runs before the action. Only replay that rejection,
      // once, with the same actor and payload; interrupted writes are ambiguous.
      if(body&&attempt===0&&response.status===403&&value.error?.code==='forbidden'){
        try{
          const bootstrap=await fetch('/api/bootstrap',{cache:'no-store',signal:AbortSignal.timeout(10000)}),fresh=await bootstrap.json();
          if(bootstrap.ok&&fresh.ok&&typeof fresh.csrf==='string'&&fresh.csrf&&fresh.csrf!==sentToken&&actorId&&fresh.actor?.id===actorId){
            csrf=fresh.csrf;
            continue;
          }
        }catch{/* Keep the original failure and the current page state. */}
      }
      if(!response.ok||value.ok===false)throw Error(value.error?.message||value.error||'Could not reach skill manager');
      return value;
    }
  }
  function use(value,reset=false){
    data=value;skills=library(data);
    if(reset||!dirty){selected=new Set(data.selected);choices={...data.choices};maxWords=data.max_words||400;}
    for(const name of data.disabled||[])selected.delete(name);
    if(!editingMarkdown&&!skills.some(s=>s.key===active))active=skills[0]?.key||'';
    render();
    clearTimeout(timer);if(data.busy)timer=setTimeout(poll,1200);
  }
  async function poll(){try{use(await request());}catch(e){fail(e);timer=setTimeout(poll,4000);}}
  const current = skill => choices[skill.name] ? skill.versions.find(v=>v.digest===choices[skill.name]) : (skill.versions.length===1?skill.versions[0]:null);
  function changes(){selectionRevision++;dirty=true;$('#skills-error').hidden=true;render();}
  function destroyEditor(){if(codeMirrorInstance){codeMirrorInstance.destroy();codeMirrorInstance=null;}}
  function currentEditorContent(){
    if(codeMirrorInstance)return codeMirrorInstance.content();
    const ta=$('#skill-markdown-textarea');
    return ta?ta.value:editorDraft;
  }
  function render(){
    const focus=document.activeElement;
    const restore=['select','choose','open','preview'].find(key=>focus?.dataset?.[key]);
    const focusValue=restore?focus.dataset[restore]:null;
    const global=skills.filter(s=>s.scope==='global'), busy=pending||data.busy;
    $('#skills-main').classList.toggle('is-busy',!!busy);
    $('#skills-summary').innerHTML=[[global.length,'skills discovered'],[global.filter(s=>selected.has(s.name)).length,'selected to sync'],[global.filter(s=>needsAttention(s,data.machines)).length,'not on all machines'],[(data.machines||[]).filter(m=>m.scanned_at).length,'machines scanned']].map(([n,label])=>`<div><strong>${n}</strong><span>${label}</span></div>`).join('');
    const machines=data.machines||[], attention=machines.filter(m=>m.error).length;
    $('#machine-summary').textContent=busy?'Scanning or syncing…':`${machines.length} known${attention?` · ${attention} need attention`:''}`;
    $('#skills-machines').innerHTML=machines.map(m=>`<article class="machine-card"><div>${icon('monitor')}<strong>${esc(m.hostname||m.host)}</strong><span class="skill-pill ${m.error?'amber':'green'}">${m.error?'Needs attention':m.state==='synced'?'Synced':'Online'}</span></div><p>${esc(m.host==='local'?'This machine':m.host)} · ${(m.copies||[]).filter(c=>c.scope==='global').length} copies${m.scanned_at?` · ${esc(new Date(m.scanned_at*1000).toLocaleTimeString([],{hour:'2-digit',minute:'2-digit'}))}`:''}</p>${m.error?`<p class="machine-error">${esc(m.error)}</p>`:''}</article>`).join('')||'<p>No machine inventory yet. Scan to discover your skills.</p>';
    renderList();
    if(!editingMarkdown)renderDetail();
    else document.querySelectorAll('[data-save-markdown], [data-cancel-editor]').forEach(button=>{button.disabled=!!busy;});
    const draftConflict=$('[data-draft-conflict]');
    if(draftConflict)draftConflict.hidden=!!skills.find(s=>s.key===active)?.versions.some(v=>v.digest===editorContext?.baseDigest);
    renderFeedback();
    const conflicts=unresolved(skills,selected,choices), count=global.filter(s=>selected.has(s.name)).length;
    $('#selection-summary').textContent=`${count} skill${count===1?'':'s'} selected${dirty?' · Unsaved changes':''}`;
    $('#skills-status').textContent=conflicts.length?`Pick a version to keep for ${conflicts.join(', ')} before distributing all.`:data.message||'Scan your machines to build the library.';
    $('#skills-distribute').disabled=!!busy||!skills.length||!!conflicts.length;
    $('#skills-scan').disabled=!!busy;
    $('#skills-reset').hidden=!dirty;$('#skills-reset').disabled=!!busy;
    if(restore&&!editingMarkdown)document.querySelector(`[data-${restore}="${CSS.escape(focusValue)}"]`)?.focus({preventScroll:true});
  }
  function renderList(){
    const visible=visibleSkills(skills,$('#skills-search').value,filter,selected,data.machines);
    $('#library-count').textContent=visible.length;
    $('#skills-filters').querySelectorAll('button').forEach(b=>b.setAttribute('aria-pressed',String(b.dataset.filter===filter)));
    $('#skills-list').innerHTML=visible.map(s=>{
      if(s.disabled)return `<div class="skill-row ${active===s.key?'is-active':''}"><span class="skill-project-icon">${icon('instructions')}</span><button class="skill-open" data-open="${esc(s.key)}" aria-current="${active===s.key}"><span class="skill-row-title">${esc(s.name)}<span class="skill-required">DISABLED</span></span><span class="skill-row-description">${needsAttention(s,data.machines)?'Some machines need attention':'Archived · Enable to restore'}</span></button><span class="skill-row-arrow">${icon('arrow-right')}</span></div>`;
      const version=current(s)||s.versions[0];
      const badge = s.name==='hey-boss'?'<span class="skill-required">CORE</span>':s.name==='AGENTS.md'?'<span class="skill-required codex-badge">CODEX</span>':'';
      const canDelete = s.scope==='global' && s.name!=='hey-boss';
      return `<div class="skill-row ${active===s.key?'is-active':''} ${selected.has(s.name)&&s.scope==='global'?'is-selected':''}">${s.scope==='global'?`<input type="checkbox" data-select="${esc(s.name)}" aria-label="Distribute ${esc(s.name)}" ${selected.has(s.name)?'checked':''} ${s.name==='hey-boss'||pending||data.busy?'disabled':''}>`:`<span class="skill-project-icon">${icon('folder')}</span>`}<button class="skill-open" data-open="${esc(s.key)}" aria-current="${active===s.key}"><span class="skill-row-title">${esc(s.name)}${badge}</span><span class="skill-row-description">${esc(version.description||'No description yet')}</span><span class="skill-row-meta">${renderMachineTags(s,false)}</span></button>${canDelete?`<button type="button" class="skill-row-delete" data-delete-row="${esc(s.name)}" title="Delete ${esc(s.name)}" aria-label="Delete ${esc(s.name)}" ${pending||data.busy?'disabled':''}>${icon('trash')||icon('x')||'×'}</button>`:`<span class="skill-row-arrow">${icon('arrow-right')}</span>`}</div>`;
    }).join('')||`<div class="skills-empty">${icon(skills.length?'search':'instructions')}<h3>${skills.length?'No matching skills':'Your library starts here'}</h3><p>${skills.length?'Try another search or filter.':'Scan all machines to discover your existing skills.'}</p></div>`;
  }
  function renderDiffSection(skill, chosen, preview){
    if(skill.versions.length<=1)return '';
    const baseVersion = chosen || skill.versions[0];
    const altDefault = skill.versions.find(v => v.digest !== baseVersion.digest) || skill.versions[1] || baseVersion;
    const compareVersion = skill.versions.find(v => v.digest === compareDigest && v.digest !== baseVersion.digest) || (preview.digest !== baseVersion.digest ? preview : altDefault);
    const baseIdx = skill.versions.findIndex(v => v.digest === baseVersion.digest) + 1;
    const compIdx = skill.versions.findIndex(v => v.digest === compareVersion.digest) + 1;
    const filesDiff = diffFiles(baseVersion, compareVersion);
    const totalAdd = filesDiff.reduce((sum,f)=>sum+f.additions,0);
    const totalDel = filesDiff.reduce((sum,f)=>sum+f.deletions,0);
    const changedFiles = filesDiff.filter(f => f.status !== 'unchanged');
    return `<section class="skill-section gh-diff-section" aria-label="Version differences">
      <div class="gh-diff-toolbar">
        <div class="gh-diff-summary">
          <strong>Version differences</strong>
          <span class="gh-diff-range">Red: Version ${baseIdx} · Green: Version ${compIdx}</span>
          <span class="gh-diff-stats"><span class="diff-add-stat">+${totalAdd}</span> <span class="diff-del-stat">−${totalDel}</span></span>
        </div>
      </div>
      ${changedFiles.map(file => `<div class="gh-diff-file">
        <div class="gh-diff-file-header">
          <span class="gh-diff-file-path">${icon('code')}<code>${esc(file.path)}</code> <small>(${esc(file.status)})</small></span>
          <span class="gh-diff-file-meta">
            <span class="diff-add-stat">+${file.additions}</span>
            <span class="diff-del-stat">−${file.deletions}</span>
            <button type="button" class="button small" data-edit-file="${esc(file.path)}">${icon('edit')||''}Edit</button>
          </span>
        </div>
        <div class="gh-diff-table" role="region" aria-label="Diff for ${esc(file.path)}">
          ${file.hunks.map(hunk => `<div class="gh-diff-hunk-header">${esc(hunk.header)}</div>${hunk.lines.map(l => `<div class="gh-diff-line is-${l.type}"><span class="gh-diff-num old">${l.oldLine ?? ''}</span><span class="gh-diff-num new">${l.newLine ?? ''}</span><span class="gh-diff-sign">${l.type==='add'?'+':l.type==='del'?'-':' '}</span><span class="gh-diff-code">${esc(l.text)||' '}</span></div>`).join('')}`).join('')}
        </div>
      </div>`).join('') || '<p class="gh-diff-identical">Markdown contents are identical across these versions.</p>'}
    </section>`;
  }
  function renderDetail(){
    destroyEditor();
    const skill=skills.find(s=>s.key===active);
    if(!skill){$('#skills-detail').innerHTML='<div class="skills-empty"><h2>Select a skill</h2><p>Scan your machines to build the library.</p></div>';return;}
    if(skill.disabled){
      const attention=needsAttention(skill,data.machines);
      $('#skills-detail').innerHTML=`<div class="skill-detail-heading"><div class="skill-mark">${icon('instructions')}</div><div><span class="skills-eyebrow">GLOBAL SKILL · DISABLED</span><h2>${esc(skill.name)}</h2></div><div class="skill-header-actions"><button type="button" class="button small primary" data-enable-skill="${esc(skill.name)}" ${pending||data.busy?'disabled':''}>Enable skill</button>${attention?`<button type="button" class="button small" data-disable-skill="${esc(skill.name)}" ${pending||data.busy?'disabled':''}>Retry disable</button>`:''}</div></div><p class="skill-description">The complete folders are kept in a private archive on each machine. Enable restores them to their original locations.</p>${attention?'<p class="skill-draft-notice">Some machines may still have installed copies. Review the machine errors below and retry.</p>':''}<div data-rollout-feedback></div>`;
      renderFeedback();return;
    }
    const chosen=current(skill), preview=skill.versions.find(v=>v.digest===previewDigest)||chosen||skill.versions[0], warnings=preview.warnings||[];
    const copies=skill.copies.filter(c=>c.digest===preview.digest), hasAgent=a=>copies.some(c=>c.agent===a||(a==='codex'&&c.agent==='agents'));
    const mdFiles = getMarkdownFiles(preview);
    if(!activeFile || !mdFiles.some(f=>f.path===activeFile)) activeFile = mdFiles[0]?.path || 'SKILL.md';
    const currentFileObj = mdFiles.find(f=>f.path===activeFile) || mdFiles[0] || {path:'SKILL.md',text:preview.text||''};
    if(!editingMarkdown&&!skipDraftRestore&&readDraft(draftKey(skill.key,currentFileObj.path)))beginEditor(skill,preview,currentFileObj);
    const ignored = preview.ignored_files || [];
    const canDelete = skill.scope==='global' && skill.name!=='hey-boss';
    $('#skills-detail').innerHTML=`<div class="skill-detail-heading">
      <div class="skill-mark">${icon('instructions')}</div>
      <div>
        <span class="skills-eyebrow">${skill.name==='AGENTS.md'?'MAIN CODEX SKILL · .codex/AGENTS.md':skill.scope==='project'?'PROJECT SKILL · AUDIT ONLY':'SKILL INSPECTOR'}</span>
        <h2>${esc(skill.name)}</h2>
      </div>
      <div class="skill-header-actions">
        <button type="button" class="button small primary" data-open-native-editor="${esc(currentFileObj.path)}" title="Open in Hey Boss native Markdown editor" ${pending||data.busy?'disabled':''}>Edit</button>
        <button type="button" class="button small" data-toggle-editor="${esc(currentFileObj.path)}" ${pending||data.busy?'disabled':''}>Edit in browser</button>
        <button type="button" class="button small" data-open-dir="${esc(skill.name)}" title="Open directory in Finder" ${pending||data.busy?'disabled':''}>${icon('folder')||''}Open folder</button>
        ${canDelete&&skill.name!=='AGENTS.md'?`<button type="button" class="button small" data-disable-skill="${esc(skill.name)}" ${pending||data.busy?'disabled':''} title="Archive the complete folders and remove them from all agent skill locations">Disable skill</button>`:''}
        ${canDelete?`<button type="button" class="button small danger" data-delete-prompt="${esc(skill.name)}" ${pending||data.busy?'disabled':''}>Delete skill</button>`:''}
      </div>
    </div>
    <p class="skill-description">${esc(preview.description||'Add a clear description to help agents discover this skill.')}</p>
    <div class="skill-targets">
      ${renderMachineTags(skill,true)}
      <span class="agent-target ${hasAgent('codex')?'present':''}">${icon('code')}Codex <small>${hasAgent('codex')?'found':'not found'}</small></span>
      <span class="agent-target ${hasAgent('claude')?'present':''}">${icon('spark')}Claude <small>${hasAgent('claude')?'found':'not found'}</small></span>
      ${(() => {
        const cov = machineCoverage(skill, data.machines);
        return cov.attention.length
          ? '<span class="skill-pill amber">Rollout needs attention</span>'
          : cov.allGreen
          ? '<span class="skill-pill green">Synced on all machines</span>'
          : !cov.inSync
            ? `<span class="skill-pill amber">${skill.versions.length} versions · Pick one to unify</span>`
            : cov.missing.length
              ? `<span class="skill-pill amber">On ${cov.present.length} of ${cov.known.length} machines</span>`
              : '<span class="skill-pill green">Unified</span>';
      })()}
      <span class="skill-size">${preview.word_count||0} words · ${(currentFileObj.text||'').split('\n').length} lines</span>
    </div>
    <section class="skill-section">
      <div class="skill-section-title"><h3>Versions across machines <span>${skill.versions.length}</span></h3><span>${(() => {
        const cov = machineCoverage(skill, data.machines);
        return cov.attention.length ? 'Review the rollout results below' : !cov.inSync
          ? 'Pick one version to unify across all machines'
          : cov.missing.length
            ? `Missing on ${cov.missing.map(shortHost).join(', ')} · Distribute to install everywhere`
            : `In sync across all ${cov.known.length || 1} machines`;
      })()}</span></div>
      <div data-rollout-feedback></div>
      ${skill.versions.length>1?'<p class="skill-conflict-note">This skill differs across machines. Review the differences below and pick the version you want to keep everywhere.</p>':''}
      <div class="skill-versions">${skill.versions.map((v,i)=>{
        const locations=skill.copies.filter(c=>c.digest===v.digest);
        const used=locations.some(c=>c.agent!=='library');
        const locationRows=[...new Set(locations.map(c=>c.host))].map(host=>{const copies=locations.filter(c=>c.host===host);return `<div><strong>${esc(host==='local'?'This machine':shortHost(host))}</strong><span>${esc(copies.map(c=>labels[c.agent]||c.agent).join(' · '))}</span></div>`;}).join('');
        return `<div class="skill-version ${preview.digest===v.digest?'is-previewing':''}">
          <button class="version-preview" data-preview="${esc(v.digest)}" aria-pressed="${preview.digest===v.digest}">
            <span class="version-symbol">V${i+1}</span>
            <span><strong>${used?'Used by agents':'Saved copy only'}</strong><small>${v.word_count||0} words · ${preview.digest===v.digest?'Viewing this version':'View this version'}${locations.every(c=>c.stale)?' · Last seen':''}</small></span>
          </button>
          <div class="version-locations">${locationRows}<details><summary>File paths</summary>${locations.map(c=>`<p>${esc(shortHost(c.host))} · ${esc(labels[c.agent]||c.agent)}<code>${esc(c.path)}</code></p>`).join('')}</details></div>
          ${skill.scope==='global'?(() => {
            const cov = machineCoverage(skill, data.machines);
            if (cov.allGreen && skill.versions.length === 1) {
              return `<div class="version-buttons"><span class="version-choose chosen">${icon('check')}Synced everywhere</span></div>`;
            }
            return `<div class="version-buttons">
              <button type="button" class="button small version-choose ${chosen?.digest===v.digest?'chosen':''}" data-choose="${esc(v.digest)}" aria-pressed="${chosen?.digest===v.digest}" aria-label="Select version ${i+1}">${chosen?.digest===v.digest?icon('check')+'Selected':'Select version'}</button>
            </div>`;
          })():''}
        </div>`;
      }).join('')}</div>
      ${skill.scope==='global'&&!machineCoverage(skill,data.machines).allGreen?`<div class="skill-version-action">
        <p id="skill-version-help" role="status">${pending||data.busy
          ? 'Sync in progress. You can select a version now and apply it when this finishes.'
          : chosen ? `Version ${skill.versions.indexOf(chosen)+1} selected. Apply it to all machines when ready.`
          : 'Select a version to apply to all machines.'}</p>
        <button type="button" class="button small primary version-unify" data-unify="${esc(chosen?.digest||'')}" aria-describedby="skill-version-help" ${pending||data.busy||!chosen?'disabled':''}>Use selected version everywhere</button>
      </div>`:''}
    </section>
    ${renderDiffSection(skill, chosen, preview)}
    <section class="skill-section skill-files-section">
      <div class="skill-section-title">
        <h3>Markdown files <span>${mdFiles.length}</span></h3>
        <span>Saves sync everywhere · previous versions kept</span>
      </div>
      ${mdFiles.length > 1 ? `<div class="skill-md-tabs" role="tablist" aria-label="Skill Markdown files">
        ${mdFiles.map(f=>`<button type="button" role="tab" class="skill-md-tab ${f.path===currentFileObj.path?'is-active':''}" aria-selected="${f.path===currentFileObj.path}" data-md-file="${esc(f.path)}">${icon('docs')||icon('code')}<span>${esc(f.path)}${readDraft(draftKey(skill.key,f.path))?' · Draft':''}</span></button>`).join('')}
      </div>` : ''}
      ${!editingMarkdown&&readDraft(draftKey(skill.key,currentFileObj.path))?'<p class="skill-draft-notice">You have a draft saved in this browser. <button class="button small" data-toggle-editor>Resume editing</button></p>':''}
      ${editingMarkdown ? `<div class="skill-markdown-editor-card">
        <div class="skill-markdown-editor-toolbar">
          <strong>Editing <code>${esc(currentFileObj.path)}</code></strong>
          <div class="skill-markdown-editor-actions">
            <button type="button" class="button small" data-cancel-editor>Close editor</button>
            <button type="button" class="button small primary" data-save-markdown="distribute" ${pending||data.busy?'disabled':''}>Save &amp; sync everywhere</button>
          </div>
        </div>
        ${skill.name!=='AGENTS.md'&&currentFileObj.path==='SKILL.md'?'<p class="skill-edit-hint">Changing the frontmatter <code>name</code> also renames the skill folder when you save.</p>':''}
        <p class="skill-editor-status" data-editor-status role="status" aria-live="polite">${esc(editorStatus)}</p>
        <div class="skill-draft-notice" data-draft-conflict ${skill.versions.some(v=>v.digest===editorContext?.baseDigest)?'hidden':''}>The saved version changed. Review the versions above, then <button class="button small" data-rebase-draft>Use viewed version as save base</button>. Your draft text stays unchanged.</div>
        <div id="skill-codemirror-host" class="skill-codemirror-host"></div>
        <textarea id="skill-markdown-textarea" class="skill-markdown-textarea" aria-label="Markdown content for ${esc(currentFileObj.path)}" spellcheck="true">${esc(editorDraft)}</textarea>
      </div>` : `<div class="skill-source-viewer">
        <div class="skill-source-header">
          <span>${icon('code')}<code>${esc(currentFileObj.path)}</code> <span class="source-version">Version ${skill.versions.findIndex(v=>v.digest===preview.digest)+1}</span></span>
          <span class="skill-source-path-inline">${esc(shortHost(preview.host))} · <code>${esc(preview.path)}</code></span>
        </div>
        <pre tabindex="0" aria-label="Skill source">${(currentFileObj.text||'').split('\n').map((line,i)=>`<span id="skill-line-${i+1}" data-number="${i+1}">${esc(line)||' '}</span>`).join('')}</pre>
        <p class="skill-source-path">${esc(preview.host)} · ${esc(preview.path)}</p>
      </div>`}
    </section>`;
    renderFeedback();
    if(editingMarkdown){
      const host=$('#skill-codemirror-host'), ta=$('#skill-markdown-textarea');
      if(ta) ta.oninput=keepDraft;
      loadEditorBundle().then(()=>{
        if(!host||!host.isConnected||!editingMarkdown)return;
        if(ta)ta.hidden=true;codeMirrorInstance=window.HeyBossArtifactEditor(host,editorDraft,()=>{
          editorDraft=codeMirrorInstance.content();
          if(ta)ta.value=editorDraft;
          keepDraft();
        });
      }).catch(()=>{});
    }
  }
  $('#skills-search').oninput=renderList;
  $('#skills-filters').onclick=e=>{const b=e.target.closest('[data-filter]');if(b){filter=b.dataset.filter;renderList();}};
  $('#skills-list').onclick=e=>{
    const delRow=e.target.closest('[data-delete-row]');
    if(delRow){
      e.stopPropagation();
      const s=skills.find(x=>x.name===delRow.dataset.deleteRow&&x.scope==='global');
      if(s){keepDraft();skipDraftRestore=false;active=s.key;editingMarkdown=false;renderList();renderDetail();openDeleteSkillModal(s.name,delRow);}
      return;
    }
    const b=e.target.closest('[data-open]');
    if(b){keepDraft();skipDraftRestore=false;active=b.dataset.open;previewDigest='';compareDigest='';activeFile='';editingMarkdown=false;deletePromptSkill='';const params=new URLSearchParams(location.hash.slice(1));params.set('skill',active);params.delete('file');history.replaceState(null,'',`#${params}`);renderList();renderDetail();if(innerWidth<850)$('#skills-detail').scrollIntoView({behavior:'smooth',block:'start'});}
  };
  $('#skills-list').onchange=e=>{const name=e.target.dataset.select;if(name){e.target.checked?selected.add(name):selected.delete(name);changes();}};
  $('#skills-detail').onclick=async e=>{
    if(e.target.closest('[data-refresh-inventory]')){await action('scan');return;}
    const skill=skills.find(s=>s.key===active);
    if(!skill)return;
    if(e.target.closest('[data-disable-skill], [data-enable-skill]')){
      keepDraft();editingMarkdown=false;skipDraftRestore=true;
      await action(e.target.closest('[data-enable-skill]')?'enable':'disable',{skill:skill.name});return;
    }
    if(e.target.closest('[data-rebase-draft]')&&editorContext){
      editorContext.baseDigest=(skill.versions.find(v=>v.digest===previewDigest)||current(skill)||skill.versions[0]).digest;keepDraft();renderDetail();return;
    }
    const choose=e.target.closest('[data-choose]'), unify=e.target.closest('[data-unify]'), preview=e.target.closest('[data-preview]'), line=e.target.closest('[data-line]');
    const mdTab=e.target.closest('[data-md-file]'), toggleEdit=e.target.closest('[data-toggle-editor]'), editFile=e.target.closest('[data-edit-file]');
    const cancelEdit=e.target.closest('[data-cancel-editor]'), saveMd=e.target.closest('[data-save-markdown]');
    const openDir=e.target.closest('[data-open-dir]'), openNative=e.target.closest('[data-open-native-editor]');
    const delPrompt=e.target.closest('[data-delete-prompt]'), delCancel=e.target.closest('[data-delete-cancel]'), delConfirm=e.target.closest('[data-delete-confirm]');

    if(choose){
      keepDraft();editingMarkdown=false;skipDraftRestore=true;
      choices[skill.name]=choose.dataset.choose;
      selected.add(skill.name);
      previewDigest=choose.dataset.choose;
      changes();
      return;
    }
    if(unify){
      const digest=unify.dataset.unify;
      choices[skill.name]=digest;
      selected.add(skill.name);
      previewDigest=digest;
      await action('distribute',{skill:skill.name,choices:{...choices,[skill.name]:digest},selected:[...selected]});
      return;
    }
    if(preview){
      keepDraft();skipDraftRestore=true;
      previewDigest=preview.dataset.preview;
      compareDigest=preview.dataset.preview;
      editingMarkdown=false;
      renderDetail();
      return;
    }
    if(mdTab){
      keepDraft();
      activeFile=mdTab.dataset.mdFile;
      const params=new URLSearchParams(location.hash.slice(1));params.set('file',activeFile);history.replaceState(null,'','#'+params);
      const previewCopy=skill.versions.find(v=>v.digest===previewDigest)||current(skill)||skill.versions[0];
      const f=getMarkdownFiles(previewCopy).find(x=>x.path===activeFile);
      if(f&&editingMarkdown)beginEditor(skill,previewCopy,f);
      skipDraftRestore=false;
      renderDetail();
      return;
    }
    if(toggleEdit||editFile){
      const targetPath=(toggleEdit?.dataset.toggleEditor)||(editFile?.dataset.editFile)||activeFile||'SKILL.md';
      activeFile=targetPath;
      const previewCopy=skill.versions.find(v=>v.digest===previewDigest)||current(skill)||skill.versions[0];
      const f=getMarkdownFiles(previewCopy).find(x=>x.path===activeFile)||getMarkdownFiles(previewCopy)[0];
      if(editingMarkdown&&editorContext?.file===targetPath)return;
      keepDraft();
      beginEditor(skill,previewCopy,f||{path:targetPath,text:previewCopy.text||''});
      renderDetail();
      $('.skill-markdown-editor-card')?.scrollIntoView({behavior:'smooth',block:'center'});
      return;
    }
    if(cancelEdit){
      keepDraft();skipDraftRestore=true;
      editingMarkdown=false;
      renderDetail();
      return;
    }
    if(saveMd){
      const content=currentEditorContent();
      const distribute=true;
      const previewCopy=skill.versions.find(v=>v.digest===previewDigest)||current(skill)||skill.versions[0];
      keepDraft();
      await action('save_file',{skill:skill.name,file_path:activeFile||'SKILL.md',content,base_digest:editorContext?.baseDigest||previewCopy?.digest||'',distribute});
      return;
    }
    if(openDir){
      const previewCopy=skill.versions.find(v=>v.digest===previewDigest)||current(skill)||skill.versions[0];
      await action('open_dir',{skill:skill.name,base_digest:previewCopy?.digest||''});
      return;
    }
    if(openNative){
      const previewCopy=skill.versions.find(v=>v.digest===previewDigest)||current(skill)||skill.versions[0];
      await action('open_editor',{skill:skill.name,file_path:openNative.dataset.openNativeEditor||activeFile||'SKILL.md',base_digest:previewCopy?.digest||''});
      return;
    }
    if(delPrompt){
      openDeleteSkillModal(delPrompt.dataset.deletePrompt,delPrompt);
      return;
    }
    if(line){
      const target=$(`#skill-line-${line.dataset.line}`);
      target?.scrollIntoView({behavior:'smooth',block:'center'});
      target?.classList.add('highlight');
    }
  };
  $('#skills-detail').onchange=e=>{if(e.target.id==='skill-word-budget'){if(!e.target.reportValidity())return;maxWords=Number(e.target.value);dirty=true;$('#skills-reset').hidden=false;$('#selection-summary').textContent+=' · Unsaved policy';}};
  $('#skills-reset').onclick=()=>{keepDraft();skipDraftRestore=false;dirty=false;previewDigest='';editingMarkdown=false;deletePromptSkill='';poll();};
  async function action(kind, extra={}){
    if(pending||data.busy)return;
    const submittedSelectionRevision=selectionRevision;
    const savingContext=kind==='save_file'?editorContext:null;
    pending=true;actionError='';$('#skills-error').hidden=true;
    if(savingContext)showEditorStatus('Saving… Your draft is preserved.');
    render();
    try{
      const payload={action:kind,revision:data.revision,selected:[...selected],choices,max_words:maxWords,...extra};
      const value=await request(payload);
      if(savingContext){
        const savedName=value.saved_skill||extra.skill, savedKey='global:'+savedName;
        const sameEditor=editingMarkdown&&editorContext===savingContext;
        const draft=readDraft(savingContext.key);
        const newer=draft&&draft.content!==extra.content;
        const digest=value.choices?.[savedName]||savingContext.baseDigest;
        if(newer){
          const key=draftKey(savedKey,savingContext.file);
          if(key!==savingContext.key)removeDraft(savingContext.key);
          writeDraft(key,{...draft,baseDigest:digest});
          if(sameEditor){editorContext={...savingContext,key,skill:savedKey,baseDigest:digest};showEditorStatus('Earlier changes saved · newer edits kept as a draft');}
        }else removeDraft(savingContext.key);
        const keys=new Set(draftMemory.keys());
        try{for(const key of Object.keys(localStorage))keys.add(key);}catch{}
        for(const key of keys){
          if(!key.startsWith('hey-boss-skill-draft:')||key===savingContext.key)continue;
          try{
            const [actor,skill,file]=JSON.parse(key.slice('hey-boss-skill-draft:'.length));
            if(actor!==actorId||skill!==savingContext.skill)continue;
            const other=readDraft(key);if(!other)continue;
            const next=draftKey(savedKey,file);
            writeDraft(next,{...other,baseDigest:other.baseDigest===savingContext.baseDigest?digest:other.baseDigest});
            if(next!==key)removeDraft(key);
          }catch{}
        }
        if(sameEditor){
          editingMarkdown=!!newer;skipDraftRestore=false;
          active=savedKey;previewDigest='';compareDigest='';
          const params=new URLSearchParams(location.hash.slice(1));params.set('skill',active);
          history.replaceState(null,'','#'+params);
        }
      }
      // Choosing a version is local, even while a write is awaiting its reply.
      // That reply acknowledges the submitted choice, not a later selection.
      const resetSelection=selectionRevision===submittedSelectionRevision&&['distribute','save_file','delete','enable','disable'].includes(kind);
      if(resetSelection)dirty=false;
      use(value,resetSelection);
    }
    catch(e){fail(e);if(savingContext&&editingMarkdown)showEditorStatus('Could not save · draft preserved. '+e.message);if(!data.busy)timer=setTimeout(poll,1200);}
    finally{pending=false;render();}
  }
  $('#skills-scan').onclick=()=>action('scan');$('#skills-distribute').onclick=()=>action('distribute');
  document.addEventListener('keydown',e=>{if(e.key==='/'&&!e.defaultPrevented&&!e.isComposing&&!e.metaKey&&!e.ctrlKey&&!e.altKey&&!e.composedPath().some(el=>el instanceof Element&&(el.matches('input,textarea,select')||el.isContentEditable))){e.preventDefault();$('#skills-search').focus();}});
  window.addEventListener('pagehide',keepDraft);
  window.addEventListener('beforeunload',e=>{keepDraft();if(dirty||editingMarkdown){e.preventDefault();e.returnValue='';}});
  async function start(){
    HeyBossUI.icons();$('#nav-skills')?.setAttribute('aria-current','page');
    const bootResponse=await fetch('/api/bootstrap',{cache:'no-store',signal:AbortSignal.timeout(10000)}),boot=await bootResponse.json();
    if(!bootResponse.ok||!boot.ok)throw Error(boot.error?.message||'Could not connect');csrf=boot.csrf;actorId=boot.actor?.id||'';
    const projects=boot.projects||[];
    const initialProjId=HeyBossUI.projectId(boot.project?.id||projects[0]?.id);
    let currentProj=projects.find(p=>p.id===initialProjId||p.name===initialProjId)||boot.project||projects[0]||null;
    const picker=new HeyBossUI.ProjectPicker({onSelect(id){
      currentProj=projects.find(p=>p.id===id||p.name===id)||currentProj;
      if(currentProj){
        picker.update(projects,currentProj);
        const params=new URLSearchParams(location.hash.slice(1));
        params.set('project',currentProj.id);
        history.replaceState(null,'','#'+params);
      }
    }});
    if(currentProj){
      picker.update(projects,currentProj);
      const params=new URLSearchParams(location.hash.slice(1));
      if(!params.get('project')||params.get('project')!==currentProj.id){params.set('project',currentProj.id);history.replaceState(null,'','#'+params);}
    } else {
      picker.update(projects,null);
    }
    active=new URLSearchParams(location.hash.slice(1)).get('skill')||'';
    activeFile=new URLSearchParams(location.hash.slice(1)).get('file')||'';
    use(await request(),true);if(!data.busy&&!(data.machines||[]).some(m=>m.scanned_at))await action('scan');
  }
  start().catch(fail);
})();
