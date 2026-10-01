"use strict";
const HeyBossMergedPRs = (() => {
  const esc = value => String(value ?? '').replace(/[&<>"']/g, c => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));
  const day = date => `${date.getFullYear()}-${date.getMonth()}-${date.getDate()}`;
  function groups(rows, now = new Date()) {
    const yesterday = new Date(now); yesterday.setDate(now.getDate()-1);
    const result = new Map();
    for (const row of [...rows].sort((a,b)=>(b.merged_at||0)-(a.merged_at||0)||a.url.localeCompare(b.url))) {
      const date = row.merged_at ? new Date(row.merged_at) : null;
      const key = date ? day(date) : 'unknown';
      if (!result.has(key)) result.set(key, {key, label: !date ? 'Merge date unavailable' : key===day(now) ? 'Today' : key===day(yesterday) ? 'Yesterday' : date.toLocaleDateString(undefined,{weekday:'long',month:'short',day:'numeric',year:'numeric'}), rows:[]});
      result.get(key).rows.push(row);
    }
    return [...result.values()];
  }
  function render(rows, context) {
    return groups(rows).map(group=>`<section class="merge-day"><h2>${esc(group.label)} <span>${group.rows.length} ${group.rows.length===1?'PR':'PRs'}</span></h2><div class="merge-rows">${group.rows.map(pr=>{
      const match = /^https:\/\/github\.com\/([^/]+)\/([^/]+)\/pull\/(\d+)$/.exec(pr.url);
      const label = match ? `${match[1]}/${match[2]}#${match[3]}` : 'Pull request';
      const time = pr.merged_at || pr.observed_at;
      const title = pr.title || label;
      return `<article class="merge-row"><span class="merge-mark" data-icon="pr-merged" role="img" aria-label="Merged"></span><div class="merge-main">${match?`<a class="merge-title" href="${esc(pr.url)}" target="_blank" rel="noopener noreferrer">${esc(title)}</a>`:`<span class="merge-title">${esc(title)}</span>`}<div class="merge-meta"><span>${esc(label)}</span>${(pr.issues||[]).map(issue=>`<a href="/#${esc(new URLSearchParams({project:context.project,issue:issue.number,...(context.host?{host:context.host}:{})}))}" title="${esc(issue.title)}">#${issue.number} ${esc(issue.title)}</a>`).join('')}</div></div><span class="merge-time">${time?`${pr.merged_at?'':'Observed '}<time datetime="${new Date(time).toISOString()}" title="${esc(new Date(time).toLocaleString())}">${esc(new Date(time).toLocaleString(undefined,pr.merged_at?{hour:'numeric',minute:'2-digit'}:{month:'short',day:'numeric',year:'numeric'}))}</time>`:'Date unavailable'}</span></article>`;
    }).join('')}</div></section>`).join('');
  }
  async function start() {
    const $ = selector => document.querySelector(selector);
    HeyBossUI.icons();
    const mobile = document.documentElement.dataset.issueMobile === 'true';
    let boot, context, generation=0, rows=[], next=null, busy=false, timer;
    const picker = new HeyBossUI.ProjectPicker({onSelect:project=>{location.hash=new URLSearchParams({project,...(context.host?{host:context.host}:{})});}});
    const error = message => {$('#merged-error').textContent=message;$('#merged-error').hidden=!message;};
    async function request(operation, project, host) {
      const response=await fetch('/api/action',{method:'POST',headers:{'Content-Type':'application/json',...(!mobile?{'X-Hey-Boss-CSRF':boot.csrf}:{})},body:JSON.stringify({project,operation,...(host?{host}:{})}),signal:AbortSignal.timeout(30000)});
      const value=await response.json();
      if(!response.ok||!value.ok)throw Error(value.error?.message||value.error||'Could not load merged PRs');
      return value;
    }
    async function load(reset=false) {
      if(busy&&!reset)return;
      clearTimeout(timer);
      const seq=++generation, current={...context}; busy=true;error('');
      $('#merged-more').disabled=true;$('#merged-refresh').disabled=true;
      $('#merged-status').textContent='Loading merged PRs…';
      try {
        const value=await request({action:'merged_pull_requests',limit:100,offset:reset?0:next},current.project,current.host);
        if(seq!==generation)return;
        rows=reset?value.pull_requests:[...new Map([...rows,...value.pull_requests].map(pr=>[pr.url,pr])).values()];next=value.next_offset;
        $('#merged-list').innerHTML=render(rows,current);
        HeyBossUI.icons($('#merged-list'));
        $('#merged-status').textContent=rows.length?`${rows.length}${next!==null?' +':''} merged ${rows.length===1?'PR':'PRs'} · Dates in your local timezone`:value.authorship_pending?'Checking GitHub authors before showing your merged PRs…':'No merged fix PRs authored by your GitHub account yet.';
        $('#merged-more').hidden=next===null;
        if(value.authorship_pending){if(rows.length)$('#merged-status').textContent+=' · Checking remaining PR authors…';timer=setTimeout(()=>load(true),3000);}
      } catch(e) {if(seq===generation){error(e.message);$('#merged-status').textContent='';}}
      finally {if(seq===generation){busy=false;$('#merged-more').disabled=false;$('#merged-refresh').disabled=false;}}
    }
    async function navigate() {
      const params=new URLSearchParams(location.hash.slice(1));
      const project=HeyBossUI.projectId(boot.project?.id||boot.projects[0]?.id);
      context={project,host:mobile?null:params.get('host')||boot.backend_host||null};
      const selected=boot.projects.find(p=>p.id===project)||{id:project,name:project};
      picker.update(boot.projects,selected);
      $('#nav-merged-prs').setAttribute('aria-current','page');
      rows=[];next=null;$('#merged-list').innerHTML='';$('#merged-more').hidden=true;
      if(!project){$('#merged-status').textContent='Choose a project to see merged PRs.';return;}
      await load(true);
    }
    async function bootstrap() {
      try {
        const response=await fetch('/api/bootstrap',{signal:AbortSignal.timeout(30000)});boot=await response.json();
        if(!response.ok||!boot.ok)throw Error(boot.error?.message||boot.error||'Could not connect');
        await navigate();
      }catch(e){error(e.message);$('#merged-status').textContent='';$('#merged-refresh').disabled=false;}
    }
    $('#merged-more').onclick=()=>load();$('#merged-refresh').onclick=()=>boot?.ok?load(true):bootstrap();
    window.addEventListener('hashchange',()=>{if(boot?.ok)navigate();});
    await bootstrap();
  }
  return {groups,render,start};
})();
if(typeof module!=='undefined')module.exports=HeyBossMergedPRs;
if(typeof document!=='undefined'&&document.querySelector('#merged-main'))HeyBossMergedPRs.start();
