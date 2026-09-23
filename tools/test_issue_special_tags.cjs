const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const app = fs.readFileSync('src/issues/web/app.js', 'utf8');
const tags = fs.readFileSync('src/issues/web/tags.js', 'utf8');
const elements = new Map();
const element = selector => {
  if (!elements.has(selector)) elements.set(selector, {innerHTML:'', value:'', hidden:true, focus(){}, insertAdjacentHTML(){}, scrollIntoView(){}});
  return elements.get(selector);
};
const context = vm.createContext({
  model:{actor:{id:'human:boss'},project:{id:'qa'},route:{host:null},labels:['ready'],detail:{issue:{number:1,version:1,labels:[]}}},
  esc:String, icon:name=>`<svg data-icon="${name}"></svg>`,
  $:element, $$:()=>[], document:{addEventListener(){}},
  CSS:{escape:String},
  confirmDialog:async()=>{throw Error('YOLO toggles must not open a confirmation');}, toast(){}, refreshProjects:async()=>{},
  mutate:async()=>{throw Error('Tests must install a mutation stub before changing tags');},
});
vm.runInContext(app.slice(app.indexOf('const specialIssueTags ='), app.indexOf('function toast(')), context);
vm.runInContext(tags, context);
(async () => {
  const sidebar = context.renderTagSidebar(context.model.detail.issue);
  assert.ok(!sidebar.includes('agent-permissions'), 'Permissions live in tags, without a separate panel');
  assert.ok(context.label('yolo').includes('data-icon="bolt"'), 'YOLO has its distinctive icon');
  context.openIssueTagPicker({focus(){}});
  assert.ok(element('#issue-tag-options').innerHTML.includes('data-issue-tag="yolo"'), 'YOLO offered even before any issue uses it');
  assert.ok(element('#issue-tag-options').innerHTML.includes('No sandbox or approval prompts'), 'Tag has a concise tooltip');
  const writes = [];
  context.mutate = async (operation, project, host) => {
    writes.push({operation, project, host});
    return {issue:{...context.model.detail.issue,version:context.model.detail.issue.version + 1,
      labels:operation.enabled ? ['yolo'] : []}};
  };
  await context.applyIssueTag('yolo', true);
  assert.equal(writes[0].operation.action, 'set_yolo', 'Special tags use the protected permission action');
  assert.equal(writes[0].operation.if_version, 1, 'Permission changes guard the saved revision');
  assert.equal(writes[0].project, 'qa');
  assert.equal(writes[0].host, null, 'Permission changes retain their original host');
  await context.applyIssueTag('yolo', false);
  assert.equal(writes[1].operation.enabled, false, 'Removing the tag revokes YOLO');
  assert.equal(writes[1].operation.if_version, 2, 'Subsequent changes use the new revision');
  context.model.detail.issue.labels = ['yolo'];
  assert.ok(context.renderIssueTagChips(context.model.detail.issue).includes('data-remove-issue-tag="yolo"'), 'Boss removes YOLO with the usual chip control');
  context.model.actor.id = 'codex:test';
  context.renderIssueTagOptions();
  assert.match(element('#issue-tag-options').innerHTML, /data-issue-tag="yolo"[^>]*disabled/, 'Agents cannot change special permissions');
  assert.ok(!context.renderIssueTagChips(context.model.detail.issue).includes('data-remove-issue-tag="yolo"'), 'Agents have no removal control');
  context.model.actor.id = 'human:boss';
  context.model.detail.issue.deleted_at = 1;
  assert.ok(!context.renderIssueTagChips(context.model.detail.issue).includes('data-remove-issue-tag="yolo"'), 'Deleted issues have no removal control');
  console.log('Special issue tag controls passed');
})().catch(error => {console.error(error); process.exitCode=1;});
