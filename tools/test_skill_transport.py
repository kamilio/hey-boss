#!/usr/bin/env python3
"""Exercise real bundles and guarded installs without touching user skills."""
import importlib.util
import pathlib
import tempfile
import sys
sys.dont_write_bytecode = True
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('transport', ROOT / 'src/skill/transport.py')
transport = importlib.util.module_from_spec(spec)
spec.loader.exec_module(transport)

class SkillTransportTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.home = pathlib.Path(self.tmp.name)
        self.addCleanup(self.tmp.cleanup)

    def skill(self, agent, name='stacked-prs', body='Use native stacks.'):
        root = self.home / agent / 'skills' / name
        root.mkdir(parents=True, exist_ok=True)
        (root / 'SKILL.md').write_text('---\nname: '+name+'\ndescription: Work with stacks\n---\n'+body)
        return root

    def test_nested_assets_modes_and_differing_copies_roundtrip(self):
        root = self.skill('.claude')
        script = root / 'scripts/nested/run.sh'
        script.parent.mkdir(parents=True)
        script.write_text('#!/bin/sh\necho works\n')
        script.chmod(0o755)
        self.skill('.codex', body='Older copy')
        snapshot = transport.scan(self.home)
        copies = snapshot['copies']
        self.assertEqual(len(copies), 2)
        self.assertNotEqual(copies[0]['digest'], copies[1]['digest'])
        source = next(c for c in copies if c['agent'] == 'claude')
        transport.install(self.home, [source], copies)
        for agent in ('.codex', '.agents', '.claude'):
            installed = self.home / agent / 'skills/stacked-prs/scripts/nested/run.sh'
            self.assertEqual(installed.read_bytes(), script.read_bytes())
            self.assertTrue(installed.stat().st_mode & 0o111)
        self.assertTrue(list((self.home / '.hey-boss/skill-backups').glob('*/.codex/stacked-prs/SKILL.md')))

    def test_concurrent_edit_is_not_overwritten(self):
        root = self.skill('.claude')
        copies = transport.scan(self.home)['copies']
        (root / 'SKILL.md').write_text('An edit after scanning')
        with self.assertRaisesRegex(ValueError, 'changed since'):
            transport.install(self.home, copies, copies)
        self.assertEqual((root / 'SKILL.md').read_text(), 'An edit after scanning')
        self.assertFalse((self.home / '.codex/skills/stacked-prs').exists())

    def test_unselected_and_extra_files_are_preserved_or_backed_up(self):
        self.skill('.agents', 'private')
        self.skill('.claude')
        copies = transport.scan(self.home)['copies']
        source = next(c for c in copies if c['name'] == 'stacked-prs')
        transport.install(self.home, [source], copies)
        self.assertTrue((self.home / '.agents/skills/private/SKILL.md').exists())
        self.assertFalse((self.home / '.claude/skills/private').exists())

    def test_symlink_escape_and_oversized_skill_are_reported(self):
        root = self.skill('.claude')
        outside = self.home / 'outside'
        outside.write_text('outside bundle')
        (root / 'escape').symlink_to(outside)
        report = transport.scan(self.home)
        self.assertEqual(report['copies'], [])
        self.assertIn('symlink', report['errors'][0])

    def test_malicious_bundle_path_is_rejected_before_writes(self):
        self.skill('.claude')
        copies = transport.scan(self.home)['copies']
        copies[0]['files'][0]['path'] = '../outside'
        with self.assertRaises(ValueError):
            transport.install(self.home, copies, [])
        self.assertFalse((self.home / '.codex/skills').exists())

    def test_unrelated_broken_skill_does_not_block_selected_distribution(self):
        self.skill('.claude')
        broken = self.skill('.agents', 'unselected')
        (broken / 'broken-link').symlink_to(self.home / 'missing')
        report = transport.scan(self.home)
        source = next(c for c in report['copies'] if c['name'] == 'stacked-prs')
        transport.install(self.home, [source], report['copies'])
        self.assertTrue((self.home / '.codex/skills/stacked-prs/SKILL.md').exists())
        self.assertTrue((broken / 'broken-link').is_symlink())

    def test_flat_copy_cannot_hide_concurrent_directory_edit(self):
        root = self.skill('.claude')
        flat = root.parent / 'stacked-prs.md'
        flat.write_text('Legacy flat skill')
        copies = transport.scan(self.home)['copies']
        source = next(c for c in copies if c['path'] == str(root))
        (root / 'SKILL.md').write_text('New directory edit')
        with self.assertRaisesRegex(ValueError, 'changed since'):
            transport.install(self.home, [source], copies)
        self.assertEqual(flat.read_text(), 'Legacy flat skill')

    def test_same_bundle_retry_is_idempotent(self):
        self.skill('.claude')
        copies = transport.scan(self.home)['copies']
        transport.install(self.home, copies, copies)
        transport.install(self.home, copies, copies)
        self.assertEqual(len(transport.scan(self.home)['copies']), 3)

if __name__ == '__main__':
    unittest.main()
