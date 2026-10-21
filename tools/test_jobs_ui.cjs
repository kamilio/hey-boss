const assert = require('node:assert/strict');
const {readFileSync} = require('node:fs');
const {webcrypto} = require('node:crypto');
const vm = require('node:vm');
const {summary, modelChoices, availability, Mutation, scheduleState, filterJobs, scheduleFields, scheduleCron} = require('../src/issues/web/jobs.js');
assert.deepEqual(scheduleFields('25 */3 * * *'),{frequency:'hours',time:'09:00',interval:3,minute:25});
assert.deepEqual(scheduleFields('00 */12 * * *'),{frequency:'hours',time:'09:00',interval:12,minute:0});
assert.equal(scheduleCron('hours','09:00',{interval:3,minute:25}),'25 */3 * * *');
assert.equal(summary('25 */3 * * *'),'Every 3 hours at :25');
assert.equal(summary('0 */5 * * *'),'Every 5 hours at :00, restarting each day');
for(const interval of [2,3,4,5,6,12,23])for(const minute of [0,25,59]){
 const cron=scheduleCron('hours','09:00',{interval,minute});
 assert.equal(scheduleCron('hours','09:00',scheduleFields(cron)),cron,'Hourly steps round trip');
}
for(const interval of [0,1,24,2.5,NaN])assert.equal(scheduleCron('hours','09:00',{interval,minute:25}),'');
for(const minute of [-1,60,1.5,NaN])assert.equal(scheduleCron('hours','09:00',{interval:3,minute}),'');
for(const cron of ['25 */0 * * *','25 */24 * * *','60 */3 * * *','25 1-23/3 * * *','25 */3 * * MON'])assert.equal(scheduleFields(cron).frequency,'custom');
assert.deepEqual(scheduleFields('30 17 * * 1-5'),{frequency:'weekdays',time:'17:30'});
assert.deepEqual(scheduleFields('005 009 * * *'),{frequency:'daily',time:'09:05'},'Valid zero-padded cron remains editable');
assert.equal(summary('005 009 * * *'),'Every day at 09:05');
assert.deepEqual(scheduleFields('0 * * * *'),{frequency:'hourly',time:'09:00',minute:0});
assert.deepEqual(scheduleFields('*/15 * * * *'),{frequency:'minutes',time:'09:00',interval:15});
assert.deepEqual(scheduleFields('45 * * * *'),{frequency:'hourly',time:'09:00',minute:45});
assert.deepEqual(scheduleFields('5 9 * * 1,3,5'),{frequency:'weekly',time:'09:05',days:[1,3,5]});
assert.deepEqual(scheduleFields('5 9 31 * *'),{frequency:'monthly',time:'09:05',day:31});
assert.equal(scheduleFields('5 9 1 * 1').frequency,'custom','Never rewrite cron OR day semantics as a preset');
assert.equal(scheduleFields('*/61 * * * *').frequency,'custom');
assert.equal(scheduleCron('minutes','09:00',{interval:15}),'*/15 * * * *');
assert.equal(scheduleCron('hourly','09:00',{minute:45}),'45 * * * *');
assert.equal(scheduleCron('weekly','09:05',{days:[5,1,3]}),'5 9 * * 1,3,5');
assert.equal(scheduleCron('weekly','09:05',{days:[]}),'','An empty day selection must not save an old schedule');
assert.equal(scheduleCron('monthly','09:05',{day:31}),'5 9 31 * *');
assert.equal(scheduleCron('daily',''),'','An empty time must not save an old schedule');
assert.equal(summary('*/15 * * * *'),'Every 15 minutes');
assert.equal(summary('*/7 * * * *'),'Every 7 minutes, restarting each hour');
assert.deepEqual(scheduleFields('0 9 * * 7,0'),{frequency:'weekly',time:'09:00',days:[0]});
for(const cron of ['0 24 * * *','60 9 * * *','0 9 0 * *','0 9 32 * *','0 9 * * 8','0 9 * * * extra'])assert.equal(scheduleFields(cron).frequency,'custom');
assert.equal(summary('45 * * * *'),'Every hour at :45');
assert.equal(summary('5 9 * * 1,3,5'),'Mon, Wed, Fri at 09:05');
assert.equal(summary('5 9 31 * *'),'Day 31 of each month at 09:05');
assert.equal(scheduleFields('*/7 2-8 * JAN MON').frequency,'custom','Complex schedules must remain exact');
assert.equal(scheduleCron('weekdays','17:30'),'30 17 * * 1-5');
assert.equal(scheduleCron('weekly','09:05'),'5 9 * * 1');
assert.equal(summary('5 9 * * 1'),'Mondays at 09:05');
const jobs=[
 {id:'on',enabled:true,deleted_at:null,snapshot:{definition:{name:'Chief',model:'gpt',cron:'0 * * * *'}},last_run:{state:'failed'}},
 {id:'off',enabled:false,deleted_at:null,snapshot:{definition:{name:'Weekly review',model:'claude',cron:'0 9 * * 1'}},active_run:{state:'running'}},
 {id:'gone',enabled:false,deleted_at:123,snapshot:{definition:{name:'Old',model:'gpt',cron:'0 * * * *'}}}
];
assert.equal(scheduleState(jobs[0]),'Enabled','Failed execution must not disable a schedule');
assert.equal(scheduleState(jobs[1]),'Disabled','An active execution must not make a disabled schedule appear enabled');
assert.equal(scheduleState(jobs[2]),'Deleted');
assert.deepEqual(filterJobs(jobs,'disabled','').map(j=>j.id),['off'],'Deleted is distinct from disabled');
assert.deepEqual(filterJobs(jobs,'enabled',' CHIEF ').map(j=>j.id),['on']);
assert.deepEqual(filterJobs(jobs,'all','claude').map(j=>j.id),['off']);
assert.equal(summary('0 9 * * 1-5'), 'Weekdays at 09:00');
assert.equal(summary('30 8 * * *'), 'Every day at 08:30');
assert.match(summary('*/7 2-8 * JAN MON'), /Minute/);
assert.deepEqual(modelChoices({models:[{selection:{id:'exact',route:'openai'},label:'Exact',advertised:true}]},'custom'),[
 {value:'custom',label:'custom · Not advertised',advertised:false}, {value:'openai/exact',label:'Exact',advertised:true}
]);
const machine={state:'connected',heartbeat:100,jobs:{available:true,workers:[{id:'pool',projects:['p']}],runtimes:{codex:{available:true}}},workers:[{intent:'pause',free:0}]};
assert.equal(availability([machine],'p','codex',100000),true);
assert.equal(availability([{...machine,jobs:{...machine.jobs,workers:[]}}],'p','codex',100000),false);
assert.equal(availability([{...machine,jobs:{...machine.jobs,workers:[{id:'other',projects:['q']}]}}],'p','codex',100000),false);
assert.equal(availability([machine],'p','claude',100000),false);
assert.equal(availability([machine],'p','codex',200000),false);
(async()=>{
 for(const crypto of [webcrypto,{getRandomValues:webcrypto.getRandomValues.bind(webcrypto)}]){
  const context=vm.createContext({crypto,document:{addEventListener(){}}});
  vm.runInContext(readFileSync('src/issues/web/components.js','utf8'),context);
  delete context.document;
  vm.runInContext(readFileSync('src/issues/web/jobs.js','utf8')+'\nthis.Mutation=HeyBossJobs.Mutation;',context);
  const sent=[],saved=[];
  let loseResponse=true;
  const mutation=new context.Mutation(async body=>{sent.push(body);if(loseResponse){loseResponse=false;throw Error('Lost response');}return {ok:true};},undefined,value=>saved.push(value&&JSON.stringify(value)));
  await assert.rejects(mutation.send({command:'set_enabled',id:'daily',if_revision:1,enabled:false},'p','local'),/Lost response/);
  assert.match(mutation.pending.request_id,/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/);
  const restored=new context.Mutation(mutation.post);
  restored.pending=JSON.parse(saved[0]);
  await restored.retry();
  assert.equal(JSON.stringify(sent[0]),JSON.stringify(sent[1]),'Restored disable retries the original payload and ID on HTTP and secure origins');
  await restored.send({command:'set_enabled',id:'daily',if_revision:2,enabled:true},'p','local');
  assert.notEqual(sent[2].request_id,sent[0].request_id,'A new action gets a fresh request ID');
  assert.equal(restored.pending,null);
 }
 let calls=[],fail=true;
 const mutation=new Mutation(async body=>{calls.push(body);if(fail){fail=false;throw Error('Lost response');}return {ok:true};},()=> 'request-one');
 await assert.rejects(mutation.send({command:'run_now',id:'daily'},'p'));
 await mutation.retry();
 assert.deepEqual(calls[0],calls[1]);
 assert.equal(calls[0].request_id,'request-one');
 assert.equal(mutation.pending,null);
 console.log('Jobs UI behavior checks passed');
})();
