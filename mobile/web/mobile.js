"use strict";
const resources=document.createElement('div');resources.className='project-resources';resources.setAttribute('aria-label','Project resources');
for(const id of ['nav-artifacts','nav-mindmaps']){
 const link=document.getElementById(id);if(link)resources.append(link);
}
document.querySelector('main')?.prepend(resources);
// Keep copied links and related documents in the shared phone issue interface.
document.addEventListener('click',event=>{
 const link=event.target.closest('a');if(!link||event.metaKey||event.ctrlKey||event.shiftKey||event.altKey)return;
 const url=new URL(link.href,location.href);if(url.origin!==location.origin)return;
 if(link.id==='nav-inbox'){
  event.preventDefault();event.stopImmediatePropagation();location.href='/';
 }else if(url.pathname==='/'&&url.hash&&!url.hash.includes('view=inbox')){
  event.preventDefault();event.stopImmediatePropagation();location.href='/issues'+url.hash;
 }
},true);
if('serviceWorker'in navigator)navigator.serviceWorker.register('/sw.js').catch(()=>{});
