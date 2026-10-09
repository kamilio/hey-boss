const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const {execFileSync} = require('node:child_process');

const root = fs.mkdtempSync(path.join(os.tmpdir(), 'native-setup-'));
try {
  // Mock only launchd; exercise the real setup code and generated registration.
  const launchctl = path.join(root, 'launchctl');
  fs.writeFileSync(launchctl, '#!/bin/sh\nexit 0\n', {mode: 0o700});
  const source = fs.readFileSync('setup_hey_boss.swift', 'utf8');
  assert.equal(source.split('"/bin/launchctl"').length, 2);
  const script = path.join(root, 'setup.swift');
  fs.writeFileSync(script, source.replace('"/bin/launchctl"', JSON.stringify(launchctl)));
  const setup = path.join(root, 'setup');
  execFileSync('xcrun', ['swiftc', '-module-cache-path', path.join(root, 'cache'), script, '-o', setup]);
  for (const companion of [false, true]) {
    const directory = path.join(root, companion ? 'companion' : 'desktop');
    const bin = path.join(directory, 'bin');
    const state = path.join(directory, 'state');
    const agents = path.join(directory, 'agents');
    fs.mkdirSync(bin, {recursive: true});
    if (companion) fs.writeFileSync(path.join(bin, 'hey-boss.companion'), 'existing companion');
    const daemon = path.join(directory, 'Hey Boss.app/Contents/MacOS/hey-boss-daemon');
    for (let install = 0; install < 2; install++) {
      execFileSync(setup, [state, bin, agents, daemon]);
      const plist = JSON.parse(execFileSync('plutil', ['-convert', 'json', '-o', '-', path.join(agents, 'local.hey-boss.plist')], {encoding: 'utf8'}));
      assert.equal(plist.Sockets.Listener.SockPathName, path.join(state, companion ? 'desktop.sock' : 'daemon.sock'));
      assert.deepEqual(plist.ProgramArguments, [daemon]);
      assert.equal(plist.EnvironmentVariables.HEY_BOSS_CLI_PATH, path.join(bin, 'hey-boss'));
      assert.equal(fs.readFileSync(path.join(bin, 'hey-boss.state'), 'utf8'), state);
      if (companion) assert.equal(fs.readFileSync(path.join(bin, 'hey-boss.companion'), 'utf8'), 'existing companion');
    }
  }
  console.log('Passed: native setup and reinstall preserve separate desktop and companion listeners');
} finally {
  fs.rmSync(root, {recursive: true, force: true});
}
