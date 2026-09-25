const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const source = fs.readFileSync('src/issues/web/app.js', 'utf8');
const end = source.indexOf('window.addEventListener("hashchange"');
const start = Math.max(source.lastIndexOf('window.addEventListener("beforeunload"', end), source.lastIndexOf('window.addEventListener("pagehide"', end));

for (const editor of [{key:'editor-draft'}, null]) {
  for (const comment of ['', 'Unsent comment']) {
    const window = new EventTarget();
    let editors = 0, comments = 0;
    vm.runInNewContext(source.slice(start, end), {
      window, model:{editor, route:{view:'issues',issue:1}},
      $:() => ({value:comment}),
      saveEditor:() => editors++, saveComment:() => comments++,
    });
    const exit = new Event('beforeunload', {cancelable:true});
    window.dispatchEvent(exit);
    assert.equal(exit.defaultPrevented, false, 'Leaving must never request an unsaved-change confirmation');
    window.dispatchEvent(new Event('pagehide'));
    assert.equal(editors, 1, 'Page exit flushes the editor draft');
    assert.equal(comments, 1, 'Page exit flushes the comment draft');
  }
}
console.log('PASS: navigation never blocks; page exit saves both draft buffers');
