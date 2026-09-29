const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const {execFileSync} = require('node:child_process');

// Inspect the actual packaged manifest: unbundled Swift tests do not enforce ATS.
const temporary = fs.mkdtempSync(path.join(os.tmpdir(), 'desktop-transport-'));
try {
  const app = path.join(temporary, 'Hey Boss.app');
  execFileSync('swift', ['package_hey_boss.swift', '/usr/bin/true', app]);
  const info = JSON.parse(execFileSync('plutil', ['-convert', 'json', '-o', '-', path.join(app, 'Contents/Info.plist')], {encoding:'utf8'}));
  assert.deepEqual(info.NSAppTransportSecurity, {
    NSExceptionDomains: {
      'hey-boss.test': {NSExceptionAllowsInsecureHTTPLoads: true},
    },
  }, 'Only the exact local Issues hostname may use HTTP; other domains retain default ATS');
  console.log('Packaged desktop transport policy verified');
} finally {
  fs.rmSync(temporary, {recursive:true, force:true});
}
