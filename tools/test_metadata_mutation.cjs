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
  const previous = {issue:{number:1,version:1,draft:false},comments:[{body:'Keep this discussion'}]};
  const cache = new Map();
  const context = vm.createContext({
    model:{project:{id:'fixture'},route:{issue:1},detail:previous},
    pendingMutation:new Map(), mutationKey:async ()=>'draft', HeyBossUI:{requestId:()=>'draft-id'},
    persistPending:()=>{}, detailCache:cache, detailKey:()=> 'fixture:1',
    api:async ()=>({ok:true,store:{host:'supervisor'},comments:null,subtasks:null,issue:{number:1,version:2,draft:true}}),
  });
  vm.runInContext(source.slice(start,end),context);
  await context.mutate({number:1,draft:true});
  assert.equal(cache.get('fixture:1').value.issue.version,2);
  assert.equal(cache.get('fixture:1').value.issue.draft,true);
  assert.equal(cache.get('fixture:1').value.comments,previous.comments);
  cache.clear();context.model.route.issue=2;
  await context.mutate({number:1,draft:true});
  assert.equal(cache.size,0,'Do not combine the committed issue with an unrelated detail page');
  console.log('COMPLETE: committed tunnel responses retain the detail cache and discussion without crossing issue scope');
})().catch(error=>{console.error(error);process.exitCode=1;});
