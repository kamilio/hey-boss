const assert = require('node:assert/strict');
const {parse, mentionAt, suggestions, completeMention} = require('../src/issues/web/quick-issue.js');
const projects = [
  {id:'github.com/kamilio/hey-boss',name:'hey-boss'},
  {id:'github.com/kamilio/poe-code',name:'poe-code'},
  {id:'named:Design Team',name:'Design Team'},
  {id:'named:café',name:'café'},
];
const current = projects[0];
const {taskKind, taskLabels} = require('../src/issues/web/quick-issue.js');
assert.equal(taskKind(['ready','task:plan']), 'plan');
assert.equal(taskKind(['task:plan','task:research']), 'plan');
assert.equal(taskKind(['task:research']), 'plan');
assert.equal(taskKind(['plan','research']), 'implement');
assert.deepEqual(taskLabels(['ready','task:research','ready'], 'plan'), ['ready','task:plan']);
assert.deepEqual(taskLabels(['ready'], 'research'), ['ready']);
assert.deepEqual(taskLabels(['bug','task:research'], 'implement'), ['bug']);
const check = (text, title, project = projects[1]) => assert.deepEqual(parse(text, projects, current), {title, project});
check('Fix reconnect', 'Fix reconnect', current);
check('@poe-code Fix reconnect', 'Fix reconnect');
check('Fix @poe-code reconnect', 'Fix reconnect');
check('Fix reconnect @poe-code', 'Fix reconnect');
check('Fix (@poe-code), reconnect', 'Fix (), reconnect');
check('Fix @POE-CODE reconnect', 'Fix reconnect');
check('Fix @github.com/kamilio/poe-code reconnect', 'Fix reconnect');
check('Fix @"Design Team" reconnect', 'Fix reconnect', projects[2]);
check("Fix @'Design Team' reconnect", 'Fix reconnect', projects[2]);
check('Fix @café reconnect', 'Fix reconnect', projects[3]);
check('Fix\n@poe-code\t reconnect', 'Fix reconnect');
check('Fix @poe-code and @poe-code', 'Fix and');
check('Email boss@poe-code.com', 'Email boss@poe-code.com', current);
check('Document \\@poe-code syntax', 'Document @poe-code syntax', current);
check('Fix @poe-code. Now', 'Fix . Now');
for (const text of ['@missing Fix', '@poe-code-extra Fix', '@poe-code @hey-boss Fix', '@"Design Team Fix', '@ Fix', '@poe-code', '   ']) {
  assert.throws(() => parse(text, projects, current), Error, text);
}
assert.throws(() => parse('Fix @poe-code', [...projects,{id:'github.com/other/poe-code',name:'poe-code'}], current), /ambiguous/i);
check('Fix @github.com/kamilio/poe-code', 'Fix');
assert.throws(() => parse('Fix', projects, null), /project/i);
console.log('Quick issue parser checks passed');

assert.deepEqual(mentionAt('Fix @po reconnect', 7), {start:4,end:7,query:'po'});
assert.deepEqual(mentionAt('Fix @poe-code reconnect', 7), {start:4,end:13,query:'po'});
assert.deepEqual(mentionAt('@', 1), {start:0,end:1,query:''});
assert.deepEqual(mentionAt('Fix @po. Now',7), {start:4,end:7,query:'po'});
assert.equal(mentionAt('Fix @po. Now',8), null);
assert.deepEqual(mentionAt('Fix @"Design T" reconnect', 14), {start:4,end:15,query:'Design T'});
for (const text of ['boss@po', 'Fix \\@po', 'Fix @po ', 'Fix @"Design Team" ']) {
  assert.equal(mentionAt(text, text.length), null, text);
}
assert.equal(mentionAt('Fix @po', 2), null);
assert.equal(mentionAt('@"Design @Team"', 13).start, 0);
assert.deepEqual(mentionAt('@𐐀team', 7), {start:0,end:7,query:'𐐀team'});
assert.deepEqual(suggestions(projects, 'PO'), [projects[1]]);
assert.deepEqual(suggestions(projects, 'cafe\u0301'), [projects[3]]);
assert.deepEqual(suggestions(projects, 'kamilio/poe'), [projects[1]]);
assert.deepEqual(suggestions(projects, 'unknown'), []);
assert.deepEqual(suggestions([{id:'named:long',name:'Deploy tools'},{id:'named:exact',name:'Deploy'},{id:'named:contains',name:'QA deploy'}], 'deploy').map(p=>p.id), ['named:exact','named:long','named:contains']);
assert.equal(suggestions(Array.from({length:20}, (_,i)=>({id:`named:${i}`,name:`Project ${i}`})), '').length, 8);
const duplicates = [...projects, {id:'github.com/other/poe-code',name:'poe-code'}];
for (const [text, caret, project, known, expected] of [
  ['Fix @po reconnect',7,projects[1],projects,'Fix @poe-code reconnect'],
  ['Fix @poe-code reconnect',7,projects[0],projects,'Fix @hey-boss reconnect'],
  ['Fix @',5,projects[2],projects,'Fix @"Design Team" '],
  ['Fix @po',7,projects[1],duplicates,'Fix @github.com/kamilio/poe-code '],
  ['Fix (@po), reconnect',8,projects[1],projects,'Fix (@poe-code), reconnect'],
  ['Fix @po. Now',7,projects[1],projects,'Fix @poe-code. Now'],
  ['Fix @"Design T" reconnect',14,projects[2],projects,'Fix @"Design Team" reconnect'],
]) {
  const result = completeMention(text, mentionAt(text,caret), project,known);
  assert.equal(result.text, expected);
  assert.equal(parse(result.text,known,current).project.id,project.id);
  assert(result.caret > result.text.indexOf('@'));
}
const unusual={id:'named:quote',name:'Team "A" \\ B'};
const result=completeMention('Fix @',mentionAt('Fix @',5),unusual,[unusual]);
assert.equal(parse(result.text,[unusual],current).project.id,unusual.id);
console.log('Quick issue typeahead checks passed');
