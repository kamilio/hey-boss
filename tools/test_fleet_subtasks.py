#!/usr/bin/env python3
"""Real SQLite graph replay, deferred canonical state, and offline child creation."""
import json
import os
import sqlite3
import subprocess
import unittest
from test_fleet_hey_boss import FleetTests, fleet, BINARY, PROJECT


class SubtaskFleetTests(unittest.TestCase):
    setUp = FleetTests.setUp
    tearDown = FleetTests.tearDown
    upload = FleetTests.upload
    issue = FleetTests.issue

    def command(self, path, *args, project="Fleet tests"):
        env = os.environ.copy()
        env['HEY_BOSS_ISSUE_DB'] = str(path)
        env.pop('HEY_BOSS_ISSUE_HOST', None)
        result = subprocess.run([str(BINARY), 'issue', '--project', project, '--agent', 'human:fixture', '--json', *args],
                                cwd=self.root, env=env, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        return json.loads(result.stdout)

    def pull(self, receipts=()):
        with self.main:
            payload = fleet.export_snapshot(self.main, 'agent')
        with self.agent:
            fleet.apply_pull(self.agent, 'agent', payload, receipts)
        return payload

    def seed(self, count=4):
        original = self.issue(self.main)
        with self.main:
            for number in range(2, count+1):
                fleet.put_row(self.main, 'issues', {**original, 'number':number,'sort_order':number,'title':f'Issue {number}'})
        self.pull()

    def edges(self, db):
        return [tuple(r) for r in db.execute('SELECT parent_number,child_number FROM issue_subtasks ORDER BY child_number')]

    def test_offline_atomic_child_restart_ack_replay(self):
        result = self.command(self.agent_path, 'subtask', 'create', '1', '--title', 'Offline child', '--body', '## Durable Markdown')
        child = result['issue']['number']
        self.agent.close()
        self.agent = fleet.connect_db(self.agent_path)
        changes, receipts = self.upload()
        self.assertTrue(all(r['state']=='applied' for r in receipts), receipts)
        self.assertEqual(self.edges(self.main), [(1, child)])
        with self.main:
            replay = fleet.accept_changes(self.main, 'agent', changes)
        self.assertEqual(receipts, replay)
        self.pull(replay)
        self.assertEqual(fleet.journal(self.agent), [])
        self.assertEqual(self.edges(self.agent), self.edges(self.main))
        self.assertEqual(self.main.execute('SELECT body FROM issues WHERE number=?',(child,)).fetchone()[0], '## Durable Markdown')
        self.assertEqual(self.main.execute("SELECT count(*) FROM events WHERE action='subtask_added'").fetchone()[0], 1)

    def test_competing_parent_restores_canonical_and_truthful_history(self):
        self.seed()
        self.command(self.agent_path,'subtask','add','1','3')
        self.command(self.main_path,'subtask','add','2','3')
        changes, receipts = self.upload()
        graph = [r for c,r in zip(changes,receipts) if c['table_name']=='issue_subtasks']
        self.assertEqual(graph[0]['state'],'conflict')
        self.assertEqual(graph[0]['canonical_subtask']['row']['parent_number'],2)
        self.assertEqual(self.main.execute("SELECT count(*) FROM events WHERE action='subtask_added' AND issue_number=1").fetchone()[0],0)
        self.pull(receipts)
        self.assertEqual(self.edges(self.agent),[(2,3)])
        self.assertEqual(self.agent.execute("SELECT count(*) FROM events WHERE action='subtask_change_conflict'").fetchone()[0],2)
        self.assertTrue(self.agent.execute('SELECT count(*) FROM fleet_conflicts').fetchone()[0]>=3)

    def test_incremental_reparent_replays_final_graph_without_transient_cycle(self):
        self.seed()
        self.command(self.main_path,'subtask','add','1','2');self.pull()
        cursor = fleet.state_get(self.agent,'cursor')
        self.command(self.main_path,'subtask','remove','1','2')
        self.command(self.main_path,'subtask','add','2','1')
        with self.main:
            payload=fleet.export_incremental(self.main,'agent',cursor)
        # Reverse only graph ordering to exercise final-state composition with valid delete/add keys.
        graph=[c for c in payload['changes'] if c['table_name']=='issue_subtasks']
        rest=[c for c in payload['changes'] if c['table_name']!='issue_subtasks']
        payload['changes']=rest+list(reversed(graph))
        with self.agent:fleet.apply_pull(self.agent,'agent',payload,[])
        self.assertEqual(self.edges(self.agent),[(2,1)])
        self.assertEqual(self.agent.execute('SELECT count(*) FROM fleet_deferred_subtasks').fetchone()[0],0)

    def test_canonical_graph_blocked_by_pending_cycle_is_durable_and_retried(self):
        self.seed()
        self.command(self.agent_path,'subtask','add','1','2')
        self.command(self.main_path,'subtask','add','2','1')
        self.pull()
        self.assertEqual(self.edges(self.agent),[(1,2)])
        # One canonical add and one canonical removal wait on local pending edits.
        self.assertEqual(self.agent.execute('SELECT count(*) FROM fleet_deferred_subtasks').fetchone()[0],2)
        self.assertEqual(self.agent.execute('SELECT count(*) FROM issue_pickup_ready WHERE number=2').fetchone()[0],0)
        self.agent.close();self.agent=fleet.connect_db(self.agent_path)
        _, receipts=self.upload()
        self.pull(receipts)
        self.assertEqual(self.edges(self.agent),[(2,1)])
        self.assertEqual(self.agent.execute('SELECT count(*) FROM fleet_deferred_subtasks').fetchone()[0],0)

    def test_full_parent_upsert_and_repeated_snapshot_are_safe(self):
        self.seed(101)
        with self.main:
            for child in range(2,102):self.main.execute('INSERT INTO issue_subtasks VALUES(?,1,?,0,?)',(PROJECT,child,'human:fixture'))
            edge=dict(self.main.execute('SELECT * FROM issue_subtasks WHERE child_number=2').fetchone())
            fleet.put_row(self.main,'issue_subtasks',edge)
        self.pull();self.pull()
        self.assertEqual(len(self.edges(self.agent)),100)
        self.assertEqual(self.agent.execute('SELECT count(*) FROM fleet_deferred_subtasks').fetchone()[0],0)

    def test_allocation_only_leaves_then_parent_after_completion(self):
        self.seed()
        self.command(self.main_path,'subtask','add','1','2')
        self.command(self.main_path,'subtask','add','2','3')
        with self.main:
            self.main.execute('DELETE FROM fleet_allocations')
            fleet.allocate(self.main,'agent',self.workers)
        self.assertEqual([r[0] for r in self.main.execute('SELECT issue_number FROM fleet_allocations ORDER BY issue_number')],[3,4])
        self.command(self.main_path,'close','3')
        with self.main:fleet.allocate(self.main,'agent',self.workers)
        self.assertIsNotNone(self.main.execute('SELECT 1 FROM fleet_allocations WHERE issue_number=2').fetchone())
        self.assertIsNone(self.main.execute('SELECT 1 FROM fleet_allocations WHERE issue_number=1').fetchone())
        self.command(self.main_path,'close','2')
        with self.main:fleet.allocate(self.main,'agent',self.workers)
        self.assertIsNotNone(self.main.execute('SELECT 1 FROM fleet_allocations WHERE issue_number=1').fetchone())

    def test_ack_projection_uses_current_graph_after_lost_ack(self):
        self.seed()
        self.command(self.agent_path,'subtask','add','1','3')
        changes,receipts=self.upload()
        self.command(self.main_path,'subtask','remove','1','3')
        self.command(self.main_path,'subtask','add','2','3')
        with self.main:replay=fleet.accept_changes(self.main,'agent',changes)
        relation=[r for r in replay if 'canonical_subtask' in r][0]
        self.assertEqual(relation['state'],'applied')
        self.assertEqual(relation['canonical_subtask']['row']['parent_number'],2)
        self.pull(replay)
        self.assertEqual(self.edges(self.agent),[(2,3)])

    def test_snapshot_removal_supersedes_never_applied_deferred_edge(self):
        self.seed()
        self.command(self.agent_path,'subtask','add','1','2')
        self.command(self.main_path,'subtask','add','2','1')
        self.pull()
        self.command(self.main_path,'subtask','remove','2','1')
        self.pull()
        self.assertEqual(self.agent.execute('SELECT count(*) FROM fleet_deferred_subtasks WHERE row_json IS NOT NULL').fetchone()[0],0)
        _,receipts=self.upload();self.pull(receipts)
        self.assertEqual(self.edges(self.agent),[(1,2)])

    def test_offline_number_collision_never_links_unrelated_canonical_issue(self):
        created=self.command(self.agent_path,'subtask','create','1','--title','Offline child')
        child=created['issue']['number']
        original=self.issue(self.main)
        with self.main:fleet.put_row(self.main,'issues',{**original,'number':child,'sort_order':child,'title':'Unrelated canonical issue'})
        changes,receipts=self.upload()
        graph=[r for c,r in zip(changes,receipts) if c['table_name']=='issue_subtasks'][0]
        self.assertEqual(graph['state'],'conflict')
        self.assertEqual(self.edges(self.main),[])
        self.pull(receipts)
        self.assertEqual(self.edges(self.agent),[])
        self.assertIn('Offline child',self.agent.execute("SELECT data FROM fleet_conflicts WHERE table_name='issues'").fetchone()[0])

    def test_deferred_descendant_blocks_all_ancestors(self):
        self.seed()
        self.command(self.main_path,'subtask','add','1','2')
        self.command(self.main_path,'close','2')
        self.pull()
        with self.agent:self.agent.execute('INSERT INTO fleet_deferred_subtasks VALUES(?,?,?)',(PROJECT,3,json.dumps({'project_id':PROJECT,'parent_number':2,'child_number':3,'created_at':0,'created_by':'human:fixture'})))
        self.assertIsNone(self.agent.execute('SELECT 1 FROM issue_pickup_ready WHERE number=1').fetchone())

    def test_legacy_bootstrap_keeps_graph_prs_and_truthful_history(self):
        project='Legacy graph'
        self.command(self.main_path,'whoami',project=project);self.pull()
        with self.agent:self.agent.execute("UPDATE fleet_meta SET role='standalone' WHERE id=1")
        self.command(self.agent_path,'create','--title','Existing legacy parent',project=project)
        child=self.command(self.agent_path,'subtask','create','1','--title','Existing child',project=project)['issue']['number']
        self.command(self.agent_path,'pr','add',str(child),'https://github.com/example/hey-boss/pull/42',project=project)
        with self.agent:
            self.agent.execute("UPDATE fleet_meta SET role='agent' WHERE id=1")
            self.agent.execute('DELETE FROM fleet_outbox')
            fleet.bootstrap_rows(self.agent)
        changes,receipts=self.upload()
        self.assertTrue(all(r['state']=='applied' for r in receipts),receipts)
        self.assertEqual(self.edges(self.main),[(1,child)])
        self.assertEqual(self.main.execute('SELECT count(*) FROM issue_pull_requests WHERE issue_number=?',(child,)).fetchone()[0],1)
        self.assertEqual(self.main.execute("SELECT count(*) FROM events WHERE action='subtask_added'").fetchone()[0],1)
        self.pull(receipts)
        self.assertEqual(self.edges(self.agent),[(1,child)])

    def test_approval_held_allocations_refill_ready_work(self):
        self.seed()
        with self.main:
            fleet.allocate(self.main,'agent',self.workers)
            for number in (1,2):
                self.main.execute("INSERT INTO worker_runs(id,project_id,issue_number,job,actor_id,state,owner_pid,owner_start,machine,started_at,updated_at,finished_at,summary) VALUES(?,?,?,'{}','human:fixture','blocked',0,'none','synthetic',0,0,1,'Codex needs input or approval: fixture')",('approval-'+str(number),PROJECT,number))
            fleet.allocate(self.main,'agent',self.workers)
        self.assertEqual([r[0] for r in self.main.execute('SELECT issue_number FROM fleet_allocations ORDER BY issue_number')],[1,2,3,4])

if __name__=='__main__':unittest.main()
