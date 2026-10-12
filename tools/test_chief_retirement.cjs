const assert = require('node:assert/strict');
const {readFileSync} = require('node:fs');
const {projectView} = require('../src/issues/web/fleet.js');
const legacy = {id:'chief:local:legacy',kind:'chief',project_id:'legacy',state:'running',finished_at:null};
const ordinary = {id:'issue-run',project_id:'active',project_name:'Active',finished_at:null};
const groups = projectView({machines:[{host:'local',state:'connected',heartbeat:Date.now()/1000,workers:[{pid:1,runs:[ordinary,legacy],chiefs:[legacy]}]}]});
assert.deepEqual(groups.map(group => group.id), ['active'], 'Legacy snapshots must not restore Chief cards or empty project groups');
assert.deepEqual(groups[0].active.map(entry => entry.run.id), ['issue-run']);
for (const file of ['index.html', 'project-settings.js', 'admin.js']) {
  assert.doesNotMatch(readFileSync(new URL('../src/issues/web/'+file, 'file://'+__filename), 'utf8'), /chief/i, file+' must expose only current workflows');
}
console.log('Retired Chief is absent from settings, previews, and mixed-version fleet views');
