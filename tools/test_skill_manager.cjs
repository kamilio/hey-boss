const assert = require('node:assert/strict');
const {library, visibleSkills, unresolved, computeDiff, diffFiles, getMarkdownFiles} = require('../src/issues/web/skills.js');
const copy = (name,digest,agent='codex',scope='global',text='Use git.') => ({name,digest,agent,scope,text,description:'Stacks',warnings:[]});
const data = {machines:[
  {host:'laptop',state:'online',copies:[copy('stacked-prs','a','codex','global','Line 1\nLine 2\nOld line\nLine 4'),copy('stacked-prs','a','claude','global','Line 1\nLine 2\nOld line\nLine 4'),copy('AGENTS.md','m1','codex','global','# Grand instructions\nAlways test.')]},
  {host:'desktop',state:'attention',copies:[copy('stacked-prs','b','codex','global','Line 1\nLine 2\nNew line\nExtra line\nLine 4'),copy('remote-only','c'),copy('local-task','p','agents','project')]}
]};
const skills = library(data);
assert.equal(skills.length,4);
assert.equal(skills[0].name,'AGENTS.md');
const stack = skills.find(s=>s.name==='stacked-prs');
assert.equal(stack.versions.length,2);
assert.equal(stack.copies.length,3);
assert.equal(stack.machines.length,2);
assert.equal(skills.find(s=>s.name==='remote-only').copies[0].stale,true);
assert.deepEqual(unresolved(skills,new Set(['stacked-prs']),{}),['stacked-prs']);
assert.deepEqual(unresolved(skills,new Set(['stacked-prs']),{'stacked-prs':'b'}),[]);
assert.deepEqual(unresolved(skills,new Set(['stacked-prs']),{'stacked-prs':'deleted'}),['stacked-prs']);
assert.equal(visibleSkills(skills,'desktop','all',new Set()).length,2);
assert.equal(visibleSkills(skills,'','selected',new Set(['stacked-prs'])).length,1);
assert.equal(visibleSkills(skills,'','project',new Set()).length,1);

const diff = computeDiff('Line 1\nLine 2\nOld line\nLine 4', 'Line 1\nLine 2\nNew line\nExtra line\nLine 4');
assert.equal(diff.additions, 2);
assert.equal(diff.deletions, 1);
assert.equal(diff.hunks.length, 1);
assert.ok(diff.hunks[0].header.startsWith('@@ -1,4 +1,5 @@'));

const filesDiff = diffFiles(
  {name:'stacked-prs', markdown_files:[{path:'SKILL.md',text:'A\nB'},{path:'references/guide.md',text:'Old guide'}]},
  {name:'stacked-prs', markdown_files:[{path:'SKILL.md',text:'A\nB'},{path:'references/guide.md',text:'New guide'}]}
);
assert.equal(filesDiff.find(f=>f.path==='SKILL.md').status, 'unchanged');
assert.equal(filesDiff.find(f=>f.path==='references/guide.md').status, 'modified');
assert.equal(filesDiff.find(f=>f.path==='references/guide.md').additions, 1);
assert.equal(filesDiff.find(f=>f.path==='references/guide.md').deletions, 1);
assert.equal(getMarkdownFiles({name:'AGENTS.md',text:'# Codex'})[0].path, 'AGENTS.md');

console.log('Skill library aggregation, GitHub diffing, multi-file Markdown, conflicts, and filtering passed');
