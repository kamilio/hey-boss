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
import time
import unittest
from unittest import mock

ROOT = pathlib.Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('fleet', ROOT / 'tools/fleet_hey_boss.py')
fleet = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fleet)
BINARY = pathlib.Path(os.environ.get('HEY_BOSS_TEST_BINARY', ROOT / 'target/debug/hey-boss')).resolve()
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

    def test_snapshot_reads_while_another_connection_holds_the_write_lock(self):
        with fleet.connect_db(self.main_path) as reader:
            reader.execute('PRAGMA busy_timeout=50')
            self.main.execute('BEGIN IMMEDIATE')
            try:
                with reader:
                    snapshot = fleet.export_snapshot(reader, 'agent')
                self.assertEqual(snapshot['tables']['issues'][0]['title'], 'Original')
            finally:
                self.main.rollback()

    def test_snapshot_batches_history_identity_lookups(self):
        with self.main:
            for number in range(30):
                self.main.execute("INSERT INTO comments(project_id,issue_number,author,body,created_at) VALUES(?,1,'human:fixture',?,0)",
                                  (PROJECT, str(number)))
                local_id = self.main.execute('SELECT last_insert_rowid()').fetchone()[0]
                self.main.execute("INSERT INTO fleet_row_ids VALUES('remote','comments',?,?)", (100 + number, local_id))
        with mock.patch.object(self.main, 'execute', wraps=self.main.execute) as execute:
            with self.main:
                snapshot = fleet.export_snapshot(self.main, 'agent')
        comments = snapshot['tables']['comments']
        self.assertEqual([(r['origin'], r['row']['id']) for r in comments],
                         [('remote', 100 + number) for number in range(30)])
        self.assertLess(len(execute.call_args_list), 25, 'Snapshot performs a database call per history row')

    def test_drafts_are_not_allocated_and_project_settings_replicate(self):
        with self.main:
            self.main.execute("DELETE FROM fleet_allocations")
            self.main.execute("UPDATE issues SET draft=1 WHERE project_id=?", (PROJECT,))
            self.main.execute("INSERT INTO project_settings(project_id,prompt,prs_enabled,version,drafts_enabled,plan_template) VALUES(?,'prompt',0,1,0,'plans/{timestamp}.md')", (PROJECT,))
            fleet.allocate(self.main, 'agent', self.workers)
        self.assertEqual(self.main.execute('SELECT count(*) FROM fleet_allocations').fetchone()[0], 0)
        with self.main:
            snapshot = fleet.export_snapshot(self.main, 'agent')
        with self.agent:
            fleet.apply_pull(self.agent, 'agent', snapshot, [])
        self.assertEqual(self.agent.execute('SELECT draft FROM issues WHERE project_id=?', (PROJECT,)).fetchone()[0], 1)
        settings = self.agent.execute('SELECT drafts_enabled,plan_template FROM project_settings WHERE project_id=?', (PROJECT,)).fetchone()
        self.assertEqual(tuple(settings), (0,'plans/{timestamp}.md'))
        with self.main:
            self.main.execute('UPDATE issues SET draft=0 WHERE project_id=?', (PROJECT,))
            fleet.allocate(self.main, 'agent', self.workers)
        self.assertEqual(self.main.execute('SELECT count(*) FROM fleet_allocations').fetchone()[0], 1)

    def test_workflow_project_settings_replicate_and_legacy_rows_use_defaults(self):
        overrides = json.dumps({'worktree': 'Isolate {{number}}', 'main': 'Ship {{number}}'})
        with self.main:
            self.main.execute('INSERT INTO project_settings(project_id,prompt,prs_enabled,version,worktree_enabled,prompt_overrides) VALUES(?,?,?,?,?,?)', (PROJECT, 'Shared instructions', 1, 1, 1, overrides))
            snapshot = fleet.export_snapshot(self.main, 'agent')
        with self.agent:
            fleet.apply_pull(self.agent, 'agent', snapshot, [])
        settings = dict(self.agent.execute('SELECT * FROM project_settings WHERE project_id=?', (PROJECT,)).fetchone())
        self.assertEqual(settings['worktree_enabled'], 1)
        self.assertEqual(json.loads(settings['prompt_overrides']), json.loads(overrides))
        settings.pop('worktree_enabled')
        settings.pop('prompt_overrides')
        with self.agent:
            fleet.put_row(self.agent, 'project_settings', settings)
        saved = self.agent.execute('SELECT worktree_enabled,prompt_overrides FROM project_settings WHERE project_id=?', (PROJECT,)).fetchone()
        self.assertEqual(tuple(saved), (0, '{}'))

    def test_supervisor_status_preserves_existing_saved_state(self):
        with mock.patch.object(fleet, 'STATE', self.root), mock.patch.object(fleet, 'worker_status', return_value=[]):
            supervisor = fleet.Supervisor(self.main_path, 'main')
        supervisor.nodes['remote'] = {'host': 'remote', 'role': 'agent'}
        status = supervisor.status()
        self.assertEqual(status['supervisor'], 'main')
        self.assertEqual(status['controller'], 'main')  # Older status clients.
        self.assertEqual(status['machines'][0]['role'], 'supervisor')
        self.assertEqual(status['machines'][1]['role'], 'companion')
        self.assertEqual(supervisor.nodes['remote']['role'], 'agent')
        self.assertEqual(self.main.execute('SELECT role FROM fleet_meta WHERE id=1').fetchone()[0], 'controller')
        self.assertEqual(json.loads((self.root / 'fleet-main.json').read_text())['role'], 'controller')

    def test_launchd_upgrade_retries_teardown_without_changing_service_identity(self):
        import plistlib
        for role, command in [('controller', 'supervisor'), ('agent', 'companion')]:
            with self.subTest(role=role):
                results = [subprocess.CompletedProcess([], code, '', 'Unloading' if code else '') for code in [0, 5, 0]]
                with mock.patch.object(fleet, 'STATE', self.root), mock.patch.object(pathlib.Path, 'home', return_value=self.root), mock.patch.object(fleet.sys, 'platform', 'darwin'), mock.patch.object(fleet.subprocess, 'run', side_effect=results) as run, mock.patch.object(fleet.time, 'sleep'):
                    fleet.install_service(role)
                bootstraps = [call for call in run.call_args_list if call.args[0][1] == 'bootstrap']
                self.assertEqual(len(bootstraps), 2)
                config = plistlib.loads((self.root / ('Library/LaunchAgents/local.hey-boss-fleet-' + role + '.plist')).read_bytes())
                self.assertEqual(config['Label'], 'local.hey-boss-fleet-' + role)
                self.assertEqual(config['ProgramArguments'][-1], command)

    def test_launchd_upgrade_does_not_retry_unrelated_errors(self):
        results = [subprocess.CompletedProcess([], 0, '', ''), subprocess.CompletedProcess(['launchctl'], 1, '', 'Invalid registration')]
        with mock.patch.object(fleet, 'STATE', self.root), mock.patch.object(pathlib.Path, 'home', return_value=self.root), mock.patch.object(fleet.sys, 'platform', 'darwin'), mock.patch.object(fleet.subprocess, 'run', side_effect=results) as run, mock.patch.object(fleet.time, 'sleep') as sleep:
            with self.assertRaises(subprocess.CalledProcessError):
                fleet.install_service('controller')
        self.assertEqual(run.call_count, 2)
        sleep.assert_not_called()

    def test_launchd_upgrade_bounds_teardown_retries(self):
        def unloading(command, **_):
            return subprocess.CompletedProcess(command, 0 if command[1] == 'bootout' else 5, '', '')
        with mock.patch.object(fleet, 'STATE', self.root), mock.patch.object(pathlib.Path, 'home', return_value=self.root), mock.patch.object(fleet.sys, 'platform', 'darwin'), mock.patch.object(fleet.subprocess, 'run', side_effect=unloading) as run, mock.patch.object(fleet.time, 'sleep') as sleep:
            with self.assertRaises(subprocess.CalledProcessError):
                fleet.install_service('controller')
        self.assertEqual(run.call_count, 10)
        self.assertLess(sum(call.args[0] for call in sleep.call_args_list), 10)

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

    def test_pre_draft_journal_rows_replay_after_schema_upgrade(self):
        row = self.issue(self.main)
        row.pop('draft')
        row.pop('plan')
        row['body'] = 'Durable pre-upgrade journal'
        with self.agent:
            fleet.apply_pull(self.agent, 'agent', {'changes': [{'seq': 100, 'table_name': 'issues', 'before_json': None, 'after_json': fleet.encode(row)}], 'cursor': 100, 'allocations': [], 'ranges': []}, [])
        updated = self.issue(self.agent)
        self.assertEqual(updated['body'], row['body'])
        self.assertEqual(updated['draft'], 0)
        self.assertIsNone(updated['plan'])
        settings = {'project_id': PROJECT, 'prompt': 'Instructions', 'prs_enabled': 0, 'version': 1, 'boss_name': 'Boss'}
        with self.agent:
            fleet.put_row(self.agent, 'project_settings', settings)
        saved = self.agent.execute('SELECT drafts_enabled,plan_template FROM project_settings WHERE project_id=?', (PROJECT,)).fetchone()
        self.assertEqual(tuple(saved), (1, 'plans/{timestamp}-{number}.md'))

    def test_legacy_row_upgrade_still_rejects_missing_required_fields(self):
        row = self.issue(self.main)
        row.pop('draft')
        row.pop('plan')
        row.pop('body')
        with self.assertRaisesRegex(ValueError, 'Schema mismatch'):
            fleet.put_row(self.agent, 'issues', row)

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
        self.edit(self.main, title='Supervisor title')
        _, receipts = self.upload()
        self.assertEqual(receipts[0]['state'], 'applied')
        self.assertEqual(self.issue(self.main)['title'], 'Supervisor title')
        self.assertEqual(self.issue(self.main)['body'], 'Agent body')

    def test_same_field_conflict_is_retained_and_canonical_preserved(self):
        self.edit(self.agent, title='Agent title')
        self.edit(self.main, title='Supervisor title')
        _, receipts = self.upload()
        self.assertEqual(receipts[0]['state'], 'conflict')
        self.assertEqual(self.issue(self.main)['title'], 'Supervisor title')
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

    def test_comment_resolution_remaps_foreign_comment_ids_and_syncs_both_ways(self):
        def cli(path, *args):
            environment = {**os.environ, 'HEY_BOSS_ISSUE_DB': str(path)}
            environment.pop('HEY_BOSS_ISSUE_HOST', None)
            result = subprocess.run([str(BINARY), 'issue', '--project', 'Fleet tests', '--agent', 'human:fixture', '--json', *args], cwd=self.root, env=environment, capture_output=True, check=True)
            return json.loads(result.stdout)

        # Reserve local ID 1 independently on both machines before syncing.
        foreign_id = cli(self.main_path, 'comment', '1', '--body', 'Supervisor feedback')['comment_id']
        cli(self.agent_path, 'comment', '1', '--body', 'Agent note')
        _, initial_receipts = self.upload()
        with self.main:
            snapshot = fleet.export_snapshot(self.main, 'agent')
        with self.agent:
            fleet.apply_pull(self.agent, 'agent', snapshot, initial_receipts)
        local_id = self.agent.execute("SELECT id FROM comments WHERE body='Supervisor feedback'").fetchone()[0]
        self.assertNotEqual(local_id, foreign_id)
        cli(self.agent_path, 'resolve-comment', '1', str(local_id))
        changes, receipts = self.upload()
        self.assertTrue(all(receipt['state'] == 'applied' for receipt in receipts), receipts)
        comments = cli(self.main_path, 'view', '1')['comments']
        self.assertTrue(next(c for c in comments if c['body'] == 'Supervisor feedback')['resolved'])
        self.assertFalse(next(c for c in comments if c['body'] == 'Agent note')['resolved'])
        # Acknowledgment loss must not duplicate the resolution event.
        with self.main:
            self.assertEqual(fleet.accept_changes(self.main, 'agent', changes), receipts)
            snapshot = fleet.export_snapshot(self.main, 'agent')
        with self.agent:
            fleet.apply_pull(self.agent, 'agent', snapshot, receipts)
        self.assertEqual(self.agent.execute("SELECT count(*) FROM events WHERE action='comment_resolved'").fetchone()[0], 1)
        cli(self.main_path, 'unresolve-comment', '1', str(foreign_id))
        with self.main:
            snapshot = fleet.export_snapshot(self.main, 'agent')
        with self.agent:
            fleet.apply_pull(self.agent, 'agent', snapshot, [])
        comments = cli(self.agent_path, 'view', '1')['comments']
        self.assertFalse(next(c for c in comments if c['id'] == local_id)['resolved'])

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

    def test_supervisor_restart_reconciles_interrupted_deployments(self):
        with self.main:
            fleet.state_set(self.main, 'machines', {'remote': {'host': 'remote', 'state': 'connected', 'deployment': 'updating'}})
        with mock.patch.object(fleet, 'STATE', self.root), mock.patch.object(fleet, 'worker_status', return_value=[]), mock.patch.object(fleet.subprocess, 'check_output', return_value='fixture build'):
            supervisor = fleet.Supervisor(self.main_path, 'main')
        self.assertEqual(supervisor.nodes['remote']['state'], 'disconnected')
        self.assertEqual(supervisor.nodes['remote']['deployment'], 'outdated')

    def test_remote_deployment_success_is_independent_of_local_upgrade_lock(self):
        supervisor = fleet.Supervisor.__new__(fleet.Supervisor)
        supervisor.deployment_lock = threading.Lock()
        supervisor.lock = threading.RLock()
        supervisor.source = None
        channel = mock.Mock()
        supervisor.connections = {'remote': channel}
        supervisor.update = mock.Mock()
        supervisor.event = mock.Mock()
        report = {'machines': [{'host': 'local', 'status': 'failed', 'error': 'Another upgrade is running'},
                               {'host': 'remote', 'status': 'updated'}]}
        result = subprocess.CompletedProcess([], 1, json.dumps(report), '')
        with mock.patch.object(fleet.subprocess, 'run', return_value=result):
            self.assertTrue(supervisor.deploy('remote'))
        channel.terminate.assert_called_once()
        supervisor.update.assert_called_with('remote', deployment='current', deployment_error=None)

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
            result = fleet.configure_companion(self.agent, {'controller': 'main', 'revision': 'invalid', 'workers': []})
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
        with mock.patch.object(fleet, 'worker_status', return_value=[worker]), mock.patch.object(fleet, 'control_worker'), mock.patch.object(fleet, 'start_worker', return_value=200) as start, mock.patch.object(fleet, 'STATE', self.root):
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

    def test_restart_replay_after_start_before_ack_preserves_new_worker(self):
        message = {'id': 'interrupted-signal', 'worker': 'worker', 'signal': 'restart'}
        with self.agent:
            self.agent.execute("INSERT INTO fleet_signals VALUES('interrupted-signal','local','worker','restart','starting',?,0)", (json.dumps({'prior_pid': 100}),))
        worker = {'id': 'worker', 'config': {}, 'pid': 200, 'active': 0}
        with mock.patch.object(fleet, 'worker_status', return_value=[worker]), mock.patch.object(fleet, 'control_worker') as control, mock.patch.object(fleet, 'start_worker', return_value=200) as start, mock.patch.object(fleet, 'STATE', self.root):
            receipt = fleet.apply_signal(self.agent, message)
        self.assertEqual(receipt['state'], 'acknowledged')
        start.assert_not_called()
        control.assert_not_called()

    def test_signal_id_cannot_be_reused_with_different_payload(self):
        with self.agent:
            self.agent.execute("INSERT INTO fleet_signals VALUES('reused','local','worker','restart','acknowledged','{}',0)")
        with mock.patch.object(fleet, 'STATE', self.root), self.assertRaisesRegex(ValueError, 'different payload'):
            fleet.apply_signal(self.agent, {'id': 'reused', 'worker': 'worker', 'signal': 'stop'})

    def test_failed_launch_is_not_acknowledged_and_retries_with_backoff(self):
        message = {'id': 'failed-launch', 'worker': 'worker', 'signal': 'restart'}
        worker = {'id': 'worker', 'config': {}, 'pid': None, 'active': 0}
        with mock.patch.object(fleet, 'STATE', self.root), mock.patch.object(fleet, 'worker_status', return_value=[worker]), mock.patch.object(fleet, 'control_worker'), mock.patch.object(fleet, 'start_worker', side_effect=RuntimeError('launch failed')) as start:
            with self.assertRaisesRegex(RuntimeError, 'launch failed'):
                fleet.apply_signal(self.agent, message)
            self.assertEqual(fleet.apply_signal(self.agent, message)['state'], 'pending')
            start.assert_called_once()
        row = self.agent.execute("SELECT state,result FROM fleet_signals WHERE id='failed-launch'").fetchone()
        self.assertEqual(row['state'], 'starting')
        self.assertEqual(json.loads(row['result'])['failures'], 1)

    def test_starting_replay_never_launches_over_prior_worker_or_agents(self):
        with self.agent:
            self.agent.execute("INSERT INTO fleet_signals VALUES('unsafe-replay','local','worker','restart','starting',?,0)", (json.dumps({'prior_pid': 100}),))
        for pid, active in [(100, 0), (None, 1)]:
            worker = {'id': 'worker', 'config': {}, 'pid': pid, 'active': active}
            with mock.patch.object(fleet, 'STATE', self.root), mock.patch.object(fleet, 'worker_status', return_value=[worker]), mock.patch.object(fleet, 'start_worker') as start:
                with self.assertRaisesRegex(RuntimeError, 'no duplicate'):
                    fleet.apply_signal_locked(self.agent, {'id': 'unsafe-replay', 'worker': 'worker', 'signal': 'restart'})
                start.assert_not_called()

    def test_reconciliation_does_not_steal_interrupted_restart(self):
        with self.agent:
            self.agent.execute("INSERT INTO fleet_signals VALUES('recovering','local','worker','restart','starting','{}',0)")
        with mock.patch.object(fleet, 'STATE', self.root), mock.patch.object(fleet, 'identity', return_value=('agent', self.agent_path)), mock.patch.object(fleet, 'worker_status', return_value=[]), mock.patch.object(fleet, 'start_worker') as start:
            fleet.reconcile_workers({'workers': self.workers})
            start.assert_not_called()

    def test_lifecycle_lock_excludes_another_process(self):
        lock_path = self.root / 'fleet-worker-control.lock'
        code = "import fcntl,sys; f=open(sys.argv[1],'a'); fcntl.flock(f,fcntl.LOCK_EX); print('locked',flush=True); sys.stdin.read()"
        process = subprocess.Popen([sys.executable, '-c', code, str(lock_path)], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
        try:
            self.assertEqual(process.stdout.readline().strip(), 'locked')
            with mock.patch.object(fleet, 'STATE', self.root), fleet.lifecycle_lock(wait=False) as acquired:
                self.assertFalse(acquired)
        finally:
            process.communicate('', timeout=5)
        with mock.patch.object(fleet, 'STATE', self.root), fleet.lifecycle_lock(wait=False) as acquired:
            self.assertTrue(acquired)

    def test_start_worker_requires_its_exact_registration(self):
        child = mock.Mock(pid=200)
        child.poll.return_value = None
        worker = {'id': 'worker', 'config': {'directory': str(self.root)}}
        registrations = [{'workers': [{'id': 'worker', 'pid': 100}]}, {'workers': [{'id': 'worker', 'pid': 200}]}]
        with mock.patch.object(fleet, 'STATE', self.root), mock.patch.object(fleet, 'ensure_worker'), mock.patch.object(fleet.subprocess, 'Popen', return_value=child), mock.patch.object(fleet, 'worker_overview', side_effect=registrations) as overview, mock.patch.object(fleet.time, 'sleep'):
            self.assertEqual(fleet.start_worker(worker), 200)
            self.assertEqual(overview.call_count, 2)
            child.terminate.assert_not_called()

    def test_start_worker_reports_child_exit(self):
        child = mock.Mock(pid=200)
        child.poll.return_value = 1
        worker = {'id': 'worker', 'config': {'directory': str(self.root)}}
        with mock.patch.object(fleet, 'STATE', self.root), mock.patch.object(fleet, 'ensure_worker'), mock.patch.object(fleet.subprocess, 'Popen', return_value=child), self.assertRaisesRegex(RuntimeError, 'exited during startup'):
            fleet.start_worker(worker)
        child.wait.assert_called_once()

    def test_start_worker_timeout_reaps_only_new_child(self):
        child = mock.Mock(pid=200)
        child.poll.return_value = None
        worker = {'id': 'worker', 'config': {'directory': str(self.root)}}
        with mock.patch.object(fleet, 'STATE', self.root), mock.patch.object(fleet, 'ensure_worker'), mock.patch.object(fleet.subprocess, 'Popen', return_value=child), mock.patch.object(fleet.time, 'monotonic', side_effect=[0, 16]), self.assertRaisesRegex(RuntimeError, 'did not register'):
            fleet.start_worker(worker)
        child.terminate.assert_called_once()
        child.wait.assert_called_once()

    def test_remote_restart_keeps_heartbeat_channel_responsive(self):
        code = "import importlib.util,sys,time; spec=importlib.util.spec_from_file_location('fleet',sys.argv[1]); f=importlib.util.module_from_spec(spec); spec.loader.exec_module(f); f.worker_status=lambda: []; f.apply_signal=lambda db,m: (time.sleep(2), {'id':m['id'],'state':'acknowledged'})[1]; f.companion_stdio()"
        environment = {**os.environ, 'HEY_BOSS_ISSUE_DB': str(self.agent_path), 'HEY_BOSS_FLEET_STATE': str(self.root / 'async-state'), 'HEY_BOSS_FLEET_BINARY': str(BINARY)}
        environment.pop('HEY_BOSS_ISSUE_HOST', None)
        process = subprocess.Popen([sys.executable, '-c', code, str(ROOT / 'tools/fleet_hey_boss.py')], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, env=environment)
        try:
            self.assertTrue(select.select([process.stdout], [], [], 10)[0])
            self.assertEqual(json.loads(process.stdout.readline())['kind'], 'hello')
            for message in [{'version': 1, 'kind': 'signal', 'id': 'slow-restart', 'worker': 'worker', 'signal': 'restart'}, {'version': 1, 'kind': 'ping'}]:
                process.stdin.write(json.dumps(message) + chr(10))
                process.stdin.flush()
            self.assertTrue(select.select([process.stdout], [], [], 1.5)[0], 'Restart blocked the heartbeat reader')
            self.assertEqual(json.loads(process.stdout.readline())['kind'], 'heartbeat')
            self.assertTrue(select.select([process.stdout], [], [], 5)[0])
            self.assertEqual(json.loads(process.stdout.readline())['signal']['state'], 'acknowledged')
        finally:
            _, errors = process.communicate('', timeout=10)
        self.assertEqual(process.returncode, 0, errors)

    def test_new_stop_supersedes_failed_restart_and_old_replay(self):
        with self.agent:
            self.agent.execute("INSERT INTO fleet_signals VALUES('old-restart','local','worker','restart','starting',?,0)", (json.dumps({'prior_pid': 100}),))
        worker = {'id': 'worker', 'config': {}, 'pid': None, 'active': 0}
        with mock.patch.object(fleet, 'STATE', self.root), mock.patch.object(fleet, 'worker_status', return_value=[worker]), mock.patch.object(fleet, 'control_worker'), mock.patch.object(fleet, 'start_worker') as start:
            receipt = fleet.apply_signal(self.agent, {'id': 'new-stop', 'worker': 'worker', 'signal': 'stop'})
            replay = fleet.apply_signal(self.agent, {'id': 'old-restart', 'worker': 'worker', 'signal': 'restart'})
        self.assertEqual(receipt['state'], 'acknowledged')
        self.assertEqual(replay['state'], 'superseded')
        start.assert_not_called()

    def test_duplicate_supervisor_signal_does_not_rewrite_newer_intent(self):
        saved = {'machines': {'local': {'workers': [{'id': 'worker', 'intent': 'running', 'config': {}}]}}}
        desired = self.root / 'desired.json'
        desired.write_text(json.dumps(saved))
        with self.main:
            self.main.execute("INSERT INTO fleet_signals VALUES('old-pause','local','worker','pause','acknowledged','{}',0)")
        app = fleet.Supervisor.__new__(fleet.Supervisor)
        app.path, app.lock = self.main_path, threading.RLock()
        with mock.patch.object(fleet, 'DESIRED', desired), mock.patch.object(fleet, 'inventory', return_value=[]):
            receipt = app.signal({'host': 'local', 'worker': 'worker', 'signal': 'pause', 'id': 'old-pause'})
        self.assertEqual(receipt['state'], 'acknowledged')
        self.assertEqual(json.loads(desired.read_text()), saved)

    def test_real_worker_restart_preserves_supervisor_and_stable_id(self):
        state = self.root / 'restart-state'
        config = self.root / 'inventory.json'
        config.write_text('{"ssh_hosts":[]}')
        environment = {**os.environ, 'HEY_BOSS_ISSUE_DB': str(self.root / 'restart.db'), 'HEY_BOSS_FLEET_STATE': str(state), 'HEY_BOSS_FLEET_CONFIG': str(config), 'HEY_BOSS_FLEET_DESIRED': str(self.root / 'restart-desired.json'), 'HEY_BOSS_CODEX': str(ROOT / 'tests/fixtures/codex-worker.py'), 'HEY_BOSS_TEST_CLI': str(BINARY)}
        environment.pop('HEY_BOSS_ISSUE_HOST', None)
        def command(*args):
            result = subprocess.run([str(BINARY), *args], env=environment, cwd=self.root, text=True, capture_output=True, timeout=20)
            self.assertEqual(result.returncode, 0, result.stderr)
            return json.loads(result.stdout)
        def until(predicate, timeout=45):
            deadline = time.monotonic() + timeout
            while time.monotonic() < deadline:
                result = predicate()
                if result:
                    return result
                time.sleep(.1)
            self.fail('Worker lifecycle operation timed out')
        (self.root / 'mode.txt').write_text('delay')
        command('issue', '--project', 'Worker fixture', '--agent', 'human:fixture', '--json', 'create', '--title', 'Unfinished restart work')
        worker = subprocess.Popen([str(BINARY), 'worker', '--project', 'Worker fixture', '--directory', str(self.root), '--json'], env=environment, cwd=self.root, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True)
        supervisor = None
        replacement = None
        identifier = None
        try:
            def registered():
                if worker.poll() is not None:
                    self.fail('Fixture worker failed: ' + worker.stderr.read())
                return [w for w in command('worker', '--json', 'status')['workers'] if w['pid']]
            value = until(registered)
            identifier = value[0]['id']
            old_pid = value[0]['pid']
            old_config = value[0]['config']
            self.assertEqual(old_pid, worker.pid)
            old_session = until(lambda: command('issue', '--project', 'Worker fixture', '--json', 'view', '1')['issue']['assignee'])
            old_agent_pid = command('worker', '--id', identifier, '--json', 'status')['runs'][0]['pid']
            supervisor = subprocess.Popen([str(BINARY), 'fleet', 'supervisor'], env=environment, cwd=self.root, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True)
            until(lambda: (state / 'fleet.sock').exists())
            signal = command('worker', '--json', 'restart', identifier)
            self.assertEqual(signal['state'], 'pending')
            def completed():
                overview = command('fleet', 'status')
                return next((s for s in overview['signals'] if s['id'] == signal['id'] and s['state'] == 'acknowledged'), None)
            until(completed)
            current = command('worker', '--json', 'status')['workers']
            replacement = next(w['pid'] for w in current if w['id'] == identifier)
            self.assertEqual(next(w['config'] for w in current if w['id'] == identifier), old_config)
            self.assertIsNotNone(replacement)
            self.assertNotEqual(replacement, old_pid)
            resumed = until(lambda: next((r for r in command('worker', '--id', identifier, '--json', 'status')['runs'] if r['finished_at'] is None and r['claimed_at'] is not None and r['pid'] != old_pid), None))
            self.assertEqual('codex:' + resumed['session_id'], old_session)
            self.assertNotEqual(resumed['pid'], old_agent_pid)
            self.assertEqual(command('issue', '--project', 'Worker fixture', '--json', 'view', '1')['issue']['assignee'], old_session)
            runs = command('worker', '--id', identifier, '--json', 'status')['runs']
            self.assertTrue(any(r['state'] == 'cancelled' and r['finished_at'] is not None for r in runs))
            self.assertIsNone(supervisor.poll(), 'Supervisor exited during worker restart')
            worker.wait(timeout=10)
            self.assertEqual(worker.returncode, 0, worker.stderr.read())
            with mock.patch.object(fleet, 'STATE', state):
                replay = fleet.local_request({'kind': 'signal', 'host': 'local', 'worker': identifier, 'signal': 'restart', 'id': signal['id']})
            self.assertEqual(replay['state'], 'acknowledged')
            self.assertEqual(next(w['pid'] for w in command('worker', '--json', 'status')['workers'] if w['id'] == identifier), replacement)
        finally:
            if supervisor and supervisor.poll() is None and identifier:
                try:
                    command('fleet', 'signal', 'local', identifier, 'stop')
                    until(lambda: not next(w['pid'] for w in command('worker', '--json', 'status')['workers'] if w['id'] == identifier), timeout=20)
                finally:
                    supervisor.terminate()
                    try:
                        supervisor.communicate(timeout=15)
                    except subprocess.TimeoutExpired:
                        supervisor.kill()
                        supervisor.communicate(timeout=5)
            if worker.poll() is None:
                worker.terminate()
                worker.communicate(timeout=10)
            else:
                worker.stderr.close()
            if replacement:
                # Only the isolated fixture worker, never a production registration.
                try:
                    os.kill(replacement, 15)
                except ProcessLookupError:
                    pass


if __name__ == '__main__':
    unittest.main()
