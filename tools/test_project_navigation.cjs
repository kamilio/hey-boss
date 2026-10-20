const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const nodes = new Map();
const shell = fs.readFileSync('src/issues/web/app-shell.html', 'utf8');
const mainNavigation = shell.slice(shell.indexOf('<nav class="app-navigation"'), shell.indexOf('<dialog'));
const projectLinks = [...mainNavigation.matchAll(/<a id="([^"]+)" href="([^"]+)"/g)].map(([, id]) => id);
for (const [, id, href] of shell.matchAll(/<a id="(nav-[^"]+)" href="([^"]+)"/g)) {
  nodes.set('#' + id, {id, href, hidden:false, getAttribute(name) { return this[name]; }});
}
const context = vm.createContext({
  URLSearchParams, location:{hash:'#project=one&host=remote',pathname:'/'},
  localStorage:{setItem(){}}, document:{documentElement:{dataset:{}},addEventListener(){},
    querySelector:selector=>nodes.get(selector),
    querySelectorAll:selector=>selector === '.app-navigation a, #nav-admin' ? [...projectLinks, 'nav-admin'].map(id => nodes.get('#' + id)) : []},
});
context.history = {replaceState(_state, _unused, hash) { context.location.hash = hash; }};
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

context.location.pathname = '/errors';
context.location.hash = '#project=old&host=remote';
const project = 'named:Beta & more / #2';
update({id:project,name:'Beta'});
assert.equal(new URLSearchParams(context.location.hash.slice(1)).get('project'), project, 'Current URL follows the selected project on reload');
for (const id of [...projectLinks, 'nav-admin']) {
  const link = nodes.get('#' + id), params = new URLSearchParams(link.href.split('#')[1]);
  assert.equal(params.get('project'), project, id + ' carries the current project');
  assert.equal(params.get('host'), 'remote', id + ' carries the remote host');
  assert.equal(params.get('view'), id === 'nav-inbox' ? 'inbox' : null, id + ' opens the correct section');
}
for (const id of ['nav-workers', 'nav-secrets']) {
  assert.equal(nodes.get('#' + id).href, '/' + id.slice(4), id + ' remains global');
}
context.document.documentElement.dataset.issueMobile = 'true';
update({id:'mobile',name:'Mobile'});
for (const id of [...projectLinks, 'nav-admin']) {
  assert.equal(new URLSearchParams(nodes.get('#' + id).href.split('#')[1]).has('host'), false, id + ' omits desktop host on paired devices');
}
context.location.pathname = '/';
nodes.get('#nav-issues').href = '#project=mobile&state=closed';
nodes.get('#nav-inbox').href = '#project=mobile&view=inbox&inbox_state=archive';
update({id:'mobile',name:'Mobile'});
assert.equal(nodes.get('#nav-issues').href, '#project=mobile&state=closed', 'Issue filters remain owned by the issue page');
assert.equal(nodes.get('#nav-inbox').href, '#project=mobile&view=inbox&inbox_state=archive', 'Inbox filters remain owned by the issue page');
console.log('All project sections carry selection; global links and page filters keep their scope');

context.location.pathname = '/errors';
context.location.hash = '';
update({id:project,name:'Beta'});
assert.equal(new URLSearchParams(context.location.hash.slice(1)).get('project'), project, 'Direct project pages pin the remembered selection for browser history');
for (const path of ['/workers', '/secrets']) {
  context.location.pathname = path;
  context.location.hash = '';
  update({id:project,name:'Beta'});
  assert.equal(context.location.hash, '', path + ' stays global without a project URL');
}
context.location.hash = '#project=old';
update({id:project,name:'Beta'});
assert.equal(new URLSearchParams(context.location.hash.slice(1)).get('project'), project, 'Explicit context on a global page follows picker changes');
