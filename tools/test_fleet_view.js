const assert = require('node:assert/strict');
const {fleetView, elapsed} = require('../src/issues/web/fleet.js');
const now = 100000;
const run = {started_at: 1000, finished_at: null};
const running = {id: 'live', pid: 123, active: 1, config: {concurrency: 2}, runs: [run, {...run, finished_at: 61000}]};
const saved = {id: 'saved', pid: null, active: 0, config: {enabled: true, concurrency: 20}};
const draining = {id: 'draining', pid: 124, config: {enabled: false, concurrency: 1}, runs: [run]};
const result = fleetView({machines: [
  {host: 'local', role: 'supervisor', state: 'connected', heartbeat: 99, workers: [running, saved, draining]},
  {host: 'offline', state: 'disconnected', heartbeat: 98, workers: [running]},
  {host: 'stale', state: 'connected', heartbeat: 80, workers: [running]},
  {host: 'idle', state: 'connected', heartbeat: 99, workers: [{...running, id: 'idle', active: 0, runs: []}]},
]}, now);
assert.deepEqual(result.live.map(({worker}) => worker.id), ['live', 'draining', 'idle']);
assert.equal(result.active, 2, 'History and disconnected snapshots must not inflate active agents');
assert.equal(result.capacity, 5, 'Saved capacity is excluded; paused live processes remain visible');
assert.deepEqual(result.saved.map(({worker}) => worker.id), ['saved']);
assert.deepEqual(result.offline.map(machine => machine.host), ['offline', 'stale']);
assert.equal(result.supervisor.host, 'local');
assert.equal(elapsed(run, now), '1m39s');
assert.equal(elapsed({...run, finished_at: 61000}, now), '1m00s', 'Finished time freezes history duration');
assert.equal(elapsed({started_at: 0, finished_at: 3601000}, now), '1h00m01s');
assert.equal(elapsed({started_at: now + 1000}, now), '0m00s');
assert.equal(elapsed({}, now), '0m00s');
assert.deepEqual(fleetView({}, now).live, []);
console.log('Fleet visibility and elapsed time checks passed');
const {projectView} = require('../src/issues/web/fleet.js');
const projectRun = {id:'a',project_id:'named:Atlas',project_name:'Atlas',title:'Fix reconnect',started_at:1,finished_at:null};
const projects = projectView({machines:[
 {host:'local',state:'connected',heartbeat:99,workers:[{pid:1,runs:[projectRun,{...projectRun,id:'done',finished_at:2}]}]},
 {host:'remote',state:'disconnected',heartbeat:80,workers:[{pid:1,runs:[{...projectRun,id:'b',project_id:'named:Beacon',project_name:'Beacon'}]}]},
]}, now);
assert.deepEqual(projects.map(p=>p.name), ['Atlas','Beacon']);
assert.equal(projects[0].active.length,1);
assert.equal(projects[0].history.length,1);
assert.equal(projects[1].active[0].online,false);
assert.deepEqual(projectView({machines:[]},now),[]);
console.log('Project grouping checks passed');
const {agentState, scheduledRetries} = require('../src/issues/web/fleet.js');
const held={run:{...projectRun,number:1,state:'infrastructure_blocked',finished_at:2},online:true};
assert.equal(agentState(held),'Approval service unavailable');
assert.equal(agentState({...held,online:false}),'Approval service unavailable','A finished infrastructure hold remains meaningful offline');
const retrying={...held,run:{...held.run,retry_at:Date.now()+30000}};
assert.equal(scheduledRetries({active:[],history:[held]}).length,0,'An ended attempt is not a pending retry');
assert.equal(scheduledRetries({active:[],history:[retrying]}).length,1,'An outstanding hold is visible outside collapsed history');
assert.equal(scheduledRetries({active:[{run:{number:1,started_at:3}}],history:[retrying]}).length,0,'A resumed session supersedes the old hold');
assert.equal(scheduledRetries({active:[],history:[retrying,{run:{number:1,started_at:3,state:'completed'}}]}).length,0,'A later completion supersedes the old hold');
assert.equal(scheduledRetries({active:[{run:{number:2,started_at:3}}],history:[retrying]}).length,1,'Another issue does not hide the hold');
for (const label of ['Database service unavailable', 'Model proxy unavailable']) {
  assert.equal(agentState({...held,run:{...held.run,summary:label+'. Automatic pickup is held.'}}),label);
}
console.log('Infrastructure status and outstanding hold checks passed');
const {deviceView} = require('../src/issues/web/fleet.js');
const atlasWorker = {id:'atlas',pid:12,config:{projects:['named:Atlas'],directory:'/work/atlas',enabled:true,concurrency:2},runs:[projectRun]};
const allProjects = {id:'all',pid:13,config:{projects:[],directory:'/work',enabled:true,concurrency:3},runs:[]};
const devices = deviceView({machines:[
 {host:'local',state:'connected',heartbeat:99,workers:[atlasWorker,{...atlasWorker,id:'stopped',pid:null},allProjects,{id:'beacon',pid:14,config:{projects:['named:Beacon']},runs:[]}]},
 {host:'remote',state:'connected',heartbeat:80,workers:[atlasWorker]},
]},'named:Atlas',now);
assert.deepEqual(devices[0].live.map(w=>w.id),['atlas','all'],'Selected project includes unrestricted workers but excludes unrelated projects');
assert.deepEqual(devices[0].saved.map(w=>w.id),['stopped'],'Stopped records are separate from running workers');
assert.equal(devices[0].active,1,'Only unfinished attempts contribute to active agents');
assert.equal(devices[0].capacity,5);
assert.equal(devices[1].online,false,'Stale devices have last-known state, not confirmed running capacity');
assert.equal(devices[1].active,0);
assert.equal(devices[1].capacity,0);
assert.deepEqual(deviceView({machines:[{host:'other',workers:[{config:{projects:['named:Beacon']}}]}]},'named:Atlas',now),[]);
assert.equal(deviceView({machines:[{host:'empty',workers:[]}]},null,now).length,1,'All-projects view keeps empty devices visible');
assert.deepEqual(deviceView({},null,now),[]);
console.log('Device controls scope and runtime checks passed');
const {chiefState} = require('../src/issues/web/fleet.js');
const chief = {id:'chief:local:named:Atlas',kind:'chief',project_id:'named:Atlas',project_name:'Atlas',state:'idle',finished_at:90000,next_at:now+15*60000};
const chiefEntry = {run:chief,online:true,worker:{pid:12,config:{enabled:true}}};
assert.equal(chiefState(chiefEntry,now),'Waiting · 15 minutes left');
assert.equal(chiefState({...chiefEntry,run:{...chief,state:'running',finished_at:null}},now),'Running');
assert.equal(chiefState({...chiefEntry,online:false},now),'Device disconnected');
assert.equal(chiefState({...chiefEntry,worker:{pid:12,config:{enabled:false}}},now),'Paused');
assert.equal(chiefState({...chiefEntry,run:{...chief,next_at:now-1}},now),'Waiting · Due now');
const chiefGroups=projectView({machines:[{host:'local',state:'connected',heartbeat:99,workers:[{...atlasWorker,chiefs:[chief]}]}]},now);
assert.equal(chiefGroups[0].chiefs[0].run.id,chief.id);
assert.equal(chiefGroups[0].active.length,1,'Chief does not consume an issue slot');
assert.equal(chiefGroups[0].history.length,0,'Chief has a separate last-pass presentation');

assert.match(agentState(retrying), /^Retry in /);
