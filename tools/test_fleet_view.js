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
assert.equal(elapsed(run, now), '1m 39s');
assert.equal(elapsed({...run, finished_at: 61000}, now), '1m 00s', 'Finished time freezes history duration');
for(const [seconds,expected] of [[0,'0s'],[59,'59s'],[60,'1m 00s'],[3599,'59m 59s'],[3600,'1h 00m'],[3661,'1h 01m'],[86399,'23h 59m'],[86400,'1d 00h'],[126601,'1d 11h']]){
  assert.equal(elapsed({started_at:0},seconds*1000),expected,'Runtime matches the TUI at '+seconds+' seconds');
}
assert.equal(elapsed({started_at: now + 1000}, now), '0s');
assert.equal(elapsed({}, now), '', 'Unknown start time must not look newly started');
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
assert.equal(agentState({run:{state:'attempt_held',finished_at:null},online:true}),'Task attempt protected');
assert.equal(agentState({run:{state:'attempt_held',finished_at:null,retry_at:Date.now()+30000},online:false}),'Task attempt protected');
const held={run:{...projectRun,number:1,state:'infrastructure_blocked',finished_at:2},online:true};
assert.equal(agentState(held),'Infrastructure unavailable');
assert.equal(agentState({...held,online:false}),'Infrastructure unavailable','A finished infrastructure hold remains meaningful offline');
const retrying={...held,run:{...held.run,retry_at:Date.now()+30000}};
assert.equal(scheduledRetries({active:[],history:[held]}).length,0,'An ended attempt is not a pending retry');
assert.equal(scheduledRetries({active:[],history:[retrying]}).length,1,'An outstanding hold is visible outside collapsed history');
assert.equal(scheduledRetries({active:[{run:{number:1,started_at:3}}],history:[retrying]}).length,0,'A resumed session supersedes the old hold');
assert.equal(scheduledRetries({active:[],history:[retrying,{run:{number:1,started_at:3,state:'completed'}}]}).length,0,'A later completion supersedes the old hold');
assert.equal(scheduledRetries({active:[{run:{number:2,started_at:3}}],history:[retrying]}).length,1,'Another issue does not hide the hold');
for (const label of ['GitHub quota exhausted', 'GitHub authentication failed', 'GitHub permission denied', 'GitHub request failed', 'Worker environment check failed', 'Database service unavailable', 'Model proxy unavailable', 'Approval service unavailable']) {
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
assert.equal(chiefState({...chiefEntry,run:{...chief,state:'running',queued:true}},now),'Running · 1 queued');
assert.equal(chiefState({...chiefEntry,run:{...chief,state:'running',next_at:now-1}},now),'Running · 1 queued');
assert.equal(chiefState({...chiefEntry,online:false},now),'Device disconnected');
assert.equal(chiefState({...chiefEntry,worker:{pid:12,config:{enabled:false}}},now),'Paused');
assert.equal(chiefState({...chiefEntry,run:{...chief,next_at:now-1}},now),'Waiting · Due now');
const chiefGroups=projectView({machines:[{host:'local',state:'connected',heartbeat:99,workers:[{...atlasWorker,chiefs:[chief]}]}]},now);
assert.equal(chiefGroups[0].chiefs[0].run.id,chief.id);
assert.equal(chiefGroups[0].active.length,1,'Chief does not consume an issue slot');
assert.equal(chiefGroups[0].history.length,0,'Chief has a separate last-pass presentation');

assert.match(agentState(retrying), /^Retry in /);

const {workerPhase} = require('../src/issues/web/fleet.js');
const machine={state:'connected',heartbeat:now/1000};
const working={id:'a',intent:'running',pid:1,active:1,config:{enabled:true}};
assert.equal(workerPhase(working,machine,now).group,'working');
assert.equal(workerPhase({...working,intent:'pause'},machine,now).label,'Finishing work');
assert.equal(workerPhase({...working,pid:null,active:0,intent:'stop'},machine,now).group,'stopped');
assert.equal(workerPhase(working,{...machine,state:'disconnected'},now).label,'Offline');
assert.equal(workerPhase(working,{...machine,configuration_error:'"a": Missing checkout; "b": Missing checkout'},now).group,'attention');
assert.equal(workerPhase({...working,id:'c'}, {...machine,configuration_error:'"a": Missing checkout'},now).group,'working');
console.log('Worker status classification checks passed');
const {slotUsage}=require('../src/issues/web/fleet.js');
assert.deepEqual(slotUsage({...working,config:{enabled:true,concurrency:3}},machine,now),{capacity:3,occupied:1,available:2,paused:0,online:true,running:true});
assert.equal(slotUsage({...working,intent:'pause',config:{enabled:false,concurrency:3}},machine,now).paused,2);
assert.equal(slotUsage({...working,active:4,config:{enabled:true,concurrency:2}},machine,now).available,0);
assert.equal(slotUsage(working,{...machine,state:'disconnected'},now).occupied,0);
assert.equal(slotUsage({...working,pid:null},machine,now).capacity,0);
console.log('Live slot occupancy and paused/offline capacity checks passed');

const {workerScopeGroups}=require('../src/issues/web/fleet.js');
const scoped=[
 {worker:{id:'one',config:{projects:['ashby-mcp','hey-gh','hey-proxy']}}},
 {worker:{id:'two',config:{projects:['hey-proxy','ashby-mcp','hey-gh']}}},
 {worker:{id:'three',config:{projects:['hey-gh']}}},
 {worker:{id:'four',config:{projects:[]}}},
];
const scopes=workerScopeGroups(scoped);
assert.equal(scopes.length,3,'One group per complete project set, never one copy per project');
assert.deepEqual(scopes[0].projects,['ashby-mcp','hey-gh','hey-proxy']);
assert.deepEqual(scopes[0].entries.map(e=>e.worker.id),['one','two']);
assert.equal(scopes.flatMap(g=>g.entries).length,scoped.length,'Shared workers occur exactly once');
assert.equal(workerScopeGroups([scoped[1]])[0].key,scopes[0].key,'Project order does not change group identity');
const duplicateNames=workerScopeGroups([{worker:{id:'first-id',config:{name:'Worker',projects:['one']}}},{worker:{id:'second-id',config:{name:'Worker',projects:['one']}}}])[0];
assert.notEqual(duplicateNames.labels.get('first-id'),duplicateNames.labels.get('second-id'),'Identical saved names remain distinguishable within a group');
console.log('Worker project-set grouping checks passed');

const {workerCapacityUpdate} = require('../src/issues/web/fleet.js');
const capacityDocument={machines:{local:{workers:[
  {id:'poe-code',intent:'running',config:{concurrency:5,projects:['named:Code'],provider:'claude'}},
  {id:'other',intent:'pause',config:{concurrency:37,projects:['named:Other']}},
  {id:'retired',retiring:true,intent:'drain',config:{concurrency:2}},
  {id:'small',intent:'pause',config:{concurrency:1}},
  {id:'large',intent:'running',config:{concurrency:1024}}
]}}};
assert.deepEqual(workerCapacityUpdate(capacityDocument,'local','poe-code',1),{host:'local',id:'poe-code',intent:'running',config:{concurrency:6}},'Plus changes agent concurrency of the selected worker, never the machine worker count');
assert.deepEqual(workerCapacityUpdate(capacityDocument,'local','other',-1),{host:'local',id:'other',intent:'pause',config:{concurrency:36}},'Changing a limit preserves pickup intent');
assert.equal(capacityDocument.machines.local.workers[0].config.concurrency,5,'Planning an edit leaves the snapshot unchanged');
assert.throws(()=>workerCapacityUpdate(capacityDocument,'local','small',-1),/between 1 and 1024/);
assert.throws(()=>workerCapacityUpdate(capacityDocument,'local','large',1),/between 1 and 1024/);
assert.throws(()=>workerCapacityUpdate(capacityDocument,'local','retired',1),/no longer/);
assert.throws(()=>workerCapacityUpdate(capacityDocument,'local','missing',1),/no longer/);
console.log('Project worker agent limit checks passed');

const {workerLimitState,configurationProblems}=require('../src/issues/web/fleet.js');
const limitWorker={id:'limit-worker',pid:123,active:0,config:{concurrency:3},desired_concurrency:2};
assert.deepEqual(workerLimitState(limitWorker,{state:'connected',heartbeat:now/1000},now),{limit:2,applied:3,status:'Pending',note:'Waiting for this worker to apply the new limit.'});
assert.equal(workerLimitState({...limitWorker,config:{concurrency:2}},machine,now).status,'');
assert.equal(workerLimitState({...limitWorker,active:3,config:{concurrency:2}},machine,now).status,'Finishing');
assert.equal(workerLimitState(limitWorker,{state:'disconnected'},now).status,'Queued');
assert.equal(workerLimitState(limitWorker,{...machine,configuration_error:'Checkout failed'},now).status,'Blocked');
const checkoutProblem={configuration_error:'github.com/acme/atlas: Git clone failed. Check repository access and Git authentication on this machine.',projects:{'github.com/acme/atlas':{git:'git@github.com:acme/atlas.git',path:'~/Workspace/atlas'}}};
assert.equal(configurationProblems(checkoutProblem)[0].project,'github.com/acme/atlas');
assert.equal(configurationProblems(checkoutProblem)[0].summary,'Couldn’t clone repository');
assert.equal(configurationProblems({...checkoutProblem,configuration_error:'Database unavailable'})[0].project,null);
console.log('Pending limits and actionable checkout errors passed');

assert.equal(configurationProblems({...checkoutProblem,configuration_error:'github.com/acme/atlas: Git clone timed out; check connection'}).length,1,'Semicolons inside one diagnostic do not create unrelated error rows');

const {jobConversation} = require('../src/issues/web/fleet.js');
const jobEntry={machine:{host:'local',state:'disconnected'},run:{id:'job:one',kind:'job',state:'running',machine:'node-one',standalone:true},online:false};
const jobMachine={host:'local',node:'node-one',state:'connected',heartbeat:99,workers:[]};
assert.equal(jobConversation(jobEntry,{machines:[jobMachine]},100000).online,true,'Jobs are live without any ordinary workers');
assert.equal(jobConversation(jobEntry,{machines:[{...jobMachine,heartbeat:80}]},100000).online,false,'Stale job machines remain offline');
assert.equal(jobConversation(jobEntry,{machines:[]},100000).online,false,'Missing machine must not infer liveness from persisted running state');
const ordinary={...jobEntry,run:{kind:'worker'}};
assert.equal(jobConversation(ordinary,{machines:[jobMachine]},100000),ordinary,'Ordinary and Chief entries retain their lifecycle handling');
console.log('Worker-independent job conversation liveness checks passed');
