#!/usr/bin/env python3
"""Global profile durability and conflict handling across isolated replicas."""
import importlib.util
import json
import pathlib
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('fleet_tests', ROOT / 'tools/test_fleet_hey_boss.py')
base = importlib.util.module_from_spec(spec)
spec.loader.exec_module(base)


class GlobalProfileFleetTests(unittest.TestCase):
    setUp = base.FleetTests.setUp
    tearDown = base.FleetTests.tearDown
    upload = base.FleetTests.upload
    issue = base.FleetTests.issue

    def rename(self, database, name):
        with database:
            database.execute('UPDATE global_settings SET boss_name=?,version=version+1 WHERE id=1', (name,))

    def test_offline_profile_rename_survives_reopen_and_lost_ack(self):
        original = self.issue(self.main)
        self.rename(self.agent, 'Offline Boss')
        self.agent.close()
        self.agent = base.fleet.connect_db(self.agent_path)
        changes, receipts = self.upload()
        self.assertEqual([r['state'] for r in receipts], ['applied'])
        version = self.main.execute('SELECT version FROM global_settings').fetchone()[0]
        with self.main:
            repeated = base.fleet.accept_changes(self.main, 'agent', changes)
            snapshot = base.fleet.export_snapshot(self.main, 'agent')
        self.assertEqual(repeated, receipts)
        self.assertEqual(self.main.execute('SELECT version FROM global_settings').fetchone()[0], version)
        self.assertEqual(self.issue(self.main), original)
        with self.agent:
            base.fleet.apply_pull(self.agent, 'agent', snapshot, receipts)
        self.assertEqual(base.fleet.journal(self.agent), [])
        self.assertEqual(self.agent.execute('SELECT boss_name FROM global_settings').fetchone()[0], 'Offline Boss')

    def test_conflicting_global_names_preserve_canonical_and_rejected_draft(self):
        self.rename(self.agent, 'Agent draft')
        self.rename(self.main, 'Supervisor name')
        changes, receipts = self.upload()
        self.assertEqual(receipts[0]['state'], 'conflict')
        self.assertEqual(self.main.execute('SELECT boss_name FROM global_settings').fetchone()[0], 'Supervisor name')
        data = json.loads(self.main.execute('SELECT data FROM fleet_conflicts').fetchone()[0])
        self.assertEqual(json.loads(data['after_json'])['boss_name'], 'Agent draft')
        with self.main:
            self.assertEqual(base.fleet.accept_changes(self.main, 'agent', changes), receipts)
        self.assertEqual(self.main.execute('SELECT count(*) FROM fleet_conflicts').fetchone()[0], 1)

    def test_matching_concurrent_rename_is_accepted_without_duplicate_profile(self):
        self.rename(self.agent, 'Same Boss')
        self.rename(self.main, 'Same Boss')
        _, receipts = self.upload()
        self.assertEqual(receipts[0]['state'], 'applied')
        self.assertEqual(self.main.execute('SELECT count(*) FROM global_settings').fetchone()[0], 1)
        self.assertEqual(self.issue(self.main)['version'], 1)


if __name__ == '__main__':
    unittest.main()
