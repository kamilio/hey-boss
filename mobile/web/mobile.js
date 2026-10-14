"use strict";
(()=>{
// Give the shared issue pages the same glass tab bar and overflow menu as the inbox.
const svg=body=>`<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${body}</svg>`;
const glyphs={
 inbox:svg('<polyline points="22 12 16 12 14 15 10 15 8 12 2 12"/><path d="M5.45 5.11 2 12v6a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2v-6l-3.45-6.89A2 2 0 0 0 16.76 4H7.24a2 2 0 0 0-1.79 1.11z"/>'),
 issues:svg('<rect x="3" y="5" width="6" height="6" rx="1"/><path d="m3 17 2 2 4-4"/><path d="M13 6h8"/><path d="M13 12h8"/><path d="M13 18h8"/>'),
 jobs:svg('<circle cx="12" cy="12" r="10"/><polyline points="12 6 12 12 16 14"/>'),
 plus:svg('<path d="M5 12h14"/><path d="M12 5v14"/>'),
};
const bar=document.createElement('nav');bar.className='hb-bar';bar.setAttribute('aria-label','Main navigation');
bar.innerHTML=`<div class="hb-tabs hb-glass">${[['/','Inbox','inbox'],['/issues','Issues','issues'],['/jobs','Jobs','jobs']].map(([href,label,name])=>
 `<a class="hb-tab" href="${href}"${location.pathname===href?' aria-current="page"':''}>${glyphs[name]}<span>${label}</span>${name==='inbox'?'<b class="hb-badge" hidden></b>':''}</a>`).join('')}</div><button class="hb-fab hb-glass" type="button" aria-label="New issue">${glyphs.plus}</button>`;
document.body.append(bar);
fetch('/api/tasks').then(response=>response.ok?response.json():null).then(value=>{
 const count=value?.tasks.filter(task=>task.status==='pending').length;const badge=bar.querySelector('.hb-badge');
 if(count){badge.textContent=count;badge.hidden=false;bar.querySelector('[href="/"]').setAttribute('aria-label',`Inbox, ${count} unread`);}
}).catch(()=>{});
const compose=()=>{const button=document.getElementById('new-issue');if(button?.onclick&&!button.disabled)button.click();else location.href='/issues?compose=1';};
bar.querySelector('.hb-fab').onclick=compose;
if(new URLSearchParams(location.search).has('compose')){
 history.replaceState(null,'',location.pathname+location.hash);
 const started=Date.now();const wait=()=>{if(typeof model!=="undefined"&&model.project)return compose();if(Date.now()-started<10000)setTimeout(wait,100);};wait();
}

// Settings and project documents live in the overflow menu, never in the header.
const menu=document.getElementById('navigation-more-links');
if(menu){
 const item=(label,name,action)=>{const element=document.createElement(typeof action==='string'?'a':'button');element.innerHTML=`<span data-icon="${name}"></span>${label}`;if(typeof action==='string')element.href=action;else{element.type='button';element.onclick=action;}return element;};
 // Settings dialogs exist only on the issue list; elsewhere, open them there.
 const issues=location.pathname==='/issues',open=id=>()=>document.getElementById(id).click();
 menu.prepend(item('Phone settings','bell','/#settings'),item('Settings','settings',issues?open('global-settings-trigger'):'/issues#settings=1'),...issues?[item('Project settings','instructions',open('project-settings-trigger'))]:[]);
 for(const id of ['nav-artifacts','nav-mindmaps','nav-merged-prs']){const link=document.getElementById(id);if(link)menu.append(link);}
 menu.addEventListener('click',event=>{if(event.target.closest('a,button'))menu.hidePopover?.();});
 if(typeof icons==="function")icons(menu);
}

// Keep copied links and related documents in the shared phone issue interface.
document.addEventListener('click',event=>{
 const link=event.target.closest('a');if(!link||event.metaKey||event.ctrlKey||event.shiftKey||event.altKey)return;
 const url=new URL(link.href,location.href);if(url.origin!==location.origin)return;
 if(link.id==='nav-inbox'){
  event.preventDefault();event.stopImmediatePropagation();location.href='/';
 }else if(url.pathname==='/'&&url.hash&&!url.hash.includes('view=inbox')&&url.hash!=='#settings'){
  event.preventDefault();event.stopImmediatePropagation();location.href='/issues'+url.hash;
 }
},true);
})();
if('serviceWorker'in navigator)navigator.serviceWorker.register('/sw.js').catch(()=>{});
