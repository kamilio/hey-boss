#!/usr/bin/env python3
"""Real HTTP jobs, two isolated machines, and a failed connection; no user files."""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time
import unittest
import urllib.error
import urllib.request

ROOT = Path(__file__).resolve().parents[1]
BINARY = Path(os.environ.get('HEY_BOSS_TEST_BINARY', ROOT / 'target/debug/hey-boss')).resolve()

class SkillManagerApiTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='skill-manager-test-')
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        for host in ('local', 'studio'):
            home = self.root / host
            (home / '.hey-boss').mkdir(parents=True)
            (home / '.hey-boss/skill-sync.json').write_text('{"selected":[]}')
            root = home / '.claude/skills/stacked-prs'
            root.mkdir(parents=True)
            (root / 'SKILL.md').write_text(f'---\nname: stacked-prs\ndescription: Use native PR stacks\n---\n{host} version\n')
        remote = self.root / 'studio/.agents/skills/remote-only'
        (remote / 'scripts/nested').mkdir(parents=True)
        (remote / 'SKILL.md').write_text('---\nname: remote-only\ndescription: Remote workflow\n---\nRun the helper.')
        (remote / 'scripts/nested/run.sh').write_text('#!/bin/sh\necho hello\n')
        (remote / 'scripts/nested/run.sh').chmod(0o755)
        (remote / 'asset.bin').write_bytes(bytes(range(256)))
        (self.root / 'local/.hey-boss/config.json').write_text('{"ssh_hosts":["studio","offline"]}')
        (self.root / 'bin').mkdir()
        ssh = self.root / 'bin/ssh'
        ssh.write_text('#!/usr/bin/env python3\nimport os,subprocess,sys\nif "offline" in sys.argv:\n print("Offline test machine",file=sys.stderr);sys.exit(255)\nenv=dict(os.environ);env["HOME"]='+repr(str(self.root/'studio'))+'\nsys.exit(subprocess.call(["/bin/sh","-c",sys.argv[-1]],env=env))\n')
        ssh.chmod(0o755)
        self.env = dict(os.environ)
        for key in ('HEY_BOSS_FLEET_CONFIG','HEY_BOSS_FLEET_STATE','HEY_BOSS_FLEET_DESIRED','HEY_BOSS_ISSUE_HOST'):
            self.env.pop(key, None)
        self.env.update(HOME=str(self.root/'local'), HEY_BOSS_STATE_DIR=str(self.root/'state'), PATH=str(self.root/'bin')+':'+self.env['PATH'])
        self.start_server()

    def start_server(self):
        self.log = tempfile.TemporaryFile(mode='w+')
        self.addCleanup(self.log.close)
        self.process = subprocess.Popen([str(BINARY),'issue','web','--port','0','--no-discovery','--json'], cwd=self.root/'local', env=self.env, stdout=self.log, stderr=self.log)
        self.addCleanup(self.stop_server)
        end = time.monotonic()+30
        while time.monotonic()<end:
            self.log.seek(0)
            for line in self.log.read().splitlines():
                try:
                    value=json.loads(line)
                    if value.get('url'):
                        self.url=value['url'].rstrip('/')
                        self.csrf=self.get('/api/bootstrap')['csrf']
                        return
                except json.JSONDecodeError:
                    pass
            if self.process.poll() is not None:
                self.fail('Web server failed: '+self.log.read())
            time.sleep(.1)
        self.fail('Web server startup timed out')

    def stop_server(self):
        if self.process.poll() is None:
            self.process.terminate()
            try:self.process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                self.process.kill();self.process.wait()

    def get(self, path='/api/skills'):
        with urllib.request.urlopen(self.url+path, timeout=5) as response:
            return json.load(response)

    def post(self, payload, csrf=True):
        headers={'Content-Type':'application/json'}
        if csrf:headers['X-Hey-Boss-CSRF']=self.csrf
        request=urllib.request.Request(self.url+'/api/skills', data=json.dumps(payload).encode(), headers=headers)
        with urllib.request.urlopen(request, timeout=5) as response:return json.load(response)

    def finish(self):
        end=time.monotonic()+15
        while time.monotonic()<end:
            result=self.get()
            if not result['busy']:return result
            time.sleep(.1)
        self.fail('Skill job timed out')

    def test_fleet_discovery_guarded_distribution_and_restart(self):
        with self.assertRaises(urllib.error.HTTPError) as failure:self.post({'action':'scan'}, csrf=False)
        self.assertEqual(failure.exception.code,403)
        failure.exception.close()
        self.assertTrue(self.post({'action':'scan'})['busy'])
        report=self.finish()
        machines={m['host']:m for m in report['machines']}
        self.assertEqual(set(machines),{'local','studio','offline'})
        self.assertEqual(machines['offline']['state'],'attention')
        self.assertNotIn('files',machines['studio']['copies'][0])
        source=next(c for c in machines['studio']['copies'] if c['name']=='stacked-prs')
        payload={'action':'distribute','revision':report['revision'],'selected':['stacked-prs','remote-only'],'choices':{},'max_words':300}
        with self.assertRaises(urllib.error.HTTPError) as failure:self.post(payload)
        self.assertEqual(failure.exception.code,409)
        failure.exception.close()
        payload['choices']={'stacked-prs':source['digest']}
        payload['revision']-=1
        with self.assertRaises(urllib.error.HTTPError) as failure:self.post(payload)
        self.assertEqual(failure.exception.code,409)
        failure.exception.close()
        self.assertEqual(self.get()['selected'],['hey-boss'])
        payload['revision']=report['revision']
        self.assertTrue(self.post(payload)['busy'])
        result=self.finish()
        self.assertEqual(result['max_words'],300)
        for host in ('local','studio'):
            for agent in ('.codex','.agents','.claude'):
                target=self.root/host/agent/'skills'
                self.assertIn('studio version',(target/'stacked-prs/SKILL.md').read_text())
                self.assertEqual((target/'remote-only/asset.bin').read_bytes(),bytes(range(256)))
                self.assertTrue((target/'remote-only/scripts/nested/run.sh').stat().st_mode&0o111)
        changed=self.root/'local/.claude/skills/stacked-prs/SKILL.md'
        changed.write_text('Edited after scanning. Keep this.')
        payload['revision']=result['revision']
        self.post(payload)
        result=self.finish()
        self.assertEqual(changed.read_text(),'Edited after scanning. Keep this.')
        self.assertIn('changed since',next(m for m in result['machines'] if m['host']=='local')['error'])
        self.stop_server()
        self.start_server()
        restored=self.get()
        self.assertEqual(restored['choices']['stacked-prs'],source['digest'])
        self.assertIn('remote-only',restored['selected'])
        self.assertFalse(restored['busy'])

if __name__=='__main__':unittest.main()
