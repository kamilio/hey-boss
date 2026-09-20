const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const source = fs.readFileSync('src/issues/web/routes.js', 'utf8').replace('/* ROUTE_DEFINITIONS */ []', fs.readFileSync('src/issues/web/routes.json', 'utf8'));
const context = vm.createContext({URL, URLSearchParams, module:{exports:{}}});
vm.runInContext(source, context);
const router = context.module.exports;
for (const fixture of JSON.parse(fs.readFileSync('tests/fixtures/lookup-routes.json', 'utf8'))) {
  const route = router.resolve(fixture.url);
  for (const key of ['entity','id','project','host']) assert.equal(route[key], fixture[key], `${fixture.url}: ${key}`);
}
for (const invalid of ['-1','0','01','1x','9007199254740992']) assert.equal(router.issueNumber(invalid), false);
assert.equal(router.resolve('http://localhost/unknown'), null);
console.log('Shared browser route contract passed');
