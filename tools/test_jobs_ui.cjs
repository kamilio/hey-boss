const assert = require('node:assert/strict');
const {summary, modelChoices, availability, Mutation} = require('../src/issues/web/jobs.js');
assert.equal(summary('0 9 * * 1-5'), 'Weekdays at 09:00');
assert.equal(summary('30 8 * * *'), 'Every day at 08:30');
assert.match(summary('*/7 2-8 * JAN MON'), /Minute/);
assert.deepEqual(modelChoices({models:[{selection:{id:'exact',route:'openai'},label:'Exact',advertised:true}]},'custom'),[
 {value:'custom',label:'custom · Not advertised',advertised:false}, {value:'openai/exact',label:'Exact',advertised:true}
]);
const machine={state:'connected',heartbeat:100,jobs:{available:true,checkouts:{p:'/p'},runtimes:{codex:{available:true}}},workers:[{intent:'pause',free:0}]};
assert.equal(availability([machine],'p','codex',100000),true);
assert.equal(availability([{...machine,workers:[]}],'p','codex',100000),true);
assert.equal(availability([machine],'p','claude',100000),false);
assert.equal(availability([machine],'p','codex',200000),false);
(async()=>{
 let calls=[],fail=true;
 const mutation=new Mutation(async body=>{calls.push(body);if(fail){fail=false;throw Error('Lost response');}return {ok:true};},()=> 'request-one');
 await assert.rejects(mutation.send({command:'run_now',id:'daily'},'p'));
 await mutation.retry();
 assert.deepEqual(calls[0],calls[1]);
 assert.equal(calls[0].request_id,'request-one');
 assert.equal(mutation.pending,null);
 console.log('Jobs UI behavior checks passed');
})();
