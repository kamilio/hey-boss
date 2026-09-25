const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const source = fs.readFileSync('src/issues/web/app.js', 'utf8');

// Exercise the real renderer with a failed optional component. Core actions
// must keep working while the other sections finish rendering.
const empty = () => '';
const start = source.indexOf('function renderDetail(');
const render = source.slice(start, source.indexOf('let markdownSequence', start));
const headingStart = source.indexOf('function mountIssueSection(');
let checks = 0;
for (const state of ['open', 'blocked', 'ready', 'closed', 'draft', 'deleted']) {
  for (const failure of ['progress', 'attachments', 'artifacts', 'subtasks']) {
    const nodes = new Map();
    const node = selector => {
      if (!nodes.has(selector)) nodes.set(selector, {innerHTML:'', value:'', closest:() => ({after(){}}), addEventListener(_event, callback){this.listener=callback;}});
      return nodes.get(selector);
    };
    let mounts = 0;
    const fail = name => () => {mounts++;if (failure === name) throw Error('Unavailable ' + name);};
    let failures = 0;
    const context = vm.createContext({
      console:{error(){failures++;}},
      model:{project:{id:'named:QA',name:'QA'},route:{},actor:{id:'human:boss'}},
      $:node, $$:() => [], document:{querySelector:node},
      esc:String, icon:empty, date:empty, avatar:empty, actorName:empty,
      renderDraftNotice:empty, renderIssueWork:empty, renderIssueContext:empty,
      renderPullRequests:empty, issueStateActions:empty, renderIssueComment:empty,
      mountIssueProgress:fail('progress'), placeIssueWork(){}, secureLinks(){}, loadRelatedNotices(){},
      HeyBossStatus:{card:empty}, HeyBossOrigin:{creator:empty},
      IssueSubtasks:{parent:empty,card:empty,rendered:fail('subtasks')},
      HeyBossAttachments:{mount:fail('attachments')}, HeyBossArtifacts:{mount:fail('artifacts')},
      persistDrafts:true, storage:{get:empty}, draftKey:empty, submitComment(){}, loadHistory(){},
      openTransfer(){checks++;},
    });
    vm.runInContext(fs.readFileSync('src/issues/web/blockers.js', 'utf8'), context);
    if (headingStart >= 0) vm.runInContext(source.slice(headingStart, start), context);
    vm.runInContext(render, context);
    assert.doesNotThrow(() => context.renderDetail({issue:{number:1,title:'Move me',state:state==='draft'?'open':state,draft:state==='draft',deleted_at:state==='deleted'?1:null},comments:[]}));
    assert.equal(mounts, 4, 'A failed section must not stop other sections from mounting');
    assert.equal(failures, 1, 'Failed section is reported without aborting the detail view');
    const html = node('#detail-view').innerHTML;
    if (state === 'deleted') {
      assert.doesNotMatch(html, /data-transfer/);
    } else {
      assert.match(html, /data-transfer/, `${state}: Move to project survives failed ${failure}`);
      assert.match(html, /More issue actions/);
      assert.match(html, /data-action="delete"/);
      const listenerStart = source.indexOf('$("#detail-view").addEventListener("click"');
      vm.runInContext(source.slice(listenerStart, source.indexOf('async function performAction(', listenerStart)), context);
      const button = {hasAttribute:name=>name==='data-transfer',dataset:{}};
      node('#detail-view').listener({target:{closest:()=>button}});
    }
  }
}
assert.equal(checks, 20, 'Move remains actionable in every non-deleted state');
console.log('COMPLETE: 24/24 transfer control render checks; 20/20 delegated actions');
