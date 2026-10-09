"use strict";
const HeyBossCalendar=(()=>{
  const esc=value=>String(value??'').replace(/[&<>"']/g,c=>({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));
  const civil=key=>new Date(key+'T12:00:00Z');
  const key=date=>date.toISOString().slice(0,10);
  function dayKey(ms,zone){const parts=new Intl.DateTimeFormat('en-CA',{timeZone:zone,year:'numeric',month:'2-digit',day:'2-digit'}).formatToParts(ms);return ['year','month','day'].map(type=>parts.find(p=>p.type===type).value).join('-');}
  function range(anchor,mode){
    const start=civil(anchor);
    if(mode==='month')start.setUTCDate(1);
    start.setUTCDate(start.getUTCDate()-(start.getUTCDay()+6)%7);
    if(mode!=='month')return {start:key(start),days:7};
    const end=civil(anchor);end.setUTCMonth(end.getUTCMonth()+1,0);
    return {start:key(start),days:Math.ceil(((end-start)/86400000+1)/7)*7};
  }
  function shift(anchor,mode,delta){const date=civil(anchor);if(mode==='month')date.setUTCMonth(date.getUTCMonth()+delta,1);else date.setUTCDate(date.getUTCDate()+delta*7);return key(date);}
  function mount({root,read,edit,links}){
    const $=s=>root.querySelector(s);
    let zone=Intl.DateTimeFormat().resolvedOptions().timeZone||'UTC',anchor=dayKey(Date.now(),zone),mode=matchMedia('(max-width:640px)').matches?'agenda':'month',job=null,seq=0,detailSeq=0,active=false,currentDays=[],dayEntries=[],cursor=null,openedDay=null,filterAfter=null,filterSeq=0,filtersBusy=false;
    const labels={scheduled:'Scheduled',pending:'Pending',running:'Running',succeeded:'Succeeded',failed:'Failed',cancelled:'Cancelled',skipped:'Skipped'};
    const format=(ms,timezone=zone)=>ms==null?'Not yet':new Intl.DateTimeFormat(undefined,{timeZone:timezone,month:'short',day:'numeric',year:'numeric',hour:'numeric',minute:'2-digit',timeZoneName:'short'}).format(ms);
    const clock=ms=>new Intl.DateTimeFormat(undefined,{timeZone:zone,hour:'numeric',minute:'2-digit'}).format(ms);
    const civilLabel=(value,options)=>civil(value).toLocaleDateString(undefined,{timeZone:'UTC',...options});
    root.innerHTML=`<div class="calendar-toolbar"><div class="calendar-heading"><div class="calendar-arrows"><button class="icon-button" id="calendar-prev" aria-label="Previous period">‹</button><button class="icon-button" id="calendar-next" aria-label="Next period">›</button></div><h2 id="calendar-title"></h2><button class="button" id="calendar-today">Today</button></div><div class="calendar-modes" role="group" aria-label="Calendar presentation">${['month','week','agenda'].map(m=>`<button class="button" data-mode="${m}" aria-pressed="false">${m[0].toUpperCase()+m.slice(1)}</button>`).join('')}</div></div>
      <div class="calendar-filters"><label>Job<select id="calendar-job"><option value="">All jobs · including past runs</option></select></label><button class="button" id="calendar-jobs-more" hidden>More job filters</button><label class="calendar-zone">Display timezone<select id="calendar-zone">${[...new Set([zone,'UTC',...(Intl.supportedValuesOf?.('timeZone')||[])])].map(z=>`<option ${z===zone?'selected':''}>${esc(z)}</option>`).join('')}</select></label><button class="icon-button" id="calendar-refresh" aria-label="Refresh calendar"><span data-icon="refresh"></span></button></div>
      <div class="calendar-legend" aria-label="Entry states">${Object.entries(labels).map(([state,label])=>`<span class="calendar-state ${state}">${label}</span>`).join('')}</div><p id="calendar-status" role="status" aria-live="polite"></p><div id="calendar-grid"></div>
      <dialog id="calendar-dialog" aria-labelledby="calendar-dialog-title"><div class="calendar-dialog-heading"><h2 id="calendar-dialog-title"></h2><button class="icon-button" id="calendar-close" aria-label="Close event details">×</button></div><div id="calendar-detail"></div><button class="button" id="calendar-more" hidden>Show next 100 entries</button></dialog>`;
    const dialog=$('#calendar-dialog');
    function query(command,start,days,position=null){return {command,start,days,timezone:zone,job_id:job,cursor:position};}
    function eventMarkup(entry,index,prefix){return `<button class="calendar-event ${esc(entry.state)}" data-entry="${prefix}:${index}" aria-label="${esc(labels[entry.state]+' · '+entry.snapshot.definition.name+' · '+format(entry.scheduled_at))}" title="${esc(labels[entry.state]+' · '+entry.snapshot.definition.name+' · '+format(entry.scheduled_at))}"><time>${esc(clock(entry.scheduled_at))}</time><span>${esc(entry.snapshot.definition.name)}</span><small>${esc(labels[entry.state])}</small></button>`;}
    function render(){
      const selectedMonth=anchor.slice(0,7),today=dayKey(Date.now(),zone),count=currentDays.reduce((sum,d)=>sum+d.count,0);
      $('#calendar-status').textContent=count?`${count.toLocaleString()} ${count===1?'occurrence':'occurrences'} · Scheduled times in ${zone}`:`No occurrences in this period · ${zone}`;
      $('#calendar-grid').className='calendar-grid '+mode;
      $('#calendar-grid').innerHTML=(mode==='month'?'<div class="calendar-weekdays" aria-hidden="true">'+['Mon','Tue','Wed','Thu','Fri','Sat','Sun'].map(d=>`<span>${d}</span>`).join('')+'</div>':'')+currentDays.map((day,i)=>`<section class="calendar-day ${day.date.slice(0,7)!==selectedMonth&&mode==='month'?'outside':''} ${day.date===today?'today':''}" aria-label="${esc(civilLabel(day.date,{weekday:'long',month:'long',day:'numeric',year:'numeric'}))}"><h3><button data-day="${i}" ${day.date===today?'aria-current="date"':''}><span class="calendar-day-weekday">${esc(civilLabel(day.date,{weekday:'short'}))}</span><span class="calendar-day-number">${civil(day.date).getUTCDate()}</span><span class="calendar-day-month">${esc(civilLabel(day.date,{month:'short'}))}</span></button><small>${day.count?day.count.toLocaleString():''}</small></h3><div class="calendar-day-events">${day.entries.map((e,n)=>eventMarkup(e,n,String(i))).join('')}${day.count>day.entries.length?`<button class="calendar-overflow" data-day="${i}">+${(day.count-day.entries.length).toLocaleString()} more <span class="calendar-overflow-label">· View day</span></button>`:day.count===0?'<p class="calendar-day-empty">No occurrences</p>':''}</div></section>`).join('');
    }
    async function refresh(quiet=false){
      if(!active)return;
      const generation=++seq,{start,days}=range(anchor,mode);
      detailSeq++;if(dialog.open)dialog.close();
      for(const button of root.querySelectorAll('[data-mode]'))button.setAttribute('aria-pressed',String(button.dataset.mode===mode));
      const end=civil(start);end.setUTCDate(end.getUTCDate()+days-1);
      $('#calendar-title').textContent=mode==='month'?civilLabel(anchor,{month:'long',year:'numeric'}):`${civilLabel(start,{month:'short',day:'numeric'})} – ${civilLabel(key(end),{month:'short',day:'numeric',year:'numeric'})}`;
      if(!quiet){$('#calendar-status').textContent='Loading calendar…';$('#calendar-grid').replaceChildren();$('#calendar-grid').setAttribute('aria-busy','true');}
      try {const result=await read(query('calendar',start,days));if(generation!==seq||!active)return;if(quiet&&JSON.stringify(currentDays)===JSON.stringify(result.days))return;
        const focused=$('#calendar-grid').contains(document.activeElement)?document.activeElement:null;
        const focusKey=focused?.dataset.day!=null?['day',focused.dataset.day]:focused?.dataset.entry?['entry',focused.dataset.entry]:null;
        currentDays=result.days;render();if(focusKey)$('#calendar-grid').querySelector('[data-'+focusKey[0]+'="'+CSS.escape(focusKey[1])+'"]')?.focus();}
      catch(e){if(generation===seq){$('#calendar-status').textContent='Could not load calendar. '+e.message;$('#calendar-grid').innerHTML='<div class="calendar-empty"><h3>Your calendar is temporarily unavailable</h3><p>Try again when the connection is ready.</p><button class="button" data-calendar-retry>Retry calendar</button></div>';}}
      finally{if(generation===seq)$('#calendar-grid').setAttribute('aria-busy','false');}
    }
    async function filters(reset){
      const generation=++filterSeq;filtersBusy=true;$('#calendar-jobs-more').disabled=true;
      try{
        const result=await read({command:'list',include_deleted:true,after:reset?null:filterAfter,limit:50});if(generation!==filterSeq)return;
        if(reset)$('#calendar-job').innerHTML='<option value="">All jobs · including past runs</option>';
        for(const row of result.jobs){const option=document.createElement('option');option.value=row.id;option.textContent=row.snapshot.definition.name+(row.deleted_at?' · Deleted':row.enabled?'':' · Paused');$('#calendar-job').append(option);}
        if(job&&!Array.from($('#calendar-job').options).some(o=>o.value===job)){const option=new Option('Selected job',job);$('#calendar-job').append(option);}
        $('#calendar-job').value=job||'';filterAfter=result.next_cursor;$('#calendar-jobs-more').textContent='More job filters';$('#calendar-jobs-more').hidden=!filterAfter;
      }catch(e){if(generation===filterSeq){$('#calendar-jobs-more').hidden=false;$('#calendar-jobs-more').textContent='Retry job filters';}}
      finally{if(generation===filterSeq){filtersBusy=false;$('#calendar-jobs-more').disabled=false;}}
    }
    function showDialog(title){$('#calendar-dialog-title').textContent=title;$('#calendar-more').hidden=true;if(!dialog.open)dialog.showModal();}
    async function openDay(day,more=false){
      const generation=++detailSeq;openedDay=day;showDialog(civilLabel(day.date,{weekday:'long',month:'long',day:'numeric',year:'numeric'}));
      if(!more){dayEntries=[];cursor=null;$('#calendar-detail').textContent='Loading occurrences…';}
      $('#calendar-more').disabled=true;
      try {
        const result=await read(query('calendar_entries',day.date,1,more?cursor:null));if(generation!==detailSeq||!dialog.open)return;
        dayEntries.push(...result.entries);cursor=result.next_cursor;
        $('#calendar-detail').innerHTML=`<p class="job-help">Scheduled times · ${esc(zone)}</p><div class="calendar-day-list">${dayEntries.length?dayEntries.map((e,n)=>eventMarkup(e,n,'day')).join(''):'<p class="job-help">No occurrences on this day.</p>'}</div>`;
        $('#calendar-more').textContent='Show next 100 entries';$('#calendar-more').hidden=!cursor;
      }catch(e){if(generation===detailSeq){$('#calendar-detail').textContent='Could not load this day. '+e.message;$('#calendar-more').textContent='Retry day';$('#calendar-more').hidden=false;}}
      finally{if(generation===detailSeq)$('#calendar-more').disabled=false;}
    }
    async function inspect(entry){
      if(!entry.run){dialog.close();edit(entry.snapshot.job_id);return;}
      const generation=++detailSeq;showDialog(entry.snapshot.definition.name);$('#calendar-detail').textContent='Loading execution…';
      try{
        const {run}=await read({command:'run',id:entry.snapshot.job_id,run_id:entry.run.id});if(generation!==detailSeq||!dialog.open)return;
        const d=run.snapshot.definition;
        $('#calendar-detail').innerHTML=`<span class="job-badge ${esc(run.state)}">${esc(labels[run.state])}</span><p class="calendar-reason">${esc(run.reason||({pending:'Waiting for a job service to start this execution.',running:'This execution is in progress.',skipped:'This occurrence was skipped.'}[run.state])||'')}</p><dl class="calendar-facts"><dt>Schedule timezone</dt><dd>${esc(d.timezone)}</dd><dt>Scheduled time</dt><dd>${esc(format(run.scheduled_at,d.timezone))}</dd><dt>In display timezone</dt><dd>${esc(format(run.scheduled_at))}</dd><dt>Queued</dt><dd>${esc(format(run.created_at))}</dd><dt>Actual start</dt><dd>${run.state==='skipped'?'Not run':run.started_at==null&&!['pending','running'].includes(run.state)?'Not started':esc(format(run.started_at))}</dd><dt>Actual finish</dt><dd>${run.state==='skipped'?'Not run':esc(format(run.finished_at))}</dd><dt>Trigger</dt><dd>${run.trigger==='manual'?'Run now':'Scheduled'} · Revision ${run.snapshot.revision}</dd><dt>Schedule</dt><dd><code>${esc(d.cron)}</code></dd><dt>Execution</dt><dd>${esc(d.harness)} / ${esc(d.model)}</dd></dl><div class="job-run-links">${links(run)}</div><button class="button calendar-open-job" data-edit-job="${esc(run.snapshot.job_id)}">Open job & history</button>`;
      }catch(e){if(generation===detailSeq)$('#calendar-detail').textContent='Could not load execution. '+e.message;}
    }
    root.onclick=event=>{
      const button=event.target.closest('button');if(!button)return;
      if(button.dataset.mode){mode=button.dataset.mode;refresh();}
      if(button.dataset.day!=null)openDay(currentDays[Number(button.dataset.day)]);
      if(button.dataset.entry){const [group,index]=button.dataset.entry.split(':');inspect(group==='day'?dayEntries[Number(index)]:currentDays[Number(group)].entries[Number(index)]);}
      if(button.hasAttribute('data-calendar-retry'))refresh();
      if(button.dataset.editJob){dialog.close();edit(button.dataset.editJob);}
    };
    $('#calendar-grid').onkeydown=event=>{
      const day=event.target.closest('h3 [data-day]');if(!day)return;
      const offset={ArrowLeft:-1,ArrowRight:1,ArrowUp:mode==='month'?-7:-1,ArrowDown:mode==='month'?7:1}[event.key];
      if(offset!=null){event.preventDefault();$('#calendar-grid').querySelector(`h3 [data-day="${Math.max(0,Math.min(currentDays.length-1,Number(day.dataset.day)+offset))}"]`)?.focus();}
    };
    $('#calendar-prev').onclick=()=>{anchor=shift(anchor,mode,-1);refresh();};$('#calendar-next').onclick=()=>{anchor=shift(anchor,mode,1);refresh();};$('#calendar-today').onclick=()=>{anchor=dayKey(Date.now(),zone);refresh();};
    $('#calendar-zone').onchange=()=>{zone=$('#calendar-zone').value;refresh();};$('#calendar-job').onchange=()=>{job=$('#calendar-job').value||null;refresh();};$('#calendar-refresh').onclick=()=>{refresh();filters(true);};
    $('#calendar-jobs-more').onclick=()=>{if(!filtersBusy)filters(!filterAfter);};$('#calendar-close').onclick=()=>dialog.close();dialog.addEventListener('close',()=>detailSeq++);
    $('#calendar-more').onclick=()=>openDay(openedDay,true);
    HeyBossUI.icons(root);
    return {
      show(reset=false){active=true;if(reset){job=null;filterAfter=null;filterSeq++;}filters(true);refresh();},
      hide(){active=false;seq++;filterSeq++;detailSeq++;if(dialog.open)dialog.close();},
      refresh(){if(active&&!dialog.open)refresh(true);}
    };
  }
  return {range,shift,dayKey,mount};
})();
if(typeof module!=='undefined')module.exports=HeyBossCalendar;
