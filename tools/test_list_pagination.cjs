const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const context = vm.createContext({});
vm.runInContext(fs.readFileSync('src/issues/web/pagination.js', 'utf8'), context);
const {page, number} = context.HeyBossPagination;
for (const value of [undefined, '', '0', '-2', '1.5', 'Infinity', 'NaN', '9007199254740992']) assert.equal(number(value), 1);
assert.equal(number('3'), 3);
for (const count of [0, 1, 999, 1000, 1001, 2000, 2005]) {
  const items = Array.from({length:count}, (_, i) => i);
  const pages = Math.max(1, Math.ceil(count / 1000));
  const collected = [];
  for (let index = 1; index <= pages; index++) {
    const result = page(items, index);
    assert.equal(result.current, index);
    assert.equal(result.pages, pages);
    assert.equal(result.total, count);
    assert.ok(result.items.length <= 1000);
    collected.push(...result.items);
  }
  assert.deepEqual(collected, items, 'Every result appears exactly once');
  assert.equal(page(items, 999).current, pages, 'Clamp a stale page after deletions');
}
const filtered = Array.from({length:2005}, (_, i) => i).filter(i => i > 1999);
assert.equal(page(filtered, 3).items[0], 2000, 'Filter the full collection before paging');
console.log('PASS: pagination boundaries, invalid routes, full coverage, filters and shrinking lists');

context.URLSearchParams = URLSearchParams;
context.model = {route:{project:'named:Example',host:'',state:'closed',owner:'all',search:'',label:'',blocked:'',page:3,inbox_page:2,inbox_state:'archive',inbox_project:'',inbox_search:''}};
const source=fs.readFileSync('src/issues/web/app.js','utf8');
vm.runInContext(source.slice(source.indexOf('function routeHash('),source.indexOf('function parseRoute(')),context);
const hash=changes=>new URLSearchParams(context.routeHash({...context.model.route,...changes}).slice(1));
assert.equal(hash({issue:2001}).get('page'),'3','Opening detail keeps list position');
assert.equal(hash({notice:'notice-1001'}).get('inbox_page'),'2','Opening notice keeps Inbox position');
for(const changes of [{state:'open'},{host:'remote'},{project:'named:Other'},{search:'needle'},{owner:'mine'},{label:'bug'},{blocked:'hold'}]) assert.equal(hash(changes).get('page'),null,'Issue filter change resets page');
for(const changes of [{inbox_state:'unread'},{inbox_project:'Other'},{inbox_search:'needle'}]) assert.equal(hash(changes).get('inbox_page'),null,'Inbox filter change resets page');
assert.equal(hash({page:1}).get('page'),null,'First page keeps URLs clean');
assert.equal(hash({page:2}).get('page'),'2');
console.log('PASS: detail round trips, filter resets and page URLs');
