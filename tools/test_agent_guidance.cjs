const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const guide = fs.readFileSync('src/issues/web/agent-guide.md', 'utf8');
const script = fs.readFileSync('src/issues/web/agent-guide.js', 'utf8');
function browser(url, paired = false) {
  const node = {textContent:guide}, listeners = {};
  const context = {URL, location:{href:url}, document:{getElementById:id=>id==='hey-boss-agent-guide'?node:id==='root'&&paired?{}:null},
    window:{addEventListener:(event,fn)=>listeners[event]=fn}, history:{}};
  for (const method of ['pushState','replaceState']) context.history[method]=(_state,_title,url)=>{context.location.href=new URL(url,context.location.href).href;};
  vm.runInNewContext(script,context);
  return {node,context,listeners};
}
for (const fixture of JSON.parse(fs.readFileSync('tests/fixtures/lookup-routes.json','utf8'))) {
  const {node} = browser(fixture.url);
  const quote = "'" + new URL(fixture.url).href.replaceAll("'", "'\"'\"'") + "'";
  assert.ok(node.textContent.includes('hey-boss lookup '+quote+' --json'),fixture.url);
}
const {node,context,listeners}=browser('http://localhost/issues#issue=81');
context.history.pushState(null,'','#issue=82');
assert.ok(node.textContent.includes('#issue=82'));
context.history.replaceState(null,'','#project=named%3AStudio&issue=83');
assert.ok(node.textContent.includes('Studio&issue=83'));
context.location.href='http://localhost/artifacts#artifact=some-id';listeners.hashchange();
assert.ok(node.textContent.includes('artifacts#artifact=some-id'));
context.location.href='http://localhost/mm#node=topic';listeners.popstate();
assert.ok(node.textContent.includes('mm#node=topic'));
assert.ok(browser('https://example.com/issues#project=Bob\'s&issue=81').node.textContent.includes("Bob'\"'\"'s"));
assert.ok(browser('https://example.com/',true).node.textContent.includes('hey-boss notif inbox --json'));
assert.ok(browser('https://example.com/?task=notice-id',true).node.textContent.includes('hey-boss lookup'));
vm.runInNewContext(script,{document:{getElementById:()=>null}});
console.log('Agent guidance: shared routes, shell quoting, navigation and paired Inbox passed');
