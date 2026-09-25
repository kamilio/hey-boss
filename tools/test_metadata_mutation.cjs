const assert = require('node:assert/strict');
const {readFileSync} = require('node:fs');
const vm = require('node:vm');
const source = readFileSync('src/issues/web/app.js', 'utf8');
const start = source.indexOf('async function mutate(');
const end = source.indexOf('\nfunction routeHash(', start);
assert(start >= 0 && end > start);
(async () => {
  let completed = 0;
  for (const code of ['fleet_reserved', 'fleet_allocation_expired', 'fleet_allocation_missing', 'network_error']) {
    const pending = new Map();
    const ids = [];
    let failure = true, nextId = 0;
    const context = vm.createContext({
      model: {project:{id:'fixture'},route:{}}, pendingMutation:pending,
      mutationKey:async ()=>'operation', HeyBossUI:{requestId:()=>String(++nextId)},
      persistPending:()=>{}, detailCache:new Map(), detailKey:()=>'',
      api:async (_operation,_project,id)=>{ids.push(id);if(failure)throw {code};return {ok:true};},
    });
    vm.runInContext(source.slice(start,end),context);
    await assert.rejects(context.mutate({number:1}), error=>error.code===code);
    assert.equal(pending.size,code==='network_error'?1:0);
    failure=false;
    assert.equal((await context.mutate({number:1})).ok,true);
    assert.equal(ids[0]===ids[1],code==='network_error');
    assert.equal(pending.size,0);
    completed++;
  }
  assert.equal(completed,4);
  console.log('COMPLETE: 4/4 mutation retry cases; explicit denials clear tokens, uncertain network results retain deduplication');
})().catch(error=>{console.error(error);process.exitCode=1;});
