const {test} = require('node:test');
const assert = require('node:assert/strict');
const {runtime} = require('../src/issues/web/assignments.js');
const now = 2_000_000;
const entry = {online:true,machine:{state:'connected',heartbeat:now/1000},run:{state:'running',started_at:now-125000,finished_at:null}};
test('live runtime advances across minute, hour and day boundaries', () => {
  assert.equal(runtime(entry,now).duration,'2m 05s');
  assert.equal(runtime(entry,now+1000).duration,'2m 06s');
  assert.equal(runtime({...entry,run:{...entry.run,started_at:now-3661000}},now).duration,'1h 01m');
  assert.equal(runtime({...entry,run:{...entry.run,started_at:now-90000000}},now).duration,'1d 01h');
  assert.equal(runtime(entry,now).live,true);
});
test('finished and offline attempts never keep counting', () => {
  const finished={...entry,run:{...entry.run,finished_at:now-5000,state:'completed'}};
  assert.equal(runtime(finished,now+100000).duration,'2m 00s');
  assert.equal(runtime(finished,now).label,'Completed');
  const offline={...entry,online:false};
  assert.equal(runtime(offline,now+100000).duration,'2m 05s');
  assert.equal(runtime(offline,now).label,'Last seen');
  assert.equal(runtime(entry,now+16000).live,false);
});
test('unknown or invalid timestamps are explicit, never fabricated', () => {
  for(const started_at of [null,undefined,NaN,Infinity,'123',now+1]) {
    assert.equal(runtime({...entry,run:{...entry.run,started_at}},now).duration,'Unavailable');
  }
  assert.equal(runtime({...entry,machine:{state:'disconnected'}},now).duration,'Unavailable');
  assert.equal(runtime({...entry,run:{...entry.run,state:'approval_required'}},now).label,'Needs approval');
});
