// Shared by the page and service worker. Read receipts survive a closed app and
// temporary network loss; questions are never answered by opening them.
globalThis.HeyBossReceipts=(()=>{
 let database;
 const db=()=>database??=new Promise((resolve,reject)=>{
  const request=indexedDB.open('hey-boss-receipts',1);
  request.onupgradeneeded=()=>request.result.createObjectStore('opens',{keyPath:'id'});
  request.onsuccess=()=>resolve(request.result);request.onerror=()=>{database=undefined;reject(request.error);};
 });
 async function operation(mode,action){const connection=await db();return new Promise((resolve,reject)=>{
  const transaction=connection.transaction('opens',mode);const request=action(transaction.objectStore('opens'));
  transaction.oncomplete=()=>resolve(request.result);transaction.onerror=()=>reject(transaction.error);transaction.onabort=()=>reject(transaction.error);
 });}
 async function send(id){
  const controller=new AbortController();const deadline=setTimeout(()=>controller.abort(),2500);
  let response;try{response=await fetch('/api/tasks/'+encodeURIComponent(id)+'/open',{method:'POST',credentials:'include',headers:{'Content-Type':'application/json'},body:'{}',signal:controller.signal});}finally{clearTimeout(deadline);}
  if(response.ok||response.status===404){await operation('readwrite',store=>store.delete(id)).catch(()=>{});return response.ok;}
  throw Error('Read confirmation is waiting for a connection');
 }
 async function open(id){if(!id||id==='inbox')return;await operation('readwrite',store=>store.put({id})).catch(()=>{});return send(id);}
 let flushing;
 function flush(){return flushing??= (async()=>{
  for(const {id} of await operation('readonly',store=>store.getAll()))try{await send(id);}catch{break;}
 })().finally(()=>{flushing=undefined;});}
 return {open,flush};
})();
