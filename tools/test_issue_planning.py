#!/usr/bin/env python3
"""Synthetic human-terminal planning regression tests (never launches real Codex)."""
import json
import os
from pathlib import Path
import pty
import select
import signal
import sqlite3
import subprocess
import tempfile
import time
import unittest

BINARY = Path(os.environ.get('HEY_BOSS_TEST_BINARY', 'target/debug/hey-boss')).resolve()

class Planning(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.checkout = self.root / 'project'
        self.checkout.mkdir()
        self.db = self.root / 'state/issues.db'
        self.bin = self.root / 'bin'
        self.bin.mkdir()
        self.env = dict(os.environ, HEY_BOSS_ISSUE_DB=str(self.db), GIT_CEILING_DIRECTORIES=str(self.root), PATH=str(self.bin)+':'+os.environ['PATH'])
        for key in ('HEY_BOSS_ISSUE_HOST', 'HEY_BOSS_AGENT_ID', 'CODEX_THREAD_ID', 'HEY_BOSS_ISSUE_PROJECT'):
            self.env.pop(key, None)
        self.fake('exit 0')

    def fake(self, code):
        script = self.bin / 'codex'
        script.write_text('#!/bin/sh\nprintf "%s\\n" "$@" > codex-args\n'+code+'\n')
        script.chmod(0o755)

    def cli(self, *args, ok=True):
        r = subprocess.run([str(BINARY), 'issue', '--json', '--agent', 'human:test', *args], cwd=self.checkout, env=self.env, capture_output=True, text=True)
        self.assertEqual(r.returncode == 0, ok, r.stdout+r.stderr)
        return json.loads(r.stdout)

    def human(self, args, answer=None):
        master, slave = pty.openpty()
        p = subprocess.Popen([str(BINARY), 'issue', '--agent', 'human:test', *args], cwd=self.checkout, env=self.env, stdin=slave, stdout=slave, stderr=slave)
        os.close(slave)
        output = b''
        deadline = time.monotonic()+15
        sent = False
        while time.monotonic()<deadline:
            if select.select([master], [], [], .1)[0]:
                try:
                    chunk = os.read(master, 65536)
                except OSError:
                    break
                output += chunk
                if answer is not None and b'Choose hey-boss or file' in output and not sent:
                    os.write(master, (answer+'\n').encode())
                    sent = True
            if p.poll() is not None:
                break
        if p.poll() is None:
            p.kill()
            self.fail('Human workflow timed out: '+output.decode())
        p.wait()
        os.close(master)
        return p.returncode, output.decode()

    def wait_for(self, predicate, seconds=15):
        deadline = time.monotonic()+seconds
        while time.monotonic()<deadline:
            if predicate(): return
            time.sleep(.1)
        self.fail('Sync did not converge')

    def stop_owners(self):
        for f in (self.db.parent / 'plan-sync').glob('*.lock'):
            try:
                pid = int(f.read_text())
                os.kill(pid, signal.SIGTERM)
            except (ValueError, ProcessLookupError): pass

    def tearDown(self):
        self.stop_owners()
        self.temp.cleanup()

    def test_generated_plan_final_sync_and_continued_assigned_sync(self):
        self.fake("for file in plans/*.md; do printf '# Final title\\n\\nFinal body\\n' > \"$file\"; done\nexit 0")
        code, out = self.human(['create', '--title', 'Initial', '--body', 'Initial body', '--interactive'])
        self.assertEqual(code, 0, out)
        self.assertNotIn('Choose hey-boss or file', out)
        issue = self.cli('view', '1')['issue']
        self.assertFalse(issue['draft'])
        self.assertEqual((issue['title'],issue['body']), ('Final title','Final body'))
        path = issue['plan']['path']
        self.assertEqual((self.checkout / 'codex-args').read_text().splitlines(), [
            '-c', 'approval_policy="on-request"',
            '-c', 'approvals_reviewer="auto_review"',
            '-c', 'sandbox_mode="workspace-write"',
            'We are planning in '+path,
        ])
        self.cli('claim','1')
        (self.checkout / path).write_text('# Assigned edit\n\nStill syncing\n')
        self.wait_for(lambda: self.cli('view','1')['issue']['title']=='Assigned edit')
        issue = self.cli('view','1')['issue']
        self.assertEqual(issue['plan']['path'],path)
        self.assertEqual(issue['assignee'],'human:test')
        version = issue['version']
        time.sleep(10.3)
        self.assertEqual(self.cli('view','1')['issue']['version'],version)
        owners=list((self.db.parent/'plan-sync').glob('*.lock'))
        pid=owners[0].read_text()
        self.fake('exit 23')
        code,out=self.human(['edit','1','--interactive'])
        self.assertNotEqual(code,0)
        self.assertIn('assigned; plan edits continue updating',out)
        self.assertFalse(self.cli('view','1')['issue']['draft'])
        self.assertEqual(owners[0].read_text(),pid)

    def test_starting_open_issue_cancels_before_drafting_or_overwriting(self):
        self.cli('create','--title','Open title','--body','Original body')
        file=self.checkout/'plan.md'
        file.write_text('# File title\n\nFile body\n')
        before=self.cli('view','1')['issue']
        code,out=self.human(['edit','1','--draft','--interactive','--file','plan.md'],'')
        self.assertNotEqual(code,0)
        self.assertEqual(self.cli('view','1')['issue'],before)
        self.assertEqual(file.read_text(),'# File title\n\nFile body\n')
        self.assertFalse((self.checkout/'codex-args').exists())
        self.fake('exit 23')
        code,out=self.human(['edit','1','--draft','--interactive','--file','plan.md'],'file')
        self.assertNotEqual(code,0)
        issue=self.cli('view','1')['issue']
        self.assertTrue(issue['draft'])
        self.assertEqual(issue['title'],'File title')
        self.assertEqual(issue['plan']['path'],'plan.md')

    def test_assigned_issue_is_not_modified_when_drafting_is_rejected(self):
        self.cli('create','--title','Assigned title','--body','Assigned body')
        self.cli('claim','1')
        (self.checkout/'plan.md').write_text('# Different file\n\nFile body\n')
        before=self.cli('view','1')['issue']
        code,out=self.human(['edit','1','--draft','--interactive','--file','plan.md'],'file')
        self.assertNotEqual(code,0)
        self.assertEqual(self.cli('view','1')['issue'],before)
        self.assertFalse((self.checkout/'codex-args').exists())

    def test_remote_final_sync_is_required_and_uses_fleet_ssh_alias(self):
        self.cli('create','--title','Original','--draft')
        plan={'path':'plans/remote.md','checkout':'/remote/checkout','machine':'synthetic-remote','host':'unroutable-hostname'}
        with sqlite3.connect(self.db) as db:
            db.execute('UPDATE issues SET plan=? WHERE number=1',(json.dumps(plan),))
            db.execute('CREATE TABLE fleet_state(key TEXT PRIMARY KEY,value TEXT)')
            db.execute('INSERT INTO fleet_state VALUES(?,?)',('machines',json.dumps({'devbox-alias':{'node':'synthetic-remote','host':'devbox-alias'}})))
        ssh=self.bin/'ssh'
        ssh.write_text('#!/bin/sh\ncat > ssh-request\nprintf "%s\\n" "$@" > ssh-args\nexit 255\n')
        ssh.chmod(0o755)
        self.cli('undraft','1',ok=False)
        self.assertTrue(self.cli('view','1')['issue']['draft'])
        self.assertEqual(self.cli('view','1')['issue']['title'],'Original')
        ssh.write_text("""#!/bin/sh
cat > ssh-request
printf '%s\\n' "$@" > ssh-args
printf '%s\\n' '{"ok":true,"title":"Remote final","body":"Remote body"}'
""")
        self.cli('undraft','1')
        self.assertFalse(self.cli('view','1')['issue']['draft'])
        self.assertEqual(self.cli('view','1')['issue']['title'],'Remote final')
        request=json.loads((self.checkout/'ssh-request').read_text())
        self.assertEqual(request['operation']['action'],'read_plan')
        self.assertIn('devbox-alias',(self.checkout/'ssh-args').read_text())

    def test_existing_file_create_no_artificial_prompt_and_abnormal_exit(self):
        file = self.checkout / 'existing.md'
        file.write_text('# From file\n\nKeep [sibling](other.md)\n')
        self.fake('exit 23')
        code,out=self.human(['create','--title','Required CLI title','--interactive','--file','existing.md'])
        self.assertNotEqual(code,0,out)
        self.assertNotIn('Choose hey-boss or file',out)
        issue=self.cli('view','1')['issue']
        self.assertTrue(issue['draft'])
        self.assertEqual(issue['title'],'From file')
        self.assertEqual(issue['plan']['path'],'existing.md')
        self.assertEqual(file.read_text(),'# From file\n\nKeep [sibling](other.md)\n')
        code,out=self.human(['edit','1','--interactive'])
        self.assertNotIn('Choose hey-boss or file',out)
        self.assertNotEqual(code,0)

    def test_divergence_cancel_pauses_existing_owner_then_reconcile_each_direction(self):
        file=self.checkout/'existing.md'
        file.write_text('# File title\n\nFile body\n')
        self.cli('create','--title','Issue title','--body','Issue body','--draft')
        code,out=self.human(['edit','1','--interactive','--file','existing.md'], '')
        self.assertNotEqual(code,0)
        self.assertFalse((self.checkout/'codex-args').exists())
        self.assertEqual(self.cli('view','1')['issue']['title'],'Issue title')
        self.assertEqual(file.read_text(),'# File title\n\nFile body\n')
        self.fake('exit 23')
        code,out=self.human(['edit','1','--interactive','--file','existing.md'], 'hey-boss')
        self.assertNotEqual(code,0)
        self.assertEqual(file.read_text(),'# Issue title\n\nIssue body\n')
        self.cli('edit','1','--title','External edit')
        code,out=self.human(['edit','1','--interactive'], '')
        self.assertNotEqual(code,0)
        time.sleep(10.3)
        self.assertEqual(self.cli('view','1')['issue']['title'],'External edit')
        self.assertIn('# Issue title',file.read_text())
        code,out=self.human(['edit','1','--interactive'], 'file')
        self.assertNotEqual(code,0)
        self.assertEqual(self.cli('view','1')['issue']['title'],'Issue title')
        file.unlink()
        self.cli('undraft','1',ok=False)
        self.assertTrue(self.cli('view','1')['issue']['draft'])
        file.write_text('# Manual final\n\nFinal\n')
        self.cli('settings','set','--no-drafts')
        self.cli('undraft','1')
        self.assertFalse(self.cli('view','1')['issue']['draft'])
        file.write_text('# After manual\n\nEdits continue\n')
        self.wait_for(lambda:self.cli('view','1')['issue']['title']=='After manual')

if __name__ == '__main__': unittest.main()
