import assert from 'node:assert/strict';
import {parseUnifiedDiff} from '../src/issues/web/git-review/diff.js';
const patch='diff --git a/a.txt b/a.txt\n--- a/a.txt\n+++ b/a.txt\n@@ -1,1 +1,2 @@\n--- literal\n++++ literal\n+second\n';
const [file]=parseUnifiedDiff(patch);
assert.equal(file.path,'a.txt');
assert.deepEqual(file.hunks[0].lines.map(l=>l.content),['-- literal','+++ literal','second']);
const [deleted]=parseUnifiedDiff('diff --git a/gone.txt b/gone.txt\ndeleted file mode 100644\n--- a/gone.txt\n+++ /dev/null\n@@ -1 +0,0 @@\n-removed\n');
assert.equal(deleted.path,'gone.txt');
console.log('Commit diff parser: literal headers and deleted paths pass');
