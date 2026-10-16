const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const nodes = new Map();
for (const id of ['nav-artifacts','nav-mindmaps','nav-merged-prs','nav-jobs','nav-workers','nav-skills','nav-issues','nav-inbox']) nodes.set('#'+id, {hidden:false});
const context = vm.createContext({
  URLSearchParams, location:{hash:'#project=one&host=remote',pathname:'/'},
  localStorage:{setItem(){}}, document:{documentElement:{dataset:{}},addEventListener(){},
    querySelector:selector=>nodes.get(selector),querySelectorAll:()=>[]},
});
vm.runInContext(fs.readFileSync('src/issues/web/components.js','utf8')+'\nthis.ui = HeyBossUI;',context);
const update = project => context.ui.ProjectPicker.prototype.update.call({projects:[],project:null}, [project], project);
for(const id of ['project-name','project-menu']) nodes.set('#'+id,{hidden:true});
update({id:'one',name:'One',artifacts_enabled:false,mindmaps_enabled:false,prs_enabled:false});
for(const id of ['artifacts','mindmaps','merged-prs']) assert.equal(nodes.get('#nav-'+id).hidden,true,id+' disabled');
update({id:'two',name:'Two',artifacts_enabled:true,mindmaps_enabled:false,prs_enabled:true});
assert.equal(nodes.get('#nav-artifacts').hidden,false,'enabled artifacts restored when switching');
assert.equal(nodes.get('#nav-mindmaps').hidden,true,'independent mindmap setting');
assert.equal(nodes.get('#nav-merged-prs').hidden,false,'enabled PR navigation restored');
assert.match(nodes.get('#nav-artifacts').href,/project=two&host=remote/);
update({id:'legacy',name:'Legacy'});
assert.equal(nodes.get('#nav-artifacts').hidden,false,'legacy default artifacts enabled');
assert.equal(nodes.get('#nav-mindmaps').hidden,false,'legacy default mindmaps enabled');
assert.equal(nodes.get('#nav-merged-prs').hidden,true,'PR workflows default off');
console.log('Project navigation follows feature flags, defaults and project switches');

context.ui.ProjectPicker.prototype.update.call({},[{id:'identity',artifacts_enabled:false,mindmaps_enabled:false}],{id:'identity',name:'Identity only'});
assert.equal(nodes.get('#nav-artifacts').hidden,true,'identity-only pages use project settings');
assert.equal(nodes.get('#nav-mindmaps').hidden,true,'identity-only mindmap keeps settings');

context.location.hash = '';
let savedProject;
context.localStorage.getItem = () => JSON.stringify(savedProject);
for (savedProject of ['local:machine:/Users/kjopek', 'local:machine:/home/kjopek', 'local:machine:/root']) {
  assert.equal(context.ui.projectId('github.com/example/real'), 'github.com/example/real', 'ignore a previously auto-saved home directory');
}
savedProject = 'github.com/example/chosen';
assert.equal(context.ui.projectId('github.com/example/real'), savedProject, 'keep a real saved project');
context.location.hash = '#project=github.com/example/link';
assert.equal(context.ui.projectId('github.com/example/real'), 'github.com/example/link', 'explicit links win');
