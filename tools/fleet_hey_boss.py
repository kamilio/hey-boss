#!/usr/bin/env python3
"""Owner-authenticated fleet control, durable SQLite replicas, and offline workers."""
import argparse
import contextlib
import fcntl
import hashlib
import json
import os
import pathlib
import queue
import shlex
import signal
import socket
import socketserver
import sqlite3
import subprocess
import sys
import threading
import time
import uuid
import urllib.request
import urllib.parse

VERSION = 1
# Compatibility boundary: keep protocol-v1 roles, service labels, filenames and
# locks stable during rolling upgrades. User-facing terminology is Supervisor.
SUPERVISOR_ROLE = 'controller'
COMPANION_ROLE = 'agent'
LIMIT = 16 * 1024 * 1024
TABLES = {'projects': ['id'], 'agents': ['id'], 'issues': ['project_id', 'number'],
          'issue_subtasks': ['project_id', 'child_number'], 'comments': ['id'], 'events': ['id'], 'project_settings': ['project_id'],
          'global_settings': ['id'], 'issue_pull_requests': ['project_id', 'issue_number', 'url']}
APPEND = {'comments', 'events'}
READY_WORK_SQL = "i.draft=0 AND EXISTS(SELECT 1 FROM issue_pickup_ready ready WHERE ready.project_id=i.project_id AND ready.number=i.number)"
ALLOCATED_WORK_SQL = "(EXISTS(SELECT 1 FROM worker_runs r WHERE r.project_id=i.project_id AND r.issue_number=i.number AND r.finished_at IS NULL) OR (" + READY_WORK_SQL + "))"
STATE = pathlib.Path(os.environ.get('HEY_BOSS_FLEET_STATE', pathlib.Path.home() / '.local/share/hey-boss'))
DESIRED = pathlib.Path(os.environ.get('HEY_BOSS_FLEET_DESIRED', pathlib.Path.home() / '.hey-boss/fleet.json'))
BINARY = pathlib.Path(os.environ.get('HEY_BOSS_FLEET_BINARY', sys.argv[0])).resolve()
STOP = threading.Event()


def encode(value):
    return json.dumps(value, separators=(',', ':'), ensure_ascii=False)


def atomic_json(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(path.name + '.' + uuid.uuid4().hex + '.new')
    fd = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    try:
        with os.fdopen(fd, 'w') as output:
            output.write(encode(value))
            output.flush()
            os.fsync(output.fileno())
        os.replace(temporary, path)
    finally:
        temporary.unlink(missing_ok=True)


def read_json(path, default=None):
    try:
        return json.loads(path.read_text())
    except FileNotFoundError:
        return default


def valid_host(host):
    return isinstance(host, str) and 0 < len(host) <= 253 and not host.startswith('-') and all(c.isalnum() or c in '.-_@' for c in host)


def cli(args, payload=None, timeout=20):
    environment = os.environ.copy()
    environment.pop('HEY_BOSS_ISSUE_HOST', None)
    result = subprocess.run([str(BINARY), *args], input=None if payload is None else encode(payload),
                            text=True, capture_output=True, env=environment, timeout=timeout,
                            cwd=str(pathlib.Path.home()))
    try:
        value = json.loads(result.stdout)
    except ValueError:
        raise RuntimeError(result.stderr[-2000:] or 'Invalid CLI response')
    if result.returncode or value.get('ok') is False:
        raise RuntimeError(encode(value.get('error', value)))
    return value


def identity():
    value = cli(['issue', '--project', 'Fleet', '--agent', 'human:fleet', '--json', 'whoami'])
    return value['agent']['machine'], pathlib.Path(value['store']['database'])


def local_actor():
    return cli(['issue', '--project', 'Fleet', '--agent', 'human:fleet', '--json', 'whoami'])['agent']


class Row:
    def __init__(self, columns, values):
        self.columns, self.values = columns, values
    def __getitem__(self, key):
        return self.values[key] if isinstance(key, (int, slice)) else self.values[self.columns.index(key)]
    def __iter__(self):
        return iter(self.values)
    def keys(self):
        return self.columns


class Cursor:
    def __init__(self, value):
        self.rows = [Row(value['columns'], row) for row in value['rows']]
        self.index = 0
    def fetchone(self):
        if self.index >= len(self.rows):
            return None
        row = self.rows[self.index]
        self.index += 1
        return row
    def fetchall(self):
        rows = self.rows[self.index:]
        self.index = len(self.rows)
        return rows
    def __iter__(self):
        return iter(self.fetchall())


class Database:
    """Keep all fleet writes on the same patched SQLite as native issue writes."""
    def __init__(self, path):
        self.path = pathlib.Path(path)
        self.in_transaction = False
        self.process = subprocess.Popen([str(BINARY), 'fleet', 'database', '--path', str(path)],
                                        stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                        stderr=subprocess.PIPE, text=True)
        version = self.execute('SELECT sqlite_version()').fetchone()[0]
        if tuple(map(int, version.split('.'))) < (3, 51, 3):
            self.close()
            raise RuntimeError('Fleet requires bundled SQLite 3.51.3 or later')

    def request(self, value):
        self.process.stdin.write(encode(value) + '\n')
        self.process.stdin.flush()
        raw = self.process.stdout.readline(LIMIT + 1)
        if not raw:
            raise sqlite3.DatabaseError(self.process.stderr.read()[-2000:] or 'Database driver exited')
        if len(raw.encode()) > LIMIT:
            raise sqlite3.DatabaseError('Database response exceeds frame limit')
        result = json.loads(raw)
        self.in_transaction = result['transaction']
        if not result['ok']:
            error = sqlite3.IntegrityError if result.get('constraint') else sqlite3.DatabaseError
            raise error(result['error'])
        return Cursor(result)

    def execute(self, sql, args=()):
        if not self.in_transaction and sql.lstrip().split(None, 1)[0].upper() in ('INSERT', 'UPDATE', 'DELETE', 'REPLACE'):
            self.request({'sql': 'BEGIN', 'args': []})
        return self.request({'sql': sql, 'args': list(args)})

    def executemany(self, sql, rows):
        for row in rows:
            self.execute(sql, row)

    def commit(self):
        if self.in_transaction:
            self.execute('COMMIT')

    def rollback(self):
        if self.in_transaction:
            self.execute('ROLLBACK')

    def backup(self, destination):
        self.request({'backup': str(destination.path)})

    def backup_path(self, path):
        self.request({'backup': str(path)})

    def close(self):
        process = getattr(self, 'process', None)
        if process and process.stdin and not process.stdin.closed:
            process.stdin.close()
            try:
                process.wait(timeout=2)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()
            process.stdout.close()
            process.stderr.close()

    def __enter__(self):
        return self
    def __exit__(self, kind, *_):
        self.rollback() if kind else self.commit()
    def __del__(self):
        self.close()


def connect_db(path):
    return Database(path)


def install_capture(db, role, node):
    """Journal rows inside the native transaction; all writers use these triggers."""
    db.execute('UPDATE fleet_meta SET role=?,node=? WHERE id=1', (role, node))
    if any(r['origin'] == 'u' for r in db.execute('PRAGMA index_list(fleet_row_ids)')):
        db.execute('ALTER TABLE fleet_row_ids RENAME TO fleet_row_ids_legacy')
        db.execute('CREATE TABLE fleet_row_ids(origin TEXT NOT NULL,table_name TEXT NOT NULL,origin_id INTEGER NOT NULL,local_id INTEGER NOT NULL,PRIMARY KEY(origin,table_name,origin_id))')
        db.execute('INSERT INTO fleet_row_ids SELECT * FROM fleet_row_ids_legacy')
        db.execute('DROP TABLE fleet_row_ids_legacy')
    if role == SUPERVISOR_ROLE:
        for table in APPEND:
            db.execute('INSERT OR IGNORE INTO fleet_row_ids SELECT ?,?,id,id FROM ' + table, (node, table))
    for table in TABLES:
        columns = [r['name'] for r in db.execute('PRAGMA table_info(' + table + ')')]
        for operation, before, after in [('INSERT', None, 'NEW'), ('UPDATE', 'OLD', 'NEW'), ('DELETE', 'OLD', None)]:
            def row_json(prefix):
                return 'NULL' if prefix is None else 'json_object(' + ','.join("'" + c + "'," + prefix + '."' + c + '"' for c in columns) + ')'
            different = '' if operation != 'UPDATE' else ' AND NOT (' + ' AND '.join('OLD."' + c + '" IS NEW."' + c + '"' for c in columns) + ')'
            db.execute('CREATE TRIGGER IF NOT EXISTS fleet_capture_' + table + '_' + operation +
                       ' AFTER ' + operation + ' ON ' + table +
                       " WHEN (SELECT syncing FROM fleet_meta WHERE id=1)=0" + different +
                       " BEGIN INSERT INTO fleet_outbox(table_name,before_json,after_json,created_at) VALUES('" + table + "'," + row_json(before) + ',' + row_json(after) + ",CAST(strftime('%s','now') AS INTEGER)*1000); END")
    db.execute('CREATE TABLE IF NOT EXISTS fleet_ranges(node TEXT NOT NULL,project_id TEXT NOT NULL,first_number INTEGER NOT NULL,last_number INTEGER NOT NULL,PRIMARY KEY(node,project_id))')
    db.execute('CREATE TABLE IF NOT EXISTS fleet_number_reservations(node TEXT NOT NULL,project_id TEXT NOT NULL,first_number INTEGER NOT NULL,last_number INTEGER NOT NULL,PRIMARY KEY(node,project_id,first_number))')
    db.execute('CREATE TABLE IF NOT EXISTS fleet_signals(id TEXT PRIMARY KEY,host TEXT NOT NULL,worker TEXT NOT NULL,signal TEXT NOT NULL,state TEXT NOT NULL,result TEXT,created_at REAL NOT NULL)')
    db.execute('CREATE TABLE IF NOT EXISTS fleet_state(key TEXT PRIMARY KEY,value TEXT NOT NULL)')
    db.commit()


def state_get(db, key, default=None):
    row = db.execute('SELECT value FROM fleet_state WHERE key=?', (key,)).fetchone()
    return json.loads(row[0]) if row else default


def state_set(db, key, value):
    db.execute('INSERT INTO fleet_state VALUES(?,?) ON CONFLICT(key) DO UPDATE SET value=excluded.value', (key, encode(value)))


def journal(db, after=0):
    results, size = [], 0
    bootstrap = state_get(db, 'bootstrap_last_seq', 0)
    for row in db.execute('SELECT * FROM fleet_outbox WHERE seq>? ORDER BY seq LIMIT 300', (after,)):
        row = dict(row)
        if row['seq'] <= bootstrap:
            row['bootstrap'] = True
        size += len(encode(row).encode())
        if results and size > LIMIT // 3:
            break
        results.append(row)
    return results


def key_where(table, row):
    return ' AND '.join('"' + k + '"=?' for k in TABLES[table]), [row[k] for k in TABLES[table]]


def current_row(db, table, row):
    where, values = key_where(table, row)
    value = db.execute('SELECT * FROM ' + table + ' WHERE ' + where, values).fetchone()
    return dict(value) if value else None


def put_row(db, table, row):
    # Durable journals captured before schema 11 still replay after upgrade.
    # Supply only the native migration defaults; unknown/missing fields stay errors.
    defaults = {'issues': {'draft': 0, 'plan': None},
                'project_settings': {'drafts_enabled': 1, 'plan_template': 'plans/{timestamp}-{number}.md'}}
    row = {**defaults.get(table, {}), **row}
    columns = [r['name'] for r in db.execute('PRAGMA table_info(' + table + ')')]
    if set(row) != set(columns):
        raise ValueError('Schema mismatch for ' + table)
    names = ','.join('"' + c + '"' for c in columns)
    updates = ','.join('"' + c + '"=excluded."' + c + '"' for c in columns if c not in TABLES[table])
    db.execute('INSERT INTO ' + table + '(' + names + ') VALUES(' + ','.join('?' for _ in columns) + ') ON CONFLICT(' + ','.join(TABLES[table]) + ') DO UPDATE SET ' + updates,
               [row[c] for c in columns])


def append_row(db, origin, table, row, bootstrap=False):
    origin_id = row['id']
    mapped = db.execute('SELECT local_id FROM fleet_row_ids WHERE origin=? AND table_name=? AND origin_id=?', (origin, table, origin_id)).fetchone()
    if mapped:
        return mapped[0]
    own_node = db.execute('SELECT node FROM fleet_meta WHERE id=1').fetchone()[0]
    if origin == own_node:
        # An agent's own append already exists before it receives its echo.
        existing = db.execute('SELECT id FROM ' + table + ' WHERE id=?', (origin_id,)).fetchone()
        if existing:
            local_id = origin_id
        else:
            raise ValueError('Missing original append')
    else:
        values = {k: v for k, v in row.items() if k != 'id'}
        if table == 'events':
            data = json.loads(values['data'])
            if isinstance(data, dict) and 'comment_id' in data:
                comment_origin = data.get('comment_origin') or origin
                comment_id = data.get('comment_origin_id', data['comment_id'])
                comment = db.execute('SELECT local_id FROM fleet_row_ids WHERE origin=? AND table_name=? AND origin_id=?', (comment_origin, 'comments', comment_id)).fetchone()
                if not comment and comment_origin == own_node:
                    comment = db.execute('SELECT id FROM comments WHERE id=? AND project_id=? AND issue_number=?', (comment_id, values['project_id'], values['issue_number'])).fetchone()
                if not comment and values['action'] in ('comment_resolved', 'comment_unresolved'):
                    raise ValueError('Resolved comment is not available on this replica')
                if comment:
                    data['comment_id'] = comment[0]
                    values['data'] = encode(data)
        columns = list(values)
        existing = db.execute('SELECT id FROM ' + table + ' WHERE ' + ' AND '.join(k + ' IS ?' for k in columns), list(values.values())).fetchone() if bootstrap else None
        if existing:
            local_id = existing[0]
        else:
            db.execute('INSERT INTO ' + table + '(' + ','.join(columns) + ') VALUES(' + ','.join('?' for _ in columns) + ')', list(values.values()))
            local_id = db.execute('SELECT last_insert_rowid()').fetchone()[0]
    db.execute('INSERT OR IGNORE INTO fleet_row_ids VALUES(?,?,?,?)', (origin, table, origin_id, local_id))
    return local_id


def conflict(db, node, change, reason):
    identifier = node + ':' + str(change['seq'])
    db.execute('INSERT OR IGNORE INTO fleet_conflicts(id,node,seq,table_name,data,reason,created_at) VALUES(?,?,?,?,?,?,?)',
               (identifier, node, change['seq'], change['table_name'], encode(change), reason, int(time.time() * 1000)))
    return {'state': 'conflict', 'id': identifier, 'reason': reason}


def accept_changes(db, node, changes):
    """Field-aware merge with durable receipts and retained conflicting payloads."""
    results = []
    if not db.in_transaction:
        db.execute('BEGIN IMMEDIATE')
    for change in changes:
        sequence = change['seq']
        receipt = db.execute('SELECT result FROM fleet_receipts WHERE node=? AND seq=?', (node, sequence)).fetchone()
        if receipt:
            results.append({'seq': sequence, **json.loads(receipt[0])})
            if change['table_name'] == 'issue_subtasks':
                row = json.loads(change['after_json'] or change['before_json'])
                db.execute('INSERT OR IGNORE INTO fleet_subtask_receipts VALUES(?,?,?,?,?,?,?)',
                           (node, sequence, row['project_id'], row['child_number'], row['parent_number'], 'remove' if change['after_json'] is None else 'add', results[-1]['state']))
            continue
        table = change['table_name']
        if table not in TABLES:
            raise ValueError('Unknown replicated table')
        before = json.loads(change['before_json']) if change.get('before_json') else None
        after = json.loads(change['after_json']) if change.get('after_json') else None
        db.execute('SAVEPOINT incoming')
        try:
            old = current_row(db, table, after or before)
            if table == 'issue_subtasks' and after is not None:
                # A rejected offline creation must never accidentally link the
                # unrelated canonical issue that already owns its number.
                collision = db.execute("SELECT 1 FROM fleet_conflicts WHERE node=? AND table_name='issues' AND seq<? AND json_extract(data,'$.before_json') IS NULL AND json_extract(json_extract(data,'$.after_json'),'$.project_id')=? AND json_extract(json_extract(data,'$.after_json'),'$.number') IN (?,?)", (node, sequence, after['project_id'], after['parent_number'], after['child_number'])).fetchone()
                if collision:
                    raise ValueError('Subtask endpoint belongs to a rejected offline issue creation; retained for review')
            if table in APPEND:
                if before is not None or after is None:
                    raise ValueError('Append-only history cannot be rewritten')
                if table == 'events' and after['action'] in ('subtask_added', 'subtask_removed', 'parent_added', 'parent_removed'):
                    data = json.loads(after['data'])
                    latest = db.execute('SELECT * FROM fleet_subtask_receipts WHERE node=? AND project_id=? AND child_number=? AND seq<? ORDER BY seq DESC LIMIT 1',
                                        (node, after['project_id'], data['child'], sequence)).fetchone()
                    adding = after['action'] in ('subtask_added', 'parent_added')
                    relation = db.execute('SELECT parent_number FROM issue_subtasks WHERE project_id=? AND child_number=?', (after['project_id'], data['child'])).fetchone()
                    if latest and (latest['state'] != 'applied' or latest['parent_number'] != data['parent'] or latest['kind'] != ('add' if adding else 'remove')):
                        raise ValueError('Subtask relationship was rejected; attempted history retained for review')
                    if adding != bool(relation and relation[0] == data['parent']):
                        raise ValueError('Subtask history does not match the canonical relationship')
                if change.get('bootstrap') and db.execute("SELECT 1 FROM fleet_conflicts WHERE node=? AND table_name='issues' AND json_extract(data,'$.after_json') IS NOT NULL AND json_extract(json_extract(data,'$.after_json'),'$.project_id')=? AND json_extract(json_extract(data,'$.after_json'),'$.number')=?", (node, after['project_id'], after['issue_number'])).fetchone():
                    raise ValueError('Legacy history belongs to an issue-number collision; retained for review')
                local_id = append_row(db, node, table, after, change.get('bootstrap', False))
            elif table == 'projects':
                if old is None:
                    raise ValueError('New offline projects require registration before disconnection')
                # Queue order and numbering are supervisor-owned; activity may merge.
                db.execute('UPDATE projects SET activity_at=max(activity_at,?) WHERE id=?', (after['activity_at'], after['id']))
            elif table == 'agents':
                if after is not None:
                    put_row(db, table, after)
            elif table == 'issues':
                owner = db.execute('SELECT node FROM fleet_allocations WHERE project_id=? AND issue_number=?', (after['project_id'], after['number'])).fetchone() if after else None
                if before is None:
                    allocated = db.execute('SELECT 1 FROM fleet_number_reservations WHERE node=? AND project_id=? AND ? BETWEEN first_number AND last_number', (node, after['project_id'], after['number'])).fetchone()
                    legacy = change.get('bootstrap', False) and not db.execute('SELECT 1 FROM fleet_number_reservations WHERE node<>? AND project_id=? AND ? BETWEEN first_number AND last_number', (node, after['project_id'], after['number'])).fetchone()
                    if old == after:
                        pass
                    elif old is not None or not (allocated or legacy):
                        raise ValueError('Offline issue number is not exclusively allocated')
                    else:
                        put_row(db, table, after)
                        db.execute('UPDATE projects SET next_number=max(next_number,?) WHERE id=?', (after['number'] + 1, after['project_id']))
                        db.execute('INSERT OR IGNORE INTO fleet_allocations VALUES(?,?,?)', (after['project_id'], after['number'], node))
                else:
                    if old is None:
                        raise ValueError('Issue no longer exists')
                    if after is None:
                        raise ValueError('Physical issue deletion is not supported')
                    changes_to = {k: v for k, v in after.items() if before[k] != v and k not in ('version', 'updated_at', 'sort_order')}
                    if changes_to and (not owner or owner[0] != node):
                        raise ValueError('Issue allocation was revoked or belongs to another machine')
                    if after['state'] == 'closed' and before['state'] != 'closed' and any(old[k] != before[k] for k in ('title', 'body', 'labels')):
                        raise ValueError('Issue requirements changed before offline completion')
                    if any(old[k] != before[k] and old[k] != v for k, v in changes_to.items()):
                        raise ValueError('Concurrent edits changed the same issue field or ownership')
                    merged = {**old, **changes_to, 'version': old['version'] + 1, 'updated_at': max(old['updated_at'], after['updated_at'])}
                    put_row(db, table, merged)
            else:
                if before is not None and old != before and old != after:
                    raise ValueError('Concurrent configuration or PR change')
                if before is None and old is not None and old != after:
                    raise ValueError('Conflicting inserted row')
                if after is None:
                    where, values = key_where(table, before)
                    db.execute('DELETE FROM ' + table + ' WHERE ' + where, values)
                else:
                    put_row(db, table, after)
            result = {'state': 'applied'}
            if table in APPEND:
                original = db.execute('SELECT origin,origin_id FROM fleet_row_ids WHERE table_name=? AND local_id=? ORDER BY rowid LIMIT 1', (table, local_id)).fetchone()
                result['canonical_append'] = {'origin': original['origin'], 'origin_id': original['origin_id']}
            db.execute('RELEASE incoming')
        except (ValueError, sqlite3.IntegrityError) as error:
            db.execute('ROLLBACK TO incoming')
            db.execute('RELEASE incoming')
            result = conflict(db, node, change, str(error))
        db.execute('INSERT INTO fleet_receipts VALUES(?,?,?)', (node, sequence, encode(result)))
        if table == 'issue_subtasks':
            row = after or before
            db.execute('INSERT OR IGNORE INTO fleet_subtask_receipts VALUES(?,?,?,?,?,?,?)',
                       (node, sequence, row['project_id'], row['child_number'], row['parent_number'], 'remove' if after is None else 'add', result['state']))
        results.append({'seq': sequence, **result})
    # Receipts retain the original outcome; this projection reports current
    # canonical relationships even when an acknowledgement is replayed later.
    for change, result in zip(changes, results):
        if change['table_name'] == 'issue_subtasks':
            row = json.loads(change['after_json'] or change['before_json'])
            result['canonical_subtask'] = {'project_id': row['project_id'], 'child_number': row['child_number'],
                                           'row': current_row(db, 'issue_subtasks', row)}
    return results


def canonical_append(db, supervisor_node, table, row):
    origin = db.execute('SELECT origin,origin_id FROM fleet_row_ids WHERE table_name=? AND local_id=?', (table, row['id'])).fetchone()
    row = dict(row)
    if origin:
        row['id'] = origin[1]
        if table == 'events':
            data = json.loads(row['data'])
            if isinstance(data, dict) and 'comment_id' in data:
                comment = db.execute("SELECT origin,origin_id FROM fleet_row_ids WHERE table_name='comments' AND local_id=?", (data['comment_id'],)).fetchone()
                resolution = row['action'] in ('comment_resolved', 'comment_unresolved')
                if comment and (resolution or comment['origin'] == origin['origin']):
                    data['comment_id'] = comment['origin_id']
                    if resolution:
                        data['comment_origin'] = comment['origin']
                        data['comment_origin_id'] = comment['origin_id']
                    row['data'] = encode(data)
        return {'origin': origin[0], 'row': row}
    return {'origin': supervisor_node, 'row': row}


def export_snapshot(db, node):
    if not db.in_transaction:
        db.execute('BEGIN IMMEDIATE')
    supervisor_node = db.execute('SELECT node FROM fleet_meta WHERE id=1').fetchone()[0]
    tables = {}
    for table in TABLES:
        rows = [dict(r) for r in db.execute('SELECT * FROM ' + table)]
        tables[table] = [canonical_append(db, supervisor_node, table, r) for r in rows] if table in APPEND else rows
    return {'tables': tables, 'cursor': db.execute('SELECT coalesce(max(seq),0) FROM fleet_outbox').fetchone()[0],
            'allocations': [dict(r) for r in db.execute('SELECT * FROM fleet_allocations')],
            'ranges': [dict(r) for r in db.execute('SELECT project_id,first_number,last_number FROM fleet_ranges WHERE node=?', (node,))]}


def export_incremental(db, node, cursor):
    own = db.execute('SELECT node FROM fleet_meta WHERE id=1').fetchone()[0]
    changes = journal(db, cursor)
    for change in changes:
        if change['table_name'] in APPEND and change['after_json']:
            change['append'] = canonical_append(db, own, change['table_name'], json.loads(change['after_json']))
    return {'changes': changes, 'cursor': changes[-1]['seq'] if changes else cursor,
            'allocations': [dict(r) for r in db.execute('SELECT * FROM fleet_allocations')],
            'ranges': [dict(r) for r in db.execute('SELECT project_id,first_number,last_number FROM fleet_ranges WHERE node=?', (node,))]}


def apply_subtask_graph(db, pending, payload, acknowledged):
    """Compose a final graph, retaining canonical rows blocked by offline edits."""
    desired = {(r['project_id'], r['child_number']): json.loads(r['row_json']) if r['row_json'] else None
               for r in db.execute('SELECT * FROM fleet_deferred_subtasks')}
    snapshot = payload.get('tables', {}).get('issue_subtasks')
    if snapshot is not None:
        current = db.execute('SELECT project_id,child_number FROM issue_subtasks').fetchall()
        # A full snapshot supersedes even canonical edges never installed locally.
        desired = {key: None for key in desired}
        desired.update({(r['project_id'], r['child_number']): None for r in current})
        desired.update({(r['project_id'], r['child_number']): r for r in snapshot})
    for change in payload.get('changes', []):
        if change['table_name'] == 'issue_subtasks':
            row = json.loads(change['after_json'] or change['before_json'])
            desired[(row['project_id'], row['child_number'])] = json.loads(change['after_json']) if change['after_json'] else None
    desired.update(acknowledged)
    # Delete all replaceable edges before insertion; replay order must not
    # manufacture a temporary cycle from two individually valid graphs.
    for (project, child), row in desired.items():
        if ('issue_subtasks', (project, child)) not in pending:
            db.execute('DELETE FROM issue_subtasks WHERE project_id=? AND child_number=?', (project, child))
    for (project, child), row in desired.items():
        if ('issue_subtasks', (project, child)) in pending:
            db.execute('INSERT INTO fleet_deferred_subtasks VALUES(?,?,?) ON CONFLICT(project_id,child_number) DO UPDATE SET row_json=excluded.row_json',
                       (project, child, encode(row) if row else None))
            continue
        db.execute('SAVEPOINT subtask_pull')
        try:
            if row is not None:
                put_row(db, 'issue_subtasks', row)
            db.execute('RELEASE subtask_pull')
            db.execute('DELETE FROM fleet_deferred_subtasks WHERE project_id=? AND child_number=?', (project, child))
        except sqlite3.IntegrityError:
            db.execute('ROLLBACK TO subtask_pull')
            db.execute('RELEASE subtask_pull')
            db.execute('INSERT INTO fleet_deferred_subtasks VALUES(?,?,?) ON CONFLICT(project_id,child_number) DO UPDATE SET row_json=excluded.row_json',
                       (project, child, encode(row) if row else None))


def apply_pull(db, node, payload, receipts):
    db.execute('UPDATE fleet_meta SET syncing=1 WHERE id=1')
    try:
        acknowledged = {}
        for receipt in receipts:
            if 'canonical_subtask' in receipt:
                row = receipt['canonical_subtask']
                acknowledged[(row['project_id'], row['child_number'])] = row['row']
            if 'canonical_append' in receipt:
                change = db.execute('SELECT * FROM fleet_outbox WHERE seq=?', (receipt['seq'],)).fetchone()
                if change:
                    local = json.loads(change['after_json'])
                    origin = receipt['canonical_append']
                    db.execute('INSERT OR IGNORE INTO fleet_row_ids VALUES(?,?,?,?)', (origin['origin'], change['table_name'], origin['origin_id'], local['id']))
            if receipt['state'] == 'conflict':
                change = db.execute('SELECT * FROM fleet_outbox WHERE seq=?', (receipt['seq'],)).fetchone()
                if change:
                    conflict(db, node, dict(change), receipt['reason'])
                    if change['table_name'] == 'events' and change['after_json']:
                        event = json.loads(change['after_json'])
                        if event['action'] in ('subtask_added', 'subtask_removed', 'parent_added', 'parent_removed'):
                            data = json.loads(event['data'])
                            data.update(sync_conflict=receipt['reason'], attempted_action=event['action'])
                            db.execute("UPDATE events SET action='subtask_change_conflict',data=? WHERE id=?", (encode(data), event['id']))
            db.execute('DELETE FROM fleet_outbox WHERE seq=?', (receipt['seq'],))
        # Never replace a domain row with locally pending edits.
        pending = set()
        for change in db.execute('SELECT table_name,before_json,after_json FROM fleet_outbox'):
            table, before, after = change
            row = json.loads(after or before)
            pending.add((table, tuple(row[k] for k in TABLES[table])))

        def apply(table, row, origin=None):
            if table == 'issue_subtasks':
                return  # Applied as one graph after its endpoint rows exist.
            if (table, tuple(row[k] for k in TABLES[table])) in pending:
                return
            if table in APPEND:
                append_row(db, origin, table, row)
            elif table == 'projects':
                old = current_row(db, table, row)
                if old:
                    row = {**row, 'next_number': old['next_number']}
                put_row(db, table, row)
            else:
                put_row(db, table, row)

        for table, rows in payload.get('tables', {}).items():
            for row in rows:
                if table in APPEND:
                    apply(table, row['row'], row['origin'])
                else:
                    apply(table, row)
        for change in payload.get('changes', []):
            table = change['table_name']
            if table in APPEND:
                if change.get('append'):
                    apply(table, change['append']['row'], change['append']['origin'])
            elif change['after_json']:
                apply(table, json.loads(change['after_json']))
            elif change['before_json']:
                row = json.loads(change['before_json'])
                if table != 'issue_subtasks' and (table, tuple(row[k] for k in TABLES[table])) not in pending:
                    where, values = key_where(table, row)
                    db.execute('DELETE FROM ' + table + ' WHERE ' + where, values)
        apply_subtask_graph(db, pending, payload, acknowledged)
        db.execute('DELETE FROM fleet_allocations')
        db.executemany('INSERT INTO fleet_allocations VALUES(?,?,?)', [(r['project_id'], r['issue_number'], r['node']) for r in payload['allocations']])
        for number_range in payload['ranges']:
            previous = db.execute('SELECT first_number,last_number FROM fleet_number_ranges WHERE project_id=?', (number_range['project_id'],)).fetchone()
            db.execute('INSERT INTO fleet_number_ranges VALUES(?,?,?) ON CONFLICT(project_id) DO UPDATE SET first_number=excluded.first_number,last_number=excluded.last_number', tuple(number_range[k] for k in ('project_id', 'first_number', 'last_number')))
            if previous is None or tuple(previous) != (number_range['first_number'], number_range['last_number']):
                used = db.execute('SELECT coalesce(max(number),?-1)+1 FROM issues WHERE project_id=? AND number BETWEEN ? AND ?', (number_range['first_number'], number_range['project_id'], number_range['first_number'], number_range['last_number'])).fetchone()[0]
                db.execute('UPDATE projects SET next_number=? WHERE id=?', (used, number_range['project_id']))
            else:
                db.execute('UPDATE projects SET next_number=? WHERE id=? AND next_number NOT BETWEEN ? AND ?', (number_range['first_number'], number_range['project_id'], number_range['first_number'], number_range['last_number'] + 1))
        state_set(db, 'cursor', payload['cursor'])
        state_set(db, 'last_sync', time.time())
    finally:
        db.execute('UPDATE fleet_meta SET syncing=0 WHERE id=1')


def allocate(db, node, workers):
    if not db.in_transaction:
        db.execute('BEGIN IMMEDIATE')
    pools = {}
    for worker in workers:
        config = worker['config']
        if worker.get('intent', 'running' if config.get('enabled', True) else 'pause') == 'running':
            for project in config.get('projects', []):
                pools.setdefault(project, []).append(config)
    for worker in workers:
        config = worker['config']
        if worker.get('intent', 'running' if config.get('enabled', True) else 'pause') != 'running':
            continue
        projects = config.get('projects', [])
        if not projects:
            continue  # Discovery must establish checkout identity first.
        for project in projects:
            row = db.execute('SELECT * FROM projects WHERE id=?', (project,)).fetchone()
            if not row or row['hidden_at'] is not None:
                continue
            number_range = db.execute('SELECT * FROM fleet_ranges WHERE node=? AND project_id=?', (node, project)).fetchone()
            used = db.execute('SELECT coalesce(max(number),0) FROM issues WHERE project_id=? AND number BETWEEN ? AND ?', (project, number_range['first_number'], number_range['last_number'])).fetchone()[0] if number_range else 0
            if not number_range or used > number_range['last_number'] - 20:
                first = row['next_number']
                db.execute('UPDATE projects SET next_number=next_number+100 WHERE id=?', (project,))
                db.execute('INSERT INTO fleet_ranges VALUES(?,?,?,?) ON CONFLICT(node,project_id) DO UPDATE SET first_number=excluded.first_number,last_number=excluded.last_number', (node, project, first, first + 99))
                db.execute('INSERT INTO fleet_number_reservations VALUES(?,?,?,?)', (node, project, first, first + 99))
            tags = set(config.get('tags', []))
            existing = db.execute(f"""SELECT count(*) FROM fleet_allocations a
                JOIN issues i ON i.project_id=a.project_id AND i.number=a.issue_number
                WHERE a.node=? AND a.project_id=? AND i.state='open' AND i.deleted_at IS NULL
                AND {ALLOCATED_WORK_SQL}
                AND NOT EXISTS(SELECT 1 FROM json_each(?) wanted
                    WHERE NOT EXISTS(SELECT 1 FROM json_each(i.labels) label WHERE label.value=wanted.value))""",
                (node, project, encode(sorted(tags)))).fetchone()[0]
            capacity = sum(c.get('concurrency', 1) for c in pools[project] if set(c.get('tags', [])) == tags)
            needed = max(0, capacity * 2 - existing)
            candidates = db.execute(f"SELECT i.number,i.labels FROM issues i WHERE project_id=? AND state='open' AND deleted_at IS NULL AND assignee IS NULL AND {READY_WORK_SQL} AND NOT EXISTS(SELECT 1 FROM fleet_allocations a WHERE a.project_id=i.project_id AND a.issue_number=i.number) AND NOT EXISTS(SELECT 1 FROM worker_runs r WHERE r.project_id=i.project_id AND r.issue_number=i.number AND r.finished_at IS NULL) ORDER BY sort_order,number", (project,)).fetchall()
            for candidate in candidates:
                if needed <= 0:
                    break
                if tags.issubset(json.loads(candidate['labels'])):
                    db.execute('INSERT INTO fleet_allocations VALUES(?,?,?)', (project, candidate['number'], node))
                    needed -= 1
    # Overlapping filters can count the same buffer more than once. Keep enough
    # distinct issues for all independent slots after supplying each filter.
    for project, configs in pools.items():
        row = db.execute('SELECT hidden_at FROM projects WHERE id=?', (project,)).fetchone()
        if not row or row['hidden_at'] is not None:
            continue
        filters = [set(c.get('tags', [])) for c in configs]
        matches = lambda labels: any(tags.issubset(json.loads(labels)) for tags in filters)
        allocated = db.execute(f"SELECT i.labels FROM fleet_allocations a JOIN issues i ON i.project_id=a.project_id AND i.number=a.issue_number WHERE a.node=? AND a.project_id=? AND i.state='open' AND i.deleted_at IS NULL AND {ALLOCATED_WORK_SQL}", (node, project)).fetchall()
        needed = max(0, sum(c.get('concurrency', 1) for c in configs) * 2 - sum(matches(r['labels']) for r in allocated))
        if not needed:
            continue
        candidates = db.execute(f"SELECT i.number,i.labels FROM issues i WHERE project_id=? AND state='open' AND deleted_at IS NULL AND assignee IS NULL AND {READY_WORK_SQL} AND NOT EXISTS(SELECT 1 FROM fleet_allocations a WHERE a.project_id=i.project_id AND a.issue_number=i.number) AND NOT EXISTS(SELECT 1 FROM worker_runs r WHERE r.project_id=i.project_id AND r.issue_number=i.number AND r.finished_at IS NULL) ORDER BY sort_order,number", (project,)).fetchall()
        for candidate in candidates:
            if needed <= 0:
                break
            if matches(candidate['labels']):
                db.execute('INSERT INTO fleet_allocations VALUES(?,?,?)', (project, candidate['number'], node))
                needed -= 1


def worker_overview(worker_id=None):
    request = {'version': 1, 'project': {'id': 'named:Fleet', 'name': 'Fleet'},
               'project_override': None, 'actor': None,
               'operation': {'action': 'workers', 'worker_id': worker_id}, 'request_id': None}
    return cli(['issue', 'rpc'], request)


def worker_status():
    value = worker_overview()
    workers = value['workers']
    for worker in workers:
        selected = worker_overview(worker['id'])
        worker.update({k: selected[k] for k in ('active', 'free', 'eligible', 'runs', 'upgrading') if k in selected})
        if worker['pid']:
            try:
                os.kill(worker['pid'], 0)
            except ProcessLookupError:
                worker['pid'] = None
    return workers


def ensure_worker(worker):
    config = worker['config']
    # Create a worker definition first when it came from the supervisor.
    known = worker_overview()['workers']
    if not any(w['id'] == worker['id'] for w in known):
        request = {'version': 1, 'project': {'id': 'named:Fleet', 'name': 'Fleet'}, 'project_override': None,
                   'actor': local_actor(), 'operation': {'action': 'configure_worker', 'worker_id': None, 'config': config, 'if_version': None}, 'request_id': None}
        result = cli(['issue', 'rpc'], request)
        # Stable IDs supplied by a supervisor must not be replaced by random IDs.
        with connect_db(identity()[1]) as db:
            db.execute('UPDATE issue_workers SET id=? WHERE id=?', (worker['id'], result['worker_id']))


def config_directory(worker):
    return worker['config'].get('directory') or str(pathlib.Path.home())


@contextlib.contextmanager
def lifecycle_lock(wait=True):
    """Serialize launch/configuration across the agent daemon and SSH sessions."""
    STATE.mkdir(parents=True, exist_ok=True)
    fd = os.open(STATE / 'fleet-worker-control.lock', os.O_RDWR | os.O_CREAT, 0o600)
    with os.fdopen(fd, 'w') as lock:
        deadline = time.monotonic() + 40
        while True:
            try:
                fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
                break
            except BlockingIOError:
                if not wait:
                    yield False
                    return
                if time.monotonic() >= deadline or STOP.is_set():
                    raise RuntimeError('Worker lifecycle is busy; retry the same signal ID')
                time.sleep(.1)
        try:
            yield True
        finally:
            fcntl.flock(lock, fcntl.LOCK_UN)


def start_worker(worker):
    ensure_worker(worker)
    args = [str(BINARY), 'worker', '--id', worker['id'], '--json']
    STATE.mkdir(parents=True, exist_ok=True)
    fd = os.open(STATE / ('fleet-worker-' + hashlib.sha256(worker['id'].encode()).hexdigest()[:24] + '.log'), os.O_WRONLY | os.O_CREAT | os.O_APPEND, 0o600)
    try:
        environment = os.environ.copy()
        environment.pop('HEY_BOSS_ISSUE_HOST', None)
        environment['HEY_BOSS_FLEET_MANAGED'] = '1'
        process = subprocess.Popen(args, stdin=subprocess.DEVNULL, stdout=fd, stderr=subprocess.STDOUT, env=environment, cwd=config_directory(worker), start_new_session=True)
    finally:
        os.close(fd)
    # Popen succeeding is not proof of a working worker. Do not acknowledge
    # until this exact child owns the durable registration.
    try:
        deadline = time.monotonic() + 15
        while time.monotonic() < deadline:
            if process.poll() is not None:
                raise RuntimeError('Replacement worker exited during startup; inspect its fleet-worker log')
            selected = worker_overview(worker['id'])
            if any(w['id'] == worker['id'] and w['pid'] == process.pid for w in selected['workers']):
                return process.pid
            time.sleep(.1)
        raise RuntimeError('Replacement worker did not register within 15 seconds')
    except BaseException:
        # Only clean up the child we just created, never the supervisor/companion
        # or a PID recovered from an old registration.
        if process.poll() is None:
            process.terminate()
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()
        raise


def control_worker(worker_id, command):
    request = {'version': 1, 'project': {'id': 'named:Fleet', 'name': 'Fleet'}, 'project_override': None,
               'actor': local_actor(), 'operation': {'action': 'control_worker', 'worker_id': worker_id, 'command': command, 'run_id': None}, 'request_id': None}
    return cli(['issue', 'rpc'], request)


def apply_signal(db, message):
    with lifecycle_lock():
        old = db.execute('SELECT worker,signal,state,result FROM fleet_signals WHERE id=?', (message['id'],)).fetchone()
        if old and (old['worker'], old['signal']) != (message['worker'], message['signal']):
            raise ValueError('Signal ID already has a different payload')
        progress = json.loads(old['result']) if old and old['result'] and old['state'] in ('stopping', 'starting') else {}
        if progress.get('retry_at', 0) > time.time():
            return {'id': message['id'], 'state': 'pending', 'error': progress.get('error', 'Worker restart will retry')}
        try:
            return apply_signal_locked(db, message)
        except Exception as error:
            row = db.execute('SELECT state,result FROM fleet_signals WHERE id=?', (message['id'],)).fetchone()
            if row and isinstance(error, ValueError):
                receipt = {'id': message['id'], 'state': 'failed', 'worker': message['worker'], 'signal': message['signal'], 'error': str(error)}
                db.execute("UPDATE fleet_signals SET state='failed',result=? WHERE id=?", (encode(receipt), message['id']))
                db.commit()
            elif row and row['state'] in ('stopping', 'starting'):
                current = json.loads(row['result']) if row['result'] else {}
                failures = progress.get('failures', 0) + 1
                current.update(failures=failures, retry_at=time.time() + min(300, 5 * 2 ** min(failures, 6)), error=str(error))
                db.execute('UPDATE fleet_signals SET result=? WHERE id=?', (encode(current), message['id']))
                db.commit()
            raise


def apply_signal_locked(db, message):
    action = message['signal']
    if action not in ('pause', 'resume', 'stop', 'restart'):
        raise ValueError('Unknown signal')
    old = db.execute('SELECT worker,signal,state,result FROM fleet_signals WHERE id=?', (message['id'],)).fetchone()
    if old and (old['worker'], old['signal']) != (message['worker'], action):
        raise ValueError('Signal ID already has a different payload')
    if old and old['state'] in ('acknowledged', 'superseded', 'failed'):
        return json.loads(old['result'])
    db.execute("INSERT OR IGNORE INTO fleet_signals VALUES(?,?,?,?,'pending',NULL,?)", (message['id'], 'local', message['worker'], action, time.time()))
    db.commit()
    worker = next((w for w in worker_status() if w['id'] == message['worker']), None)
    if not worker:
        raise ValueError('Worker not found on this machine')
    # A newer explicit control supersedes unfinished intent for this worker.
    # Replayed older requests then return a terminal receipt instead of undoing it.
    for previous in db.execute("SELECT id,signal FROM fleet_signals WHERE worker=? AND id<>? AND state IN ('stopping','starting')", (worker['id'], message['id'])).fetchall():
        receipt = {'id': previous['id'], 'state': 'superseded', 'signal': previous['signal'], 'worker': worker['id'], 'superseded_by': message['id']}
        db.execute("UPDATE fleet_signals SET state='superseded',result=? WHERE id=?", (encode(receipt), previous['id']))
    db.commit()
    phase = old['state'] if old else 'pending'
    progress = json.loads(old['result']) if old and old['result'] and phase in ('stopping', 'starting') else {}
    prior = progress.get('prior_pid', worker['pid'])
    already_started = action == 'restart' and phase == 'starting' and worker['pid'] is not None and worker['pid'] != prior
    config_path = STATE / ('fleet-main.json' if db.execute('SELECT role FROM fleet_meta WHERE id=1').fetchone()[0] == SUPERVISOR_ROLE else 'fleet-agent.json')
    saved = read_json(config_path, {})
    for desired in saved.get('workers', []):
        if desired['id'] == worker['id']:
            desired['intent'] = 'stop' if action in ('stop', 'restart') else 'pause' if action == 'pause' else 'running'
    atomic_json(config_path, saved)
    if action in ('stop', 'restart') and not already_started:
        if phase != 'starting':
            db.execute("UPDATE fleet_signals SET state='stopping',result=? WHERE id=?", (encode({'prior_pid': prior}), message['id']))
            db.commit()
            control_worker(worker['id'], 'stop_worker')
        deadline = time.monotonic() + 15
        while True:
            worker = next(w for w in worker_status() if w['id'] == worker['id'])
            if worker['pid'] is None and worker['active'] == 0:
                break
            if phase == 'starting' or time.monotonic() >= deadline:
                raise RuntimeError('Previous worker or owned agents have not stopped; no duplicate was launched')
            time.sleep(.2)
    if action == 'pause':
        control_worker(worker['id'], 'pause')
    elif action == 'resume' and worker['pid']:
        control_worker(worker['id'], 'start')
    elif action in ('resume', 'restart') and not already_started:
        if worker['active']:
            raise RuntimeError('Owned sessions are still active; no duplicate was launched')
        db.execute("UPDATE fleet_signals SET state='starting',result=? WHERE id=?", (encode({'prior_pid': prior}), message['id']))
        db.commit()
        pid = start_worker(worker)
        db.execute("UPDATE fleet_signals SET result=? WHERE id=?", (encode({'prior_pid': prior, 'replacement_pid': pid}), message['id']))
        db.commit()
    for desired in saved.get('workers', []):
        if desired['id'] == worker['id']:
            desired['intent'] = 'running' if action in ('restart', 'resume') else action
    atomic_json(config_path, saved)
    result = {'id': message['id'], 'state': 'acknowledged', 'signal': action, 'worker': worker['id']}
    db.execute("UPDATE fleet_signals SET state='acknowledged',result=? WHERE id=?", (encode(result), message['id']))
    db.commit()
    return result


def configure_companion(db, message):
    with lifecycle_lock(wait=False) as acquired:
        if not acquired:
            return {'kind': 'ack', 'configuration_error': 'Worker restart in progress; configuration will retry'}
        return configure_companion_locked(db, message)


def configure_companion_locked(db, message):
    previous = read_json(STATE / 'fleet-agent.json', {})
    configured = {'role': COMPANION_ROLE, 'controller': message['controller'], 'revision': message['revision'],
                  'workers': message.get('workers', previous.get('workers', []))}
    failures = configure_workers(db, configured['workers'])
    if failures:
        return {'kind': 'ack', 'configuration_error': '; '.join(failures)}
    atomic_json(STATE / 'fleet-agent.json', configured)
    state_set(db, 'revision', message['revision'])
    db.commit()
    return {'kind': 'ack', 'revision': message['revision']}


def configure_workers(db, workers):
    failures = []
    changing = {r['worker'] for r in db.execute("SELECT worker FROM fleet_signals WHERE state IN ('stopping','starting')")}
    for desired in workers:
        if desired['id'] in changing:
            failures.append(desired['id'] + ': Worker restart in progress; configuration will retry')
            continue
        try:
            ensure_worker(desired)
            row = db.execute('SELECT config,version FROM issue_workers WHERE id=?', (desired['id'],)).fetchone()
            config = dict(desired['config'])
            config['enabled'] = desired.get('intent', 'running') == 'running'
            if row and json.loads(row['config']) != config:
                request = {'version': 1, 'project': {'id': 'named:Fleet', 'name': 'Fleet'}, 'project_override': None,
                           'actor': local_actor(), 'operation': {'action': 'configure_worker', 'worker_id': desired['id'], 'config': config, 'if_version': row['version']}, 'request_id': None}
                cli(['issue', 'rpc'], request)
        except Exception as error:
            failures.append(desired['id'] + ': ' + str(error))
    return failures

def reconcile_workers(config):
    with lifecycle_lock(wait=False) as acquired:
        if acquired:
            reconcile_workers_locked(config)


def reconcile_workers_locked(config):
    known = worker_status()
    # A durable interrupted restart owns lifecycle intent until replay completes.
    with connect_db(identity()[1]) as db:
        changing = {r['worker'] for r in db.execute("SELECT worker FROM fleet_signals WHERE state IN ('stopping','starting')")}
    for desired in config.get('workers', []):
        if desired['id'] in changing:
            continue
        try:
            worker = next((w for w in known if w['id'] == desired['id']), None)
            intent = desired.get('intent', 'running' if desired['config'].get('enabled') else 'pause')
            if intent == 'running' and (worker is None or worker['pid'] is None):
                start_worker(desired)
            elif worker and worker['pid']:
                if intent == 'pause' and worker['config']['enabled']:
                    control_worker(worker['id'], 'pause')
                elif intent == 'stop':
                    control_worker(worker['id'], 'stop_worker')
        except Exception as error:
            atomic_json(STATE / 'fleet-agent-error.json', {'worker': desired['id'], 'error': str(error), 'at': time.time()})

def send(output, message):
    data = encode({'version': VERSION, **message}) + '\n'
    if len(data.encode()) > LIMIT:
        raise ValueError('Fleet frame exceeds 16 MiB')
    output.write(data)
    output.flush()


def bootstrap_rows(db):
    # Endpoints precede relationships, which precede their history.
    for table in ('agents', 'issues', 'issue_subtasks', 'comments', 'events', 'issue_pull_requests'):
        for row in db.execute('SELECT * FROM ' + table).fetchall():
            db.execute('INSERT INTO fleet_outbox(table_name,after_json,created_at) VALUES(?,?,?)', (table, encode(dict(row)), int(time.time()*1000)))
    state_set(db, 'bootstrap_last_seq', db.execute('SELECT coalesce(max(seq),0) FROM fleet_outbox').fetchone()[0])


def companion_stdio():
    STATE.mkdir(parents=True, exist_ok=True)
    node, path = identity()
    with connect_db(path) as db:
        if db.execute('SELECT role FROM fleet_meta WHERE id=1').fetchone()[0] == 'standalone':
            backup = STATE / ('fleet-bootstrap-' + str(int(time.time())) + '.db')
            db.backup_path(backup)
            install_capture(db, 'agent', node)
            bootstrap_rows(db)
            db.commit()
        install_capture(db, 'agent', node)
        send(sys.stdout, {'kind': 'hello', 'node': node, 'hostname': socket.gethostname(), 'build': subprocess.check_output([str(BINARY), '--version'], text=True).strip(),
                          'projects': [dict(r) for r in db.execute('SELECT * FROM projects')],
                          'local_config': [w for w in read_json(STATE / 'fleet-agent.json', {}).get('workers', []) if 'local_revision' in w],
                          'workers': worker_status(), 'cursor': state_get(db, 'cursor'), 'revision': state_get(db, 'revision'), 'pending': db.execute('SELECT count(*) FROM fleet_outbox').fetchone()[0]})
        signal_queue = queue.Queue(maxsize=100)
        output_lock = threading.Lock()
        def reply(message):
            with output_lock:
                send(sys.stdout, message)
        def signals():
            with connect_db(path) as signal_db:
                while True:
                    message = signal_queue.get()
                    if message is None:
                        return
                    try:
                        result = apply_signal(signal_db, message)
                    except Exception as error:
                        result = {'id': message['id'], 'state': 'failed' if isinstance(error, ValueError) else 'pending', 'error': str(error)}
                    reply({'kind': 'ack', 'signal': result})
        threading.Thread(target=signals, daemon=True).start()
        while raw := sys.stdin.readline(LIMIT + 1):
            if len(raw.encode()) > LIMIT:
                raise ValueError('Fleet frame exceeds 16 MiB')
            message = json.loads(raw)
            if message.get('version') != VERSION:
                raise ValueError('Unsupported fleet protocol version')
            kind = message.get('kind')
            if kind == 'configure':
                reply(configure_companion(db, message))
            elif kind == 'pull':
                with db:
                    apply_pull(db, node, message['payload'], message.get('receipts', []))
                atomic_json(STATE / 'fleet-agent-status.json', {'connected_at': time.time(), 'last_sync': time.time()})
                reconcile_workers(read_json(STATE / 'fleet-agent.json', {}))
                reply({'kind': 'ack', 'cursor': state_get(db, 'cursor'), 'pending': db.execute('SELECT count(*) FROM fleet_outbox').fetchone()[0]})
            elif kind == 'signal':
                try:
                    signal_queue.put_nowait(message)
                except queue.Full:
                    reply({'kind': 'ack', 'signal': {'id': message['id'], 'state': 'pending', 'error': 'Worker control queue is busy'}})
            elif kind == 'ping':
                atomic_json(STATE / 'fleet-agent-status.json', {'connected_at': time.time(), 'last_sync': state_get(db, 'last_sync')})
                reply({'kind': 'heartbeat', 'at': time.time(), 'workers': worker_status(), 'changes': journal(db), 'cursor': state_get(db, 'cursor'),
                                  'local_config': [w for w in read_json(STATE / 'fleet-agent.json', {}).get('workers', []) if 'local_revision' in w],
                                  'pending': db.execute('SELECT count(*) FROM fleet_outbox').fetchone()[0], 'conflicts': db.execute('SELECT count(*) FROM fleet_conflicts WHERE resolved=0').fetchone()[0], 'revision': state_get(db, 'revision')})
            else:
                raise ValueError('Unknown fleet message kind')


def inventory():
    value = read_json(pathlib.Path(os.environ.get('HEY_BOSS_FLEET_CONFIG', pathlib.Path.home() / '.hey-boss/config.json')), {})
    hosts = []
    for entry in value.get('ssh_hosts', []):
        entry = {'host': entry} if isinstance(entry, str) else entry
        if not valid_host(entry.get('host')):
            raise ValueError('Invalid configured SSH host')
        if entry.get('enabled', True):
            hosts.append(entry)
    if not hosts:
        try:
            hosts = [{'host': h} for h in (STATE / 'companion-hosts').read_text().splitlines() if valid_host(h)]
        except FileNotFoundError:
            pass
    overrides = read_json(DESIRED, {})
    for entry in hosts:
        entry.update(overrides.get('machines', {}).get(entry['host'], {}))
    return hosts


class Supervisor:
    def __init__(self, path, node):
        self.path, self.node = path, node
        self.lock = threading.RLock()
        self.nodes = {}
        self.events = []
        self.epoch = uuid.uuid4().hex
        self.sequence = 0
        self.threads = {}
        self.connections = {}
        self.deployment_lock = threading.Lock()
        self.deployment_thread = None
        self.desired_build = None
        self.startup_build = subprocess.check_output([str(BINARY), '--version'], text=True).strip()
        self.local_workers = worker_status()
        self.local_updated = time.time()
        self.source = (STATE / 'upgrade-source').read_text().strip() if (STATE / 'upgrade-source').exists() else None
        with connect_db(path) as db:
            install_capture(db, SUPERVISOR_ROLE, node)
            self.nodes = state_get(db, 'machines', {})
            for machine in self.nodes.values():
                machine['state'] = 'disconnected'
                if machine.get('deployment') == 'updating':
                    machine['deployment'] = 'outdated'
        if not (STATE / 'fleet-main.json').exists():
            atomic_json(STATE / 'fleet-main.json', {'role': SUPERVISOR_ROLE, 'workers': [{'id': w['id'], 'config': w['config'], 'intent': 'running' if w['config']['enabled'] else 'pause'} for w in self.local_workers]})

    def event(self, host, kind, detail):
        with self.lock:
            self.sequence += 1
            self.events.append({'id': self.sequence, 'epoch': self.epoch, 'at': time.time(), 'host': host, 'kind': kind, 'detail': detail})
            self.events = self.events[-200:]

    def update(self, host, **fields):
        with self.lock:
            self.nodes.setdefault(host, {'host': host}).update(fields)
            with connect_db(self.path) as db:
                state_set(db, 'machines', self.nodes)

    def status(self):
        with self.lock:
            with connect_db(self.path) as db:
                signals = [dict(r) for r in db.execute('SELECT * FROM fleet_signals ORDER BY created_at DESC LIMIT 100')]
                conflicts = [dict(r) for r in db.execute('SELECT id,node,seq,table_name,reason,created_at,substr(data,1,8192) AS saved_change FROM fleet_conflicts WHERE resolved=0 ORDER BY created_at DESC LIMIT 100')]
            local = {'host': 'local', 'hostname': socket.gethostname(), 'node': self.node, 'role': 'supervisor', 'state': 'connected', 'heartbeat': self.local_updated, 'workers': self.local_workers, 'pending': 0, 'build': self.startup_build}
            machines = json.loads(encode([v for k, v in self.nodes.items() if k != 'local']))
            for machine in machines:
                if machine.get('role') == COMPANION_ROLE:
                    machine['role'] = 'companion'
            return {'ok': True, 'supervisor': self.node, 'controller': self.node, 'epoch': self.epoch, 'sequence': self.sequence,
                    'desired_build': self.desired_build,
                    'machines': [local, *machines], 'events': list(self.events), 'signals': signals, 'conflicts': conflicts}

    def signal(self, request):
        with self.lock:
            return self.signal_locked(request)

    def signal_locked(self, request):
        host, worker, action = request['host'], request['worker'], request['signal']
        if host != 'local' and host not in {h['host'] for h in inventory()}:
            raise ValueError('Machine is not in the configured inventory')
        if action not in ('pause', 'resume', 'stop', 'restart'):
            raise ValueError('Unknown signal')
        identifier = request.get('id') or uuid.uuid4().hex
        with connect_db(self.path) as db:
            old = db.execute('SELECT * FROM fleet_signals WHERE id=?', (identifier,)).fetchone()
            if old:
                if (old['host'], old['worker'], old['signal']) != (host, worker, action):
                    raise ValueError('Signal ID already has a different payload')
                return {'ok': True, 'id': identifier, 'state': old['state']}
            else:
                db.execute("INSERT INTO fleet_signals VALUES(?,?,?,?,'pending',NULL,?)", (identifier, host, worker, action, time.time()))
        path = DESIRED
        saved = read_json(path, {})
        desired = saved.setdefault('machines', {}).setdefault(host, {})
        default = read_json(STATE / 'fleet-main.json', {}).get('workers', []) if host == 'local' else self.nodes.get(host, {}).get('desired_workers', [])
        workers = desired.setdefault('workers', json.loads(encode(default)))
        for definition in workers:
            if definition['id'] == worker:
                definition['intent'] = 'running' if action in ('resume', 'restart') else action
        atomic_json(path, saved)
        self.event(host, 'signal', action + ' queued for ' + worker)
        return {'ok': True, 'id': identifier, 'state': 'pending'}

    def local_config(self, host, changes, workers):
        with self.lock:
            return self.local_config_locked(host, changes, workers)

    def local_config_locked(self, host, changes, workers):
        if not changes:
            return workers
        current = hashlib.sha256(encode({'controller': self.node, 'workers': workers}).encode()).hexdigest()[:16]
        updated = json.loads(encode(workers))
        with connect_db(self.path) as db:
            for change in changes:
                key = 'config:' + host + ':' + change['id']
                if state_get(db, key, 0) >= change['local_revision']:
                    continue
                if change.get('base_revision') != current:
                    identifier = key + ':' + str(change['local_revision'])
                    db.execute('INSERT OR IGNORE INTO fleet_conflicts(id,node,seq,table_name,data,reason,created_at) VALUES(?,?,?,?,?,?,?)', (identifier, host, change['local_revision'], 'worker_configuration', encode(change), 'Supervisor configuration changed while local settings were edited', int(time.time()*1000)))
                else:
                    definition = {k: change[k] for k in ('id', 'config', 'intent')}
                    old = next((w for w in updated if w['id'] == change['id']), None)
                    if old:
                        old.update(definition)
                    else:
                        updated.append(definition)
                state_set(db, key, change['local_revision'])
        path = DESIRED
        saved = read_json(path, {})
        saved.setdefault('machines', {}).setdefault(host, {})['workers'] = updated
        atomic_json(path, saved)
        self.event(host, 'configuration', 'Local worker settings synchronized')
        return updated

    def schedule_deploy(self, host):
        # Builds can take minutes; keep heartbeats, status, and controls responsive.
        with self.lock:
            if self.deployment_thread and self.deployment_thread.is_alive():
                return False
            self.deployment_thread = threading.Thread(target=self.deploy, args=(host,), daemon=True)
            self.deployment_thread.start()
            return True

    def deploy(self, host):
        if not self.deployment_lock.acquire(blocking=False):
            return False
        try:
            self.update(host, deployment='updating')
            self.event(host, 'deployment', 'Installing desired software')
            args = [str(BINARY), 'upgrade', '--json', *(['--local-only'] if host == 'local' else ['--host', host])]
            if self.source:
                args += ['--source', self.source]
            result = subprocess.run(args, capture_output=True, text=True, timeout=1200)
            report = json.loads(result.stdout) if result.stdout.strip().startswith('{') else {}
            target = next((m for m in report.get('machines', []) if m['host'] == host), None)
            if not target or target['status'] not in ('current', 'updated'):
                raise RuntimeError((target or {}).get('error') or (result.stderr or result.stdout)[-2000:])
            self.update(host, deployment='current', deployment_error=None)
            self.event(host, 'deployment', 'Software deployment complete')
            # Reconnect the channel to load the new companion implementation.
            # Independently owned worker sessions are left running.
            with self.lock:
                connection = self.connections.get(host)
                if connection:
                    connection.terminate()
            return True
        except Exception as error:
            self.update(host, deployment='failed', deployment_error=str(error), retry_deploy_at=time.time() + 60)
            self.event(host, 'deployment', str(error))
            return False
        finally:
            self.deployment_lock.release()

    def connection(self, host):
        failures = 0
        while not STOP.is_set() and host in {h['host'] for h in inventory()}:
            process = None
            try:
                self.update(host, state='connecting')
                # Remote peers may still require the legacy agent command during upgrade.
                script = 'export PATH="$HOME/.local/bin:$HOME/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:$PATH"; hey-boss fleet agent --install >&2 && exec hey-boss fleet agent --stdio'
                process = subprocess.Popen(['ssh', '-T', '-o', 'BatchMode=yes', '-o', 'ConnectTimeout=8', '-o', 'StrictHostKeyChecking=yes', '-o', 'ServerAliveInterval=5', '-o', 'ServerAliveCountMax=2', host, script],
                                           stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, bufsize=1,
                                           env={**os.environ, 'SFT_NO_BROWSER': '1', 'SSH_ASKPASS_REQUIRE': 'never'})
                with self.lock:
                    self.connections[host] = process
                stderr_tail = []
                def drain_errors(process=process, tail=stderr_tail):
                    for line in process.stderr:
                        tail.append(line[-2000:])
                        del tail[:-10]
                threading.Thread(target=drain_errors, daemon=True).start()
                inbox = queue.Queue(32)
                def read(process=process, inbox=inbox):
                    try:
                        while True:
                            raw = process.stdout.readline(LIMIT + 1)
                            if not raw:
                                break
                            if len(raw.encode()) > LIMIT:
                                raise ValueError('Fleet frame exceeds limit')
                            inbox.put(json.loads(raw), timeout=10)
                    except Exception as error:
                        inbox.put(error)
                    finally:
                        inbox.put(None)
                threading.Thread(target=read, daemon=True).start()
                hello = inbox.get(timeout=15)
                if not isinstance(hello, dict) or hello.get('kind') != 'hello' or hello.get('version') != VERSION:
                    raise RuntimeError('Companion is missing or has an incompatible fleet protocol: ' + ''.join(stderr_tail)[-2000:])
                node = hello['node']
                with connect_db(self.path) as db:
                    db.execute('BEGIN IMMEDIATE')
                    for project in hello.get('projects', []):
                        if current_row(db, 'projects', project) is None:
                            put_row(db, 'projects', project)
                desired = next(h for h in inventory() if h['host'] == host)
                workers = desired.get('workers')
                if workers is None:
                    previous = self.nodes.get(host, {})
                    workers = previous.get('desired_workers') or [{'id': w['id'], 'config': w['config'], 'intent': 'running' if w['config']['enabled'] else 'pause'} for w in hello['workers']]
                workers = self.local_config(host, hello.get('local_config', []), workers)
                revision = hashlib.sha256(encode({'controller': self.node, 'workers': workers}).encode()).hexdigest()[:16]
                self.update(host, node=node, hostname=hello['hostname'], state='connected', role=COMPANION_ROLE, heartbeat=time.time(), build=hello['build'], workers=hello['workers'], desired_workers=workers, desired_revision=revision, applied_revision=hello.get('revision'), pending=hello.get('pending', 0), error=None)
                self.event(host, 'connected', 'Companion connected')
                send(process.stdin, {'kind': 'configure', 'controller': self.node, 'revision': revision, 'workers': workers})
                last_message = time.monotonic()
                last_ping = 0
                failures = 0
                while not STOP.is_set():
                    if time.monotonic() - last_ping >= 5:
                        send(process.stdin, {'kind': 'ping'})
                        last_ping = time.monotonic()
                    if time.monotonic() - last_message > 15:
                        raise TimeoutError('Companion heartbeat timed out')
                    try:
                        message = inbox.get(timeout=.25)
                    except queue.Empty:
                        continue
                    if message is None:
                        raise ConnectionError('Companion connection closed: ' + ''.join(stderr_tail)[-2000:])
                    if isinstance(message, Exception):
                        raise message
                    if message.get('version') != VERSION:
                        raise ValueError('Protocol version mismatch')
                    last_message = time.monotonic()
                    self.update(host, heartbeat=time.time())
                    if message['kind'] == 'heartbeat':
                        configured = next(h for h in inventory() if h['host'] == host).get('workers', workers)
                        workers = self.local_config(host, message.get('local_config', []), configured)
                        for discovered in message['workers']:
                            if not any(w['id'] == discovered['id'] for w in workers):
                                workers.append({'id': discovered['id'], 'config': discovered['config'], 'intent': 'running' if discovered['config']['enabled'] else 'pause'})
                        with connect_db(self.path) as db:
                            receipts = accept_changes(db, node, message['changes'])
                            allocate(db, node, workers)
                            payload = export_snapshot(db, node) if message['cursor'] is None else export_incremental(db, node, message['cursor'])
                            # A rejected local row still needs the current canonical value.
                            corrections = {}
                            for change, receipt in zip(message['changes'], receipts):
                                if receipt['state'] == 'conflict' and change['table_name'] not in APPEND:
                                    row = json.loads(change['after_json'] or change['before_json'])
                                    canonical = current_row(db, change['table_name'], row)
                                    if canonical:
                                        corrections.setdefault(change['table_name'], []).append(canonical)
                            for table, rows in corrections.items():
                                payload.setdefault('tables', {}).setdefault(table, []).extend(rows)
                            signals = [dict(r) for r in db.execute("SELECT * FROM fleet_signals WHERE host=? AND state='pending' ORDER BY created_at", (host,))]
                        send(process.stdin, {'kind': 'pull', 'payload': payload, 'receipts': receipts})
                        previous_workers = self.nodes.get(host, {}).get('workers', [])
                        def activity(workers):
                            return [(w['id'], w['active'], w['config'], [(r['id'], r['state'], r['last_event']) for r in w.get('runs', [])]) for w in workers]
                        changed = activity(previous_workers) != activity(message['workers'])
                        self.update(host, workers=message['workers'], pending=message['pending'], conflicts=message['conflicts'], applied_revision=message.get('revision'), last_sync=time.time())
                        self.event(host, 'activity' if changed else 'heartbeat', str(sum(w['active'] for w in message['workers'])) + ' active sessions; ' + str(message['pending']) + ' outgoing changes')
                        for pending in signals:
                            send(process.stdin, {'kind': 'signal', **{k: pending[k] for k in ('id', 'worker', 'signal')}})
                        current = next(h for h in inventory() if h['host'] == host).get('workers', workers)
                        updated = hashlib.sha256(encode({'controller': self.node, 'workers': current}).encode()).hexdigest()[:16]
                        if updated != revision or self.nodes.get(host, {}).get('configuration_error'):
                            workers, revision = current, updated
                            send(process.stdin, {'kind': 'configure', 'controller': self.node, 'revision': revision, 'workers': workers})
                            self.update(host, desired_workers=workers, desired_revision=revision)
                    elif message['kind'] == 'ack':
                        if 'signal' in message:
                            acknowledgment = message['signal']
                            with connect_db(self.path) as db:
                                db.execute('UPDATE fleet_signals SET state=?,result=? WHERE id=?', (acknowledgment['state'], encode(acknowledgment), acknowledgment['id']))
                            self.event(host, 'signal', encode(acknowledgment))
                        if 'revision' in message:
                            self.update(host, applied_revision=message['revision'], configuration_error=None)
                        if 'configuration_error' in message:
                            self.update(host, configuration_error=message['configuration_error'])
                            self.event(host, 'configuration', message['configuration_error'])
            except Exception as error:
                failures += 1
                self.update(host, state='disconnected', error=str(error), retry_at=time.time() + min(60, 2 ** min(failures, 6)))
                self.event(host, 'disconnected', str(error))
                machine = self.nodes.get(host, {})
                if machine.get('retry_deploy_at', 0) <= time.time() and ('protocol' in str(error) or machine.get('deployment') == 'failed'):
                    self.deploy(host)
            finally:
                if process:
                    process.terminate()
                    try:
                        process.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        process.kill()
                        process.wait()
                with self.lock:
                    self.connections.pop(host, None)
            STOP.wait(min(60, 2 ** min(failures, 6)))

    def loop(self):
        while not STOP.is_set():
            try:
                hosts = inventory()
                observed = worker_status()
                with self.lock:
                    main = read_json(STATE / 'fleet-main.json', {})
                    if any('local_revision' in w for w in main.get('workers', [])):
                        saved = read_json(DESIRED, {})
                        main['workers'] = [{k: w[k] for k in ('id', 'config', 'intent')} for w in main['workers']]
                        saved.setdefault('machines', {}).setdefault('local', {})['workers'] = main['workers']
                        atomic_json(DESIRED, saved)
                    desired = read_json(DESIRED, {}).get('machines', {}).get('local', {}).get('workers', main.get('workers', []))
                    for discovered in observed:
                        if not any(w['id'] == discovered['id'] for w in desired):
                            desired.append({'id': discovered['id'], 'config': discovered['config'], 'intent': 'running' if discovered['config']['enabled'] else 'pause'})
                    main = {'role': SUPERVISOR_ROLE, 'workers': desired, 'revision': hashlib.sha256(encode(desired).encode()).hexdigest()[:16]}
                    atomic_json(STATE / 'fleet-main.json', main)
                with connect_db(self.path) as db:
                    failures = configure_workers(db, desired)
                if failures:
                    self.event('local', 'configuration', '; '.join(failures))
                reconcile_workers(main)
                old_runs = {r['id']: (r['state'], r['last_event']) for w in self.local_workers for r in w.get('runs', [])}
                for w in observed:
                    for r in w.get('runs', []):
                        if old_runs.get(r['id']) != (r['state'], r['last_event']):
                            self.event('local', 'worker', (r['project_name'] + ' #' + str(r['number']) + ' · ' + r['state'] + ' · ' + r['last_event'])[:500])
                with self.lock:
                    self.local_workers, self.local_updated = observed, time.time()
                self.event('local', 'heartbeat', 'Worker state refreshed')
                for entry in hosts:
                    host = entry['host']
                    if host not in self.threads or not self.threads[host].is_alive():
                        thread = threading.Thread(target=self.connection, args=(host,), daemon=True)
                        self.threads[host] = thread
                        thread.start()
                with connect_db(self.path) as db:
                    local_signals = [dict(r) for r in db.execute("SELECT * FROM fleet_signals WHERE host='local' AND state IN ('pending','stopping','starting') ORDER BY created_at")]
                    for pending in local_signals:
                        try:
                            apply_signal(db, pending)
                        except Exception as error:
                            self.event('local', 'signal', pending['id'] + ': ' + str(error))
                if self.source:
                    # Fingerprint source in a separate process, with a debounce before deployment.
                    result = subprocess.run([str(BINARY), 'upgrade', '--source', self.source, '--local-only', '--check', '--json'], capture_output=True, text=True, timeout=20)
                    if result.returncode in (0, 2):
                        build = json.loads(result.stdout)['build']
                        self.desired_build = build
                        with connect_db(self.path) as db:
                            previous = state_get(db, 'desired_build')
                            state_set(db, 'desired_build', build)
                        if previous and build != previous:
                            self.event('local', 'deployment', 'Source changed; automatic deployment scheduled')
                        if ('build ' + build + ')') not in self.startup_build and self.nodes.get('local', {}).get('retry_deploy_at', 0) <= time.time():
                            self.schedule_deploy('local')
                        for entry in hosts:
                            machine = self.nodes.get(entry['host'], {})
                            if machine.get('build') and ('build ' + build + ')') in machine['build']:
                                if machine.get('deployment') != 'current' or machine.get('deployment_error'):
                                    self.update(entry['host'], deployment='current', deployment_error=None)
                            elif machine.get('build') and ('build ' + build + ')') not in machine['build'] and machine.get('deployment') != 'updating' and machine.get('retry_deploy_at', 0) <= time.time():
                                self.update(entry['host'], deployment='outdated')
                    for entry in hosts:
                        if self.nodes.get(entry['host'], {}).get('deployment') == 'outdated':
                            self.schedule_deploy(entry['host'])
                if os.environ.get('HEY_BOSS_FLEET_SUPERVISED') == '1' and subprocess.check_output([str(BINARY), '--version'], text=True).strip() != self.startup_build:
                    self.event('local', 'deployment', 'Supervisor reloading updated software')
                    STOP.set()
            except Exception as error:
                self.event('local', 'error', str(error))
            STOP.wait(5)


def local_request(value):
    with socket.socket(socket.AF_UNIX) as connection:
        connection.settimeout(15)
        connection.connect(str(STATE / 'fleet.sock'))
        connection.sendall((encode(value) + '\n').encode())
        connection.shutdown(socket.SHUT_WR)
        output = b''
        while True:
            chunk = connection.recv(65536)
            if not chunk:
                break
            output += chunk
            if len(output) > LIMIT:
                raise ValueError('Response exceeds limit')
        return json.loads(output)


class MobileIssues:
    """Only the supervisor consumes Fly's durable phone creation queue."""
    def __init__(self, path, node):
        self.path, self.node = path, node

    def rpc(self, operation, project=None, request_id=None):
        actor = {'id': 'human:boss', 'kind': 'human', 'session_id': None,
                 'machine': self.node, 'host': socket.gethostname(), 'pid': None,
                 'process_start': None, 'cwd': str(pathlib.Path.home()), 'source': 'phone'}
        request = {'version': 1, 'project': project or {'id': 'named:Fleet', 'name': 'Fleet'},
                   'project_override': None, 'actor': actor if request_id else None,
                   'operation': operation, 'request_id': request_id}
        environment = os.environ.copy()
        environment['HEY_BOSS_ISSUE_DB'] = str(self.path)
        environment.pop('HEY_BOSS_ISSUE_HOST', None)
        result = subprocess.run([str(BINARY), 'issue', '--json', 'rpc'], input=encode(request),
                                capture_output=True, text=True, env=environment,
                                cwd=str(pathlib.Path.home()), timeout=20)
        value = json.loads(result.stdout)
        if result.returncode and value.get('error', {}).get('code') not in ('invalid_input', 'not_found', 'conflict'):
            raise RuntimeError('Issue store temporarily unavailable')
        return value

    def call(self, path, body=None):
        # The existing pairing secret is used only in the HTTPS Authorization header.
        config = read_json(self.path.parent / 'mobile.json')
        if not config:
            raise FileNotFoundError('Mobile pairing is not configured')
        url = urllib.parse.urlsplit(config['url'])
        if url.scheme != 'https' or not url.hostname or url.username or url.password or url.query or url.fragment:
            raise ValueError('Invalid mobile service origin')
        request = urllib.request.Request(config['url'].rstrip('/') + path,
            data=None if body is None else encode(body).encode(),
            headers={'Authorization': 'Bearer ' + config['token'], 'Content-Type': 'application/json'})
        # Never send the bridge credential to a redirected endpoint.
        class NoRedirect(urllib.request.HTTPRedirectHandler):
            def redirect_request(self, *args, **kwargs):
                return None
        with urllib.request.build_opener(NoRedirect).open(request, timeout=10) as response:
            data = response.read(LIMIT + 1)
            if len(data) > LIMIT:
                raise ValueError('Mobile response exceeds limit')
            return json.loads(data)

    def sync(self):
        registry = self.rpc({'action': 'projects', 'include_hidden': True})
        if not registry.get('ok'):
            raise RuntimeError('Issue project registry unavailable')
        projects = {p['id']: {'id': p['id'], 'name': p['name']} for p in registry['projects']}
        visible = {p['id'] for p in registry['projects'] if p.get('hidden_at') is None}
        self.call('/api/bridge/issue-projects', {'projects': [p for id, p in projects.items() if id in visible]})
        for creation in self.call('/api/bridge/issues')['creations']:
            request_id = creation['requestID']
            if not isinstance(request_id, str) or not 0 < len(request_id) <= 128 or any(c not in 'abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-_' for c in request_id):
                raise ValueError('Invalid mobile creation ID')
            project = projects.get(creation['project'])
            accepted = False
            if project is not None and project['id'] not in visible:
                # A project hidden after native commit must not turn a lost ack
                # into a false error. Reuse the native receipt without new writes.
                with contextlib.closing(sqlite3.connect(self.path)) as db:
                    accepted = db.execute('SELECT EXISTS(SELECT 1 FROM requests WHERE project_id=? AND actor=? AND request_id=?)',
                        (project['id'], 'human:boss', 'mobile:' + request_id)).fetchone()[0]
            if project is None or project['id'] not in visible and not accepted:
                outcome = {'status': 'error', 'error': 'This project is no longer registered or is hidden. Choose a registered project and submit again.'}
            else:
                value = self.rpc({'action': 'create', 'title': creation['title'], 'body': creation['body'],
                                  'labels': creation['labels'], 'at_top': True}, project, 'mobile:' + request_id)
                outcome = {'status': 'synced', 'number': value['issue']['number']} if value.get('ok') else {
                    'status': 'error', 'error': value['error']['message'][:1000]}
            # Lost acknowledgments replay the same native idempotency key.
            self.call('/api/bridge/issues/' + request_id + '/result', outcome)

    def loop(self):
        while not STOP.is_set():
            try:
                self.sync()
            except Exception:
                # Offline/configuration/database failures leave Fly's queue pending.
                # Do not log exceptions containing request headers or pairing data.
                pass
            STOP.wait(5)


def supervisor():
    STATE.mkdir(parents=True, exist_ok=True)
    lock = open(STATE / 'fleet-controller.lock', 'a')
    fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
    node, path = identity()
    app = Supervisor(path, node)
    threading.Thread(target=MobileIssues(path, node).loop, daemon=True).start()
    socket_path = STATE / 'fleet.sock'
    socket_path.unlink(missing_ok=True)
    class Handler(socketserver.StreamRequestHandler):
        def handle(self):
            self.connection.settimeout(15)
            try:
                raw = self.rfile.readline(LIMIT + 1)
                if len(raw) > LIMIT:
                    raise ValueError('Request exceeds limit')
                value = json.loads(raw)
                if value.get('kind') == 'subscribe':
                    cursor = value.get('after', 0)
                    self.wfile.write(b'retry: 2000\nevent: connected\ndata: {}\n\n')
                    self.wfile.flush()
                    while not STOP.is_set():
                        with app.lock:
                            events = [e for e in app.events if e['id'] > cursor]
                        for event in events:
                            self.wfile.write(('id: ' + str(event['id']) + '\ndata: ' + encode(event) + '\n\n').encode())
                            cursor = event['id']
                        if not events:
                            self.wfile.write(b': heartbeat\n\n')
                        self.wfile.flush()
                        STOP.wait(1)
                    return
                if value.get('kind') == 'status':
                    result = app.status()
                elif value.get('kind') == 'signal':
                    result = app.signal(value)
                else:
                    raise ValueError('Unknown supervisor request')
            except (BrokenPipeError, ConnectionResetError):
                return
            except Exception as error:
                result = {'ok': False, 'error': str(error)}
            self.wfile.write(encode(result).encode())
    class Server(socketserver.ThreadingUnixStreamServer):
        daemon_threads = True
    with Server(str(socket_path), Handler) as server:
        os.chmod(socket_path, 0o600)
        threading.Thread(target=app.loop, daemon=True).start()
        server.timeout = .25
        while not STOP.is_set():
            server.handle_request()
    socket_path.unlink(missing_ok=True)


def install_service(role):
    STATE.mkdir(parents=True, exist_ok=True)
    command = [str(BINARY), 'fleet', 'supervisor' if role == SUPERVISOR_ROLE else 'companion']
    if sys.platform == 'darwin':
        import plistlib
        label = 'local.hey-boss-fleet-' + role
        path = pathlib.Path.home() / 'Library/LaunchAgents' / (label + '.plist')
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(plistlib.dumps({'Label': label, 'ProgramArguments': command, 'EnvironmentVariables': {'HEY_BOSS_FLEET_SUPERVISED':'1'}, 'RunAtLoad': True, 'KeepAlive': True,
                                        'ThrottleInterval': 10, 'AbandonProcessGroup': True, 'StandardOutPath': str(STATE / ('fleet-' + role + '.log')), 'StandardErrorPath': str(STATE / ('fleet-' + role + '.log'))}))
        domain = 'gui/' + str(os.getuid())
        subprocess.run(['launchctl', 'bootout', domain + '/' + label], capture_output=True)
        # launchd can return EIO while the previous registration is unloading.
        # Keep the stable service label and stop retrying as soon as it starts.
        for delay in [0, .1, .2, .4, .8, 1, 2, 2, 2]:
            if delay:
                time.sleep(delay)
            result = subprocess.run(['launchctl', 'bootstrap', domain, str(path)], capture_output=True, text=True)
            if result.returncode == 0:
                break
            if result.returncode != 5:
                break
        if result.returncode:
            sys.stderr.write(result.stderr)
            result.check_returncode()
    elif sys.platform.startswith('linux'):
        path = pathlib.Path.home() / '.config/systemd/user' / ('hey-boss-fleet-' + role + '.service')
        path.parent.mkdir(parents=True, exist_ok=True)
        name = 'supervisor' if role == SUPERVISOR_ROLE else 'companion'
        path.write_text('[Unit]\nDescription=Hey Boss fleet ' + name + '\n[Service]\nExecStart=' + shlex.join(command) + '\nEnvironment=HEY_BOSS_FLEET_SUPERVISED=1\nKillMode=process\nRestart=always\nRestartSec=10\n[Install]\nWantedBy=default.target\n')
        subprocess.run(['systemctl', '--user', 'daemon-reload'], check=True)
        subprocess.run(['systemctl', '--user', 'enable', '--now', path.name], check=True)
        subprocess.run(['systemctl', '--user', 'restart', path.name], check=True)
    else:
        raise RuntimeError('Automatic startup requires macOS launchd or Linux systemd')


def companion_daemon():
    STATE.mkdir(parents=True, exist_ok=True)
    lock = open(STATE / 'fleet-agent.lock', 'a')
    fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
    _, path = identity()
    while not STOP.is_set():
        with lifecycle_lock(wait=False) as acquired:
            if acquired:
                with connect_db(path) as db:
                    interrupted = [dict(r) for r in db.execute("SELECT * FROM fleet_signals WHERE host='local' AND state IN ('stopping','starting') ORDER BY created_at")]
                # Release the lock before apply_signal reacquires it.
        if acquired:
            for pending in interrupted:
                try:
                    with connect_db(path) as db:
                        apply_signal(db, pending)
                except Exception as error:
                    atomic_json(STATE / 'fleet-agent-error.json', {'worker': pending['worker'], 'error': str(error), 'at': time.time()})
        config = read_json(STATE / 'fleet-agent.json', {})
        if config:
            try:
                reconcile_workers(config)
            except Exception as error:
                atomic_json(STATE / 'fleet-agent-error.json', {'error': str(error), 'at': time.time()})
        STOP.wait(5)


def main():
    os.umask(0o077)
    parser = argparse.ArgumentParser()
    sub = parser.add_subparsers(dest='command', required=True)
    setup = sub.add_parser('setup')
    setup.add_argument('--source')
    sub.add_parser('supervisor', aliases=['controller'])
    companion = sub.add_parser('companion', aliases=['agent'])
    companion.add_argument('--stdio', action='store_true')
    companion.add_argument('--install', action='store_true')
    sub.add_parser('status')
    sig = sub.add_parser('signal')
    sig.add_argument('host')
    sig.add_argument('worker')
    sig.add_argument('signal')
    args = parser.parse_args()
    for signum in (signal.SIGTERM, signal.SIGINT):
        signal.signal(signum, lambda *_: STOP.set())
    if args.command == 'setup':
        if args.source:
            source = pathlib.Path(args.source).resolve()
            if not (source / 'Cargo.toml').is_file():
                raise ValueError('Source must be a hey-boss checkout')
            STATE.mkdir(parents=True, exist_ok=True)
            (STATE / 'upgrade-source').write_text(str(source) + '\n')
        install_service(SUPERVISOR_ROLE)
        print('Automatic fleet supervisor started. Workers view: http://127.0.0.1:4781/workers')
    elif args.command in ('supervisor', 'controller'):
        supervisor()
    elif args.command in ('companion', 'agent'):
        if args.install:
            version = subprocess.check_output([str(BINARY), '--version'], text=True).strip()
            if read_json(STATE / 'fleet-agent-service.json', {}).get('build') != version:
                install_service(COMPANION_ROLE)
                atomic_json(STATE / 'fleet-agent-service.json', {'build': version})
        elif args.stdio:
            companion_stdio()
        else:
            companion_daemon()
    elif args.command == 'status':
        print(json.dumps(local_request({'kind': 'status'}), indent=2))
    elif args.command == 'signal':
        print(encode(local_request({'kind': 'signal', 'host': args.host, 'worker': args.worker, 'signal': args.signal})))


if __name__ == '__main__':
    try:
        main()
    except Exception as error:
        print('hey-boss fleet: ' + str(error), file=sys.stderr)
        sys.exit(1)
