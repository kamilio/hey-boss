#!/usr/bin/env python3
"""Exercises actual native issue schemas and transactional replica journals."""
import importlib.util
import json
import os
import pathlib
import select
import sys
import sqlite3
import subprocess
import tempfile
import threading
import unittest
from unittest import mock

ROOT = pathlib.Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('fleet', ROOT / 'tools/fleet_hey_boss.py')
fleet = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fleet)
BINARY = pathlib.Path(os.environ.get('HEY_BOSS_TEST_BINARY', ROOT / 'target/debug/hey-boss'))
fleet.BINARY = BINARY
PROJECT = 'named:Fleet tests'


class FleetTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.root = pathlib.Path(self.temporary.name)
        self.main_path = self.root / 'main.db'
        environment = os.environ.copy()
        environment['HEY_BOSS_ISSUE_DB'] = str(self.main_path)
        environment.pop('HEY_BOSS_ISSUE_HOST', None)
        subprocess.run([str(BINARY), 'issue', '--project', 'Fleet tests', '--agent', 'human:fixture', '--json', 'create', '--title', 'Original', '--body', 'Requirements'], cwd=self.root, env=environment, capture_output=True, check=True)
        self.main = fleet.connect_db(self.main_path)
        fleet.install_capture(self.main, 'controller', 'main')
        self.workers = [{'id': 'worker', 'config': {'projects': [PROJECT], 'concurrency': 1, 'tags': [], 'directory': str(self.root), 'enabled': True}}]
        with self.main:
            fleet.allocate(self.main, 'agent', self.workers)
        self.agent_path = self.root / 'agent.db'
        self.agent = fleet.connect_db(self.agent_path)
        self.main.backup(self.agent)
        self.agent.execute('DELETE FROM fleet_outbox')
        self.agent.commit()
        fleet.install_capture(self.agent, 'agent', 'agent')
        with self.main:
            snapshot = fleet.export_snapshot(self.main, 'agent')
        with self.agent:
            fleet.apply_pull(self.agent, 'agent', snapshot, [])

    def tearDown(self):
        self.main.close()
        self.agent.close()
        self.temporary.cleanup()

    def edit(self, db, **fields):
        fields['version'] = db.execute('SELECT version+1 FROM issues WHERE number=1').fetchone()[0]
        fields['updated_at'] = 1800000000000
        with db:
            db.execute('UPDATE issues SET ' + ','.join(k + '=?' for k in fields) + ' WHERE number=1', list(fields.values()))

    def upload(self):
        changes = fleet.journal(self.agent)
        with self.main:
            receipts = fleet.accept_changes(self.main, 'agent', changes)
        return changes, receipts

    def issue(self, db):
        return dict(db.execute('SELECT * FROM issues WHERE number=1').fetchone())

    def test_offline_changes_survive_reopen_and_sync(self):
        self.edit(self.agent, body='Offline implementation notes')
        self.agent.close()
        self.agent = fleet.connect_db(self.agent_path)
        _, receipts = self.upload()
        self.assertEqual(receipts[0]['state'], 'applied')
        self.assertEqual(self.issue(self.main)['body'], 'Offline implementation notes')

    def test_lost_ack_replay_does_not_duplicate_mutations(self):
        self.edit(self.agent, body='Offline change')
        changes, receipts = self.upload()
        version = self.issue(self.main)['version']
        with self.main:
            again = fleet.accept_changes(self.main, 'agent', changes)
        self.assertEqual(receipts, again)
        self.assertEqual(self.issue(self.main)['version'], version)

    def test_different_field_edits_merge_without_overwrite(self):
        self.edit(self.agent, body='Agent body')
        self.edit(self.main, title='Controller title')
        _, receipts = self.upload()
        self.assertEqual(receipts[0]['state'], 'applied')
        self.assertEqual(self.issue(self.main)['title'], 'Controller title')
        self.assertEqual(self.issue(self.main)['body'], 'Agent body')

    def test_same_field_conflict_is_retained_and_canonical_preserved(self):
        self.edit(self.agent, title='Agent title')
        self.edit(self.main, title='Controller title')
        _, receipts = self.upload()
        self.assertEqual(receipts[0]['state'], 'conflict')
        self.assertEqual(self.issue(self.main)['title'], 'Controller title')
        saved = self.main.execute('SELECT data FROM fleet_conflicts').fetchone()[0]
        self.assertIn('Agent title', saved)

    def test_changed_requirements_block_offline_completion(self):
        self.edit(self.agent, state='closed', closed_by='human:fixture')
        self.edit(self.main, body='New requirements')
        _, receipts = self.upload()
        self.assertEqual(receipts[0]['state'], 'conflict')
        self.assertEqual(self.issue(self.main)['state'], 'open')

    def test_revoked_allocation_blocks_stale_changes(self):
        self.edit(self.agent, body='Offline')
        with self.main:
            self.main.execute("UPDATE fleet_allocations SET node='other' WHERE issue_number=1")
        _, receipts = self.upload()
        self.assertEqual(receipts[0]['state'], 'conflict')
        self.assertEqual(self.issue(self.main)['body'], 'Requirements')

    def test_pull_acks_without_echoing_and_preserves_other_pending_rows(self):
        self.edit(self.agent, body='Offline')
        _, receipts = self.upload()
        with self.main:
            snapshot = fleet.export_snapshot(self.main, 'agent')
        with self.agent:
            fleet.apply_pull(self.agent, 'agent', snapshot, receipts)
        self.assertEqual(fleet.journal(self.agent), [])
        self.assertEqual(self.issue(self.agent), self.issue(self.main))

    def test_duplicate_comment_and_event_replay_and_echo_are_unique(self):
        with self.agent:
            self.agent.execute('INSERT INTO comments(project_id,issue_number,author,body,created_at) VALUES(?,1,?,?,0)', (PROJECT, 'human:fixture', 'Offline comment'))
            comment = self.agent.execute('SELECT last_insert_rowid()').fetchone()[0]
            self.agent.execute('INSERT INTO events(project_id,issue_number,actor,action,created_at,data) VALUES(?,1,?,?,0,?)', (PROJECT, 'human:fixture', 'commented', json.dumps({'comment_id': comment})))
        changes, receipts = self.upload()
        with self.main:
            fleet.accept_changes(self.main, 'agent', changes)
            snapshot = fleet.export_snapshot(self.main, 'agent')
        with self.agent:
            fleet.apply_pull(self.agent, 'agent', snapshot, receipts)
        self.assertEqual(self.main.execute('SELECT count(*) FROM comments').fetchone()[0], 1)
        self.assertEqual(self.agent.execute('SELECT count(*) FROM comments').fetchone()[0], 1)
        self.assertEqual(self.agent.execute("SELECT count(*) FROM events WHERE action='commented'").fetchone()[0], 1)
        data = json.loads(self.agent.execute("SELECT data FROM events WHERE action='commented'").fetchone()[0])
        self.assertEqual(data['comment_id'], comment)

    def test_allocations_are_exclusive_and_survive_disconnect(self):
        with self.main:
            fleet.allocate(self.main, 'other', self.workers)
        rows = self.main.execute('SELECT node FROM fleet_allocations WHERE issue_number=1').fetchall()
        self.assertEqual([r[0] for r in rows], ['agent'])
        self.assertEqual(self.agent.execute('SELECT node FROM fleet_allocations WHERE issue_number=1').fetchone()[0], 'agent')

    def test_hidden_projects_receive_no_new_allocations_until_restored(self):
        with self.main:
            self.main.execute('DELETE FROM fleet_allocations')
            self.main.execute('UPDATE projects SET hidden_at=123 WHERE id=?', (PROJECT,))
            fleet.allocate(self.main, 'other', self.workers)
        self.assertEqual(self.main.execute('SELECT count(*) FROM fleet_allocations').fetchone()[0], 0)
        with self.main:
            self.main.execute('UPDATE projects SET hidden_at=NULL WHERE id=?', (PROJECT,))
            fleet.allocate(self.main, 'other', self.workers)
        self.assertEqual(self.main.execute('SELECT node FROM fleet_allocations WHERE issue_number=1').fetchone()[0], 'other')

    def test_changed_worker_tags_refill_with_matching_issues(self):
        original = self.issue(self.main)
        with self.main:
            self.main.execute("UPDATE issues SET labels='[\"parked\"]' WHERE number=1")
            for number in (2, 3, 4):
                fleet.put_row(self.main, 'issues', {**original, 'number': number, 'sort_order': number,
                                                  'title': 'Queue ' + str(number),
                                                  'labels': '["parked"]' if number == 2 else '["ready"]'})
            fleet.allocate(self.main, 'agent', self.workers)
        self.assertEqual([r[0] for r in self.main.execute('SELECT issue_number FROM fleet_allocations ORDER BY issue_number')], [1, 2])
        self.workers[0]['config']['tags'] = ['ready']
        with self.main:
            fleet.allocate(self.main, 'agent', self.workers)
        self.assertEqual([r[0] for r in self.main.execute('SELECT issue_number FROM fleet_allocations ORDER BY issue_number')], [1, 2, 3, 4])

    def test_boss_assignment_does_not_hold_worker_allocation_capacity(self):
        original = self.issue(self.main)
        with self.main:
            for number in (2, 3):
                fleet.put_row(self.main, 'issues', {**original, 'number': number,
                                                  'sort_order': number, 'title': 'Queue ' + str(number)})
            fleet.allocate(self.main, 'agent', self.workers)
            boss = json.loads(self.main.execute("SELECT metadata FROM agents WHERE id='human:fixture'").fetchone()[0])
            boss['id'] = 'human:boss'
            self.main.execute('INSERT INTO agents VALUES(?,?,0)', ('human:boss', json.dumps(boss)))
            self.main.execute("UPDATE issues SET assignee='human:boss' WHERE number=1")
            fleet.allocate(self.main, 'agent', self.workers)
        self.assertEqual([r[0] for r in self.main.execute('SELECT issue_number FROM fleet_allocations ORDER BY issue_number')], [1, 2, 3])

    def test_independent_workers_do_not_share_one_allocation_limit(self):
        original = self.issue(self.main)
        workers = [{'id': 'worker-' + str(n), 'config': {**self.workers[0]['config'], 'concurrency': 2}} for n in range(3)]
        with self.main:
            for number in range(2, 13):
                fleet.put_row(self.main, 'issues', {**original, 'number': number,
                                                  'sort_order': number, 'title': 'Queue ' + str(number)})
            fleet.allocate(self.main, 'agent', workers)
        self.assertEqual(self.main.execute('SELECT count(*) FROM fleet_allocations').fetchone()[0], 12)

    def test_overlapping_filters_keep_distinct_work_for_all_slots(self):
        original = self.issue(self.main)
        workers = [{'id': 'worker-' + str(n), 'config': {**self.workers[0]['config'], 'concurrency': 2, 'tags': tags}}
                   for n, tags in enumerate([['ready'], ['ready', 'bug'], ['bug']])]
        with self.main:
            self.main.execute('DELETE FROM fleet_allocations')
            for number in range(1, 14):
                fleet.put_row(self.main, 'issues', {**original, 'number': number, 'sort_order': number,
                                                  'title': 'Queue ' + str(number),
                                                  'labels': '["parked"]' if number == 1 else '["ready","bug"]'})
            fleet.allocate(self.main, 'agent', workers)
        self.assertEqual([r[0] for r in self.main.execute('SELECT issue_number FROM fleet_allocations ORDER BY issue_number')], list(range(2, 14)))

    def test_bootstrap_history_is_deduplicated_and_echo_remains_unique(self):
        row = dict(self.agent.execute("SELECT * FROM events WHERE action='created'").fetchone())
        change = {'seq': 999, 'table_name': 'events', 'before_json': None, 'after_json': json.dumps(row), 'bootstrap': True}
        with self.main:
            receipt = fleet.accept_changes(self.main, 'agent', [change])
            fleet.accept_changes(self.main, 'agent', [change])
            snapshot = fleet.export_snapshot(self.main, 'agent')
        self.assertEqual(receipt[0]['state'], 'applied')
        self.assertEqual(self.main.execute("SELECT count(*) FROM events WHERE action='created'").fetchone()[0], 1)
        with self.agent:
            fleet.apply_pull(self.agent, 'agent', snapshot, [])
        self.assertEqual(self.agent.execute("SELECT count(*) FROM events WHERE action='created'").fetchone()[0], 1)

    def test_bootstrap_number_collision_does_not_attach_history_to_wrong_issue(self):
        wrong = self.issue(self.agent)
        wrong['title'] = 'Unrelated legacy issue'
        changes = [{'seq': 998, 'table_name': 'issues', 'before_json': None, 'after_json': json.dumps(wrong), 'bootstrap': True},
                   {'seq': 999, 'table_name': 'comments', 'before_json': None, 'after_json': json.dumps({'id': 1, 'project_id': PROJECT, 'issue_number': 1, 'author': 'human:fixture', 'body': 'Legacy note', 'created_at': 0}), 'bootstrap': True}]
        with self.main:
            receipts = fleet.accept_changes(self.main, 'agent', changes)
        self.assertEqual([r['state'] for r in receipts], ['conflict', 'conflict'])
        self.assertEqual(self.main.execute('SELECT count(*) FROM comments').fetchone()[0], 0)

    def test_offline_numbers_are_unique_and_unallocated_number_rejected(self):
        row = self.issue(self.agent)
        row['number'] = self.agent.execute('SELECT first_number FROM fleet_number_ranges').fetchone()[0]
        row['title'] = 'Offline created'
        with self.agent:
            fleet.put_row(self.agent, 'issues', row)
        _, receipts = self.upload()
        self.assertEqual(receipts[-1]['state'], 'applied')
        self.assertEqual(self.main.execute('SELECT title FROM issues WHERE number=?', (row['number'],)).fetchone()[0], 'Offline created')
        row['number'] = 99999
        with self.agent:
            fleet.put_row(self.agent, 'issues', row)
        _, receipts = self.upload()
        self.assertEqual(receipts[-1]['state'], 'conflict')

    def test_controller_restart_reconciles_interrupted_deployments(self):
        with self.main:
            fleet.state_set(self.main, 'machines', {'remote': {'host': 'remote', 'state': 'connected', 'deployment': 'updating'}})
        with mock.patch.object(fleet, 'STATE', self.root), mock.patch.object(fleet, 'worker_status', return_value=[]), mock.patch.object(fleet.subprocess, 'check_output', return_value='fixture build'):
            controller = fleet.Controller(self.main_path, 'main')
        self.assertEqual(controller.nodes['remote']['state'], 'disconnected')
        self.assertEqual(controller.nodes['remote']['deployment'], 'outdated')

    def test_remote_deployment_success_is_independent_of_local_upgrade_lock(self):
        controller = fleet.Controller.__new__(fleet.Controller)
        controller.deployment_lock = threading.Lock()
        controller.lock = threading.RLock()
        controller.source = None
        channel = mock.Mock()
        controller.connections = {'remote': channel}
        controller.update = mock.Mock()
        controller.event = mock.Mock()
        report = {'machines': [{'host': 'local', 'status': 'failed', 'error': 'Another upgrade is running'},
                               {'host': 'remote', 'status': 'updated'}]}
        result = subprocess.CompletedProcess([], 1, json.dumps(report), '')
        with mock.patch.object(fleet.subprocess, 'run', return_value=result):
            self.assertTrue(controller.deploy('remote'))
        channel.terminate.assert_called_once()
        controller.update.assert_called_with('remote', deployment='current', deployment_error=None)

    def test_status_does_not_create_directory_projects(self):
        with mock.patch.object(fleet, 'BINARY', BINARY), mock.patch.dict(os.environ, {'HEY_BOSS_ISSUE_DB': str(self.agent_path)}, clear=False):
            fleet.identity()
            before = {r[0] for r in self.agent.execute('SELECT id FROM projects')}
            self.assertEqual(fleet.worker_status(), [])
            after = {r[0] for r in self.agent.execute('SELECT id FROM projects')}
        self.assertEqual(before, after)

    def test_invalid_checkout_does_not_prevent_registering_other_workers(self):
        config = {**self.workers[0]['config'], 'enabled': False}
        workers = [{'id': 'bad-worker', 'config': {**config, 'directory': '/missing/fleet-checkout'}, 'intent': 'pause'},
                   {'id': 'good-worker', 'config': config, 'intent': 'pause'}]
        with mock.patch.object(fleet, 'BINARY', BINARY), mock.patch.object(fleet, 'STATE', self.root), mock.patch.dict(os.environ, {'HEY_BOSS_ISSUE_DB': str(self.agent_path)}, clear=False):
            failures = fleet.configure_workers(self.agent, workers)
        self.assertEqual(len(failures), 1)
        self.assertTrue(failures[0].startswith('bad-worker:'))
        saved = self.agent.execute("SELECT config FROM issue_workers WHERE id='good-worker'").fetchone()
        self.assertIsNotNone(saved)
        self.assertFalse(json.loads(saved[0])['enabled'])

    def test_invalid_config_preserves_saved_desired_state(self):
        path = self.root / 'fleet-agent.json'
        original = {'role': 'agent', 'revision': 'previous', 'workers': []}
        path.write_text(json.dumps(original))
        with mock.patch.object(fleet, 'STATE', self.root), mock.patch.object(fleet, 'configure_workers', return_value=['bad checkout']):
            result = fleet.configure_agent(self.agent, {'controller': 'main', 'revision': 'invalid', 'workers': []})
        self.assertIn('configuration_error', result)
        self.assertEqual(json.loads(path.read_text()), original)
        self.assertIsNone(fleet.state_get(self.agent, 'revision'))

    def test_stopped_worker_receives_no_new_allocations(self):
        stopped = [{'id': 'stopped', 'config': self.workers[0]['config'], 'intent': 'stop'}]
        with self.main:
            fleet.allocate(self.main, 'stopped-machine', stopped)
        self.assertEqual(self.main.execute("SELECT count(*) FROM fleet_allocations WHERE node='stopped-machine'").fetchone()[0], 0)
        self.assertEqual(self.main.execute("SELECT count(*) FROM fleet_ranges WHERE node='stopped-machine'").fetchone()[0], 0)

    def test_actual_protocol_reconnect_upload_and_pull(self):
        with self.agent:
            self.agent.execute("UPDATE fleet_meta SET role='standalone'")
            self.agent.execute('DROP TABLE fleet_state')
        state = self.root / 'protocol-state'
        state.mkdir()
        environment = {**os.environ, 'HEY_BOSS_ISSUE_DB': str(self.agent_path),
                       'HEY_BOSS_FLEET_STATE': str(state), 'HEY_BOSS_FLEET_BINARY': str(BINARY)}
        environment.pop('HEY_BOSS_ISSUE_HOST', None)

        def open_agent():
            process = subprocess.Popen([str(BINARY), 'fleet', 'agent', '--stdio'],
                                       stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                       text=True, env=environment, cwd=self.root)
            self.addCleanup(lambda: close_agent(process) if process.poll() is None else None)
            return process

        def close_agent(process):
            process.stdin.close()
            process.wait(timeout=10)
            error = process.stderr.read()
            process.stdout.close()
            process.stderr.close()
            self.assertEqual(process.returncode, 0, error)

        def receive(process):
            self.assertTrue(select.select([process.stdout], [], [], 10)[0], 'Agent response timed out')
            return json.loads(process.stdout.readline())

        def send(process, message):
            process.stdin.write(json.dumps({'version': 1, **message}) + '\n')
            process.stdin.flush()

        process = open_agent()
        hello = receive(process)
        self.assertEqual(hello['kind'], 'hello')
        node = hello['node']
        with self.main:
            for project in hello['projects']:
                if fleet.current_row(self.main, 'projects', project) is None:
                    fleet.put_row(self.main, 'projects', project)
        send(process, {'kind': 'configure', 'controller': 'main', 'revision': 'fixture-revision', 'workers': []})
        self.assertEqual(receive(process)['revision'], 'fixture-revision')
        close_agent(process)
        self.edit(self.agent, body='Completed independently while disconnected')
        process = open_agent()
        hello = receive(process)
        self.assertEqual(hello['revision'], 'fixture-revision')
        send(process, {'kind': 'ping'})
        heartbeat = receive(process)
        self.assertEqual(heartbeat['kind'], 'heartbeat')
        self.assertGreater(heartbeat['pending'], 0)
        with self.main:
            self.main.execute('UPDATE fleet_allocations SET node=?', (node,))
            receipts = fleet.accept_changes(self.main, node, heartbeat['changes'])
            snapshot = fleet.export_snapshot(self.main, node)
        self.assertTrue(all(r['state'] == 'applied' for r in receipts), receipts)
        send(process, {'kind': 'pull', 'payload': snapshot, 'receipts': receipts})
        self.assertEqual(receive(process)['pending'], 0)
        send(process, {'kind': 'ping'})
        self.assertEqual(receive(process)['changes'], [])
        self.assertEqual(self.issue(self.main)['body'], 'Completed independently while disconnected')
        close_agent(process)

    def test_signal_replay_does_not_restart_twice(self):
        message = {'id': 'signal-id', 'worker': 'worker', 'signal': 'restart'}
        worker = {'id': 'worker', 'config': {}, 'pid': None, 'active': 0}
        with mock.patch.object(fleet, 'worker_status', return_value=[worker]), mock.patch.object(fleet, 'control_worker'), mock.patch.object(fleet, 'start_worker') as start, mock.patch.object(fleet, 'STATE', self.root):
            first = fleet.apply_signal(self.agent, message)
            second = fleet.apply_signal(self.agent, message)
        self.assertEqual(first, second)
        start.assert_called_once()

    def test_capture_rolls_back_with_domain_transaction(self):
        with self.assertRaises(RuntimeError):
            with self.agent:
                self.agent.execute("UPDATE issues SET body='Rolled back' WHERE number=1")
                raise RuntimeError('abort')
        self.assertEqual(fleet.journal(self.agent), [])
        self.assertEqual(self.issue(self.agent)['body'], 'Requirements')

    def test_restart_replay_after_start_before_ack_preserves_new_supervisor(self):
        message = {'id': 'interrupted-signal', 'worker': 'worker', 'signal': 'restart'}
        with self.agent:
            self.agent.execute("INSERT INTO fleet_signals VALUES('interrupted-signal','local','worker','restart','starting',?,0)", (json.dumps({'prior_pid': 100}),))
        worker = {'id': 'worker', 'config': {}, 'pid': 200, 'active': 0}
        with mock.patch.object(fleet, 'worker_status', return_value=[worker]), mock.patch.object(fleet, 'control_worker') as control, mock.patch.object(fleet, 'start_worker') as start, mock.patch.object(fleet, 'STATE', self.root):
            receipt = fleet.apply_signal(self.agent, message)
        self.assertEqual(receipt['state'], 'acknowledged')
        start.assert_not_called()
        control.assert_not_called()


if __name__ == '__main__':
    unittest.main()
