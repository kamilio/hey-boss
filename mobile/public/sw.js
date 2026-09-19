self.importScripts('/read-receipts.js');
self.addEventListener('install',event=>event.waitUntil(self.skipWaiting()));
self.addEventListener('activate',event=>event.waitUntil(self.clients.claim()));
self.addEventListener('push',event=>event.waitUntil((async()=>{
 let data;try{data=event.data.json();}catch{return;}
 let tasks=[];try{const response=await fetch('/api/tasks',{credentials:'include',cache:'no-store'});if(response.ok)tasks=(await response.json()).tasks;}catch{}
 const notifications=await self.registration.getNotifications();
 for(const n of notifications){const task=tasks.find(t=>t.taskID===n.tag);const resolved=ids=>ids?.length&&ids.every(id=>tasks.some(t=>t.taskID===id&&t.status!=='pending'));if(task&&task.status!=='pending'||resolved(n.data?.taskIDs))n.close();}
 if('setAppBadge'in self.navigator)await self.navigator.setAppBadge(tasks.filter(t=>t.status==='pending').length).catch(()=>{});
 const current=tasks.find(t=>t.taskID===data.id);const handled=current&&current.status!=='pending';
 await self.registration.showNotification(data.title||'Update',{body:handled?'Already handled. Open to see the answer.':data.body,icon:'/icons/icon-192.png',badge:'/icons/badge-96.png',tag:data.id,data:{id:data.id,taskIDs:data.taskIDs},renotify:false});
})()));
self.addEventListener('notificationclick',event=>{event.notification.close();event.waitUntil((async()=>{
 const id=event.notification.data?.id||'';
 const receipt=HeyBossReceipts.open(id).catch(()=>{});
 const url=new URL('/?task='+encodeURIComponent(id)+'&opened=1',self.location.origin).href;
 const showReader=(async()=>{
  for(const client of await self.clients.matchAll({type:'window',includeUncontrolled:true})){if(new URL(client.url).origin===self.location.origin){await client.navigate(url);return client.focus();}}
  return self.clients.openWindow(url);
 })();
 await Promise.all([receipt,showReader]);
})());});
self.addEventListener('message',event=>{if(event.data?.type==='flush-receipts')event.waitUntil(HeyBossReceipts.flush());});
