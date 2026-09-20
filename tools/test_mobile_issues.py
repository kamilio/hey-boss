"""Phone delivery uses the real authoritative store and its request deduplication."""
import importlib.util
import os
import pathlib
import subprocess
import tempfile
import unittest
from unittest import mock

ROOT = pathlib.Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('fleet', ROOT / 'tools/test_fleet_reference.py')
fleet = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fleet)
fleet.BINARY = pathlib.Path(os.environ.get('HEY_BOSS_TEST_BINARY', ROOT / 'target/debug/hey-boss')).resolve()

class MobileIssuesTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = pathlib.Path(self.temp.name)
        self.path = self.root / 'issues.db'
        self.env = mock.patch.dict(os.environ, {'HEY_BOSS_ISSUE_DB': str(self.path)})
        self.env.start()
        subprocess.run([str(fleet.BINARY), 'issue', '--project', 'Phone tests', '--agent', 'human:boss', '--json', 'create', '--title', 'Fixture'], capture_output=True, check=True)
        self.bridge = fleet.MobileIssues(self.path, 'machine')
        self.creation = {'requestID': 'phone-123', 'project': 'named:Phone tests', 'title': 'Over cellular', 'body': 'All text retained', 'labels': ['ready']}
        self.results = []
        self.fail_ack = False
    def tearDown(self):
        self.env.stop()
        self.temp.cleanup()
    def hub(self, path, body=None):
        if path == '/api/bridge/issues':
            return {'creations': [self.creation]}
        if path.endswith('/result'):
            if self.fail_ack:
                raise TimeoutError('Lost acknowledgment')
            self.results.append(body)
        return {'ok': True}
    def test_lost_ack_and_restart_create_exactly_one_issue(self):
        self.fail_ack = True
        with mock.patch.object(self.bridge, 'call', side_effect=self.hub):
            with self.assertRaises(TimeoutError):
                self.bridge.sync()
        self.bridge = fleet.MobileIssues(self.path, 'machine')
        self.fail_ack = False
        with mock.patch.object(self.bridge, 'call', side_effect=self.hub):
            self.bridge.sync()
            self.bridge.sync()
        self.assertEqual(self.results, [{'status': 'synced', 'number': 2}] * 2)
        with fleet.connect_db(self.path) as db:
            self.assertEqual(db.execute("SELECT count(*) FROM issues WHERE title='Over cellular'").fetchone()[0], 1)
            self.assertEqual(db.execute("SELECT created_by FROM issues WHERE number=2").fetchone()[0], 'human:boss')
    def test_native_validation_error_is_actionable_and_does_not_create(self):
        self.creation['title'] = 'bad\nline'
        with mock.patch.object(self.bridge, 'call', side_effect=self.hub):
            self.bridge.sync()
        self.assertEqual(self.results[0]['status'], 'error')
        self.assertIn('control characters', self.results[0]['error'])
    def test_accepted_creation_still_syncs_if_project_is_hidden_before_ack(self):
        self.fail_ack = True
        with mock.patch.object(self.bridge, 'call', side_effect=self.hub):
            with self.assertRaises(TimeoutError):
                self.bridge.sync()
        subprocess.run([str(fleet.BINARY), 'issue', '--project', 'Phone tests', '--agent', 'human:boss', '--json', 'hide-project'], capture_output=True, check=True)
        self.fail_ack = False
        with mock.patch.object(self.bridge, 'call', side_effect=self.hub):
            self.bridge.sync()
        self.assertEqual(self.results, [{'status': 'synced', 'number': 2}])
    def test_unknown_project_cannot_register_itself(self):
        self.creation['project'] = 'named:Unknown'
        with mock.patch.object(self.bridge, 'call', side_effect=self.hub):
            self.bridge.sync()
        self.assertEqual(self.results[0]['status'], 'error')
        with fleet.connect_db(self.path) as db:
            self.assertEqual(db.execute("SELECT count(*) FROM projects WHERE id='named:Unknown'").fetchone()[0], 0)
    def test_transient_store_failure_stays_pending(self):
        with mock.patch.object(self.bridge, 'call', side_effect=self.hub), mock.patch.object(self.bridge, 'rpc', side_effect=RuntimeError('Database busy')):
            with self.assertRaises(RuntimeError):
                self.bridge.sync()
        self.assertEqual(self.results, [])

    def test_artifact_bridge_replays_one_document_and_reports_conflicts(self):
        request = {'id': 'artifact-create', 'project': 'named:Phone tests', 'operation': {
            'action': 'artifact', 'operation': {'command': 'create', 'title': 'Phone plan', 'body': '# Plan', 'issue': 1}}}
        results = []
        def hub(path, body=None):
            if path == '/api/bridge/artifacts':
                return {'requests': [request]}
            if path.endswith('/result'):
                results.append(body)
            return {'ok': True}
        projects = {'named:Phone tests': {'id': 'named:Phone tests', 'name': 'Phone tests'}}
        with mock.patch.object(self.bridge, 'call', side_effect=hub):
            self.bridge.sync_artifacts(projects, set(projects))
            self.bridge.sync_artifacts(projects, set(projects))
        self.assertTrue(results[0]['ok'])
        self.assertEqual(results[0]['artifact']['id'], results[1]['artifact']['id'])
        with fleet.connect_db(self.path) as db:
            self.assertEqual(db.execute('SELECT count(*) FROM artifacts').fetchone()[0], 1)
        artifact = results[0]['artifact']['id']
        request.update(id='artifact-edit', operation={'action': 'artifact', 'operation': {
            'command': 'edit', 'id': artifact, 'body': 'Updated', 'if_version': 1}})
        with mock.patch.object(self.bridge, 'call', side_effect=hub):
            self.bridge.sync_artifacts(projects, set(projects))
        request['id'] = 'stale-edit'
        with mock.patch.object(self.bridge, 'call', side_effect=hub):
            self.bridge.sync_artifacts(projects, set(projects))
        self.assertEqual(results[-1]['error']['code'], 'conflict')

if __name__ == '__main__':
    unittest.main()
