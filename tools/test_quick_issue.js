const assert = require('node:assert/strict');
const {parse} = require('../src/issues/web/quick-issue.js');
const projects = [
  {id:'github.com/kamilio/hey-boss',name:'hey-boss'},
  {id:'github.com/kamilio/poe-code',name:'poe-code'},
  {id:'named:Design Team',name:'Design Team'},
  {id:'named:café',name:'café'},
];
const current = projects[0];
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
