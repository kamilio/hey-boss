const {test} = require('node:test');
const assert = require('node:assert/strict');
process.env.TZ = 'America/Chicago';
const {groups, render} = require('../src/issues/web/merged-prs.js');
test('groups by local calendar day, newest first, with undated records last', () => {
 const rows = [
  {url:'https://github.com/o/r/pull/1', merged_at:Date.parse('2026-09-30T04:59:00Z')},
  {url:'https://github.com/o/r/pull/2', merged_at:Date.parse('2026-09-30T05:01:00Z')},
  {url:'https://github.com/o/r/pull/3', merged_at:Date.parse('2026-09-30T08:00:00Z')},
  {url:'https://github.com/o/r/pull/4', merged_at:null, observed_at:Date.now()}
 ];
 const result=groups(rows,new Date('2026-09-30T12:00:00Z'));
 assert.deepEqual(result.map(g=>g.label),['Today','Yesterday','Merge date unavailable']);
 assert.deepEqual(result.map(g=>g.rows.length),[2,1,1]);
 assert.equal(result[0].rows[0].url,rows[2].url);
});
test('renders safe PR and issue links with counts and fallback dates', () => {
 const html=render([{url:'https://github.com/o/r/pull/7',title:'<script>bad</script>',merged_at:null,observed_at:1000,issues:[{number:3,title:'Fix'}]}],{project:'github.com/o/r',host:'box'});
 assert.match(html,/&lt;script&gt;/);
 assert.doesNotMatch(html,/<script>/);
 assert.match(html,/1 PR/);
 assert.match(html,/o\/r#7/);
 assert.match(html,/issue=3/);
 assert.match(html,/host=box/);
 assert.match(html,/Observed/);
 assert.doesNotMatch(render([{url:'javascript:alert(1)',issues:[]}],{project:'test'}),/href="javascript:/);
});
