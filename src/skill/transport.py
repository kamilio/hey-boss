"""Versioned skill transport; runs unchanged on older fleet installations."""
import base64
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import sys
import tempfile
import time

ROOTS = {'codex': '.codex', 'agents': '.agents', 'claude': '.claude'}
MAX_BUNDLE = 4 * 1024 * 1024
MAX_SCAN = 64 * 1024 * 1024


def valid_name(name):
    return bool(re.fullmatch(r'[A-Za-z0-9_-]+', name))


def digest(files):
    return hashlib.sha256(json.dumps(files, sort_keys=True, separators=(',', ':')).encode()).hexdigest()


def bundle(root):
    resolved = root.resolve()
    paths = sorted(root.rglob('*')) if root.is_dir() else [root]
    files, size = [], 0
    for path in paths:
        if path.is_symlink():
            raise ValueError('contains a symlink; replace it with a portable file')
        if not path.is_file():
            continue
        relative = path.relative_to(root).as_posix() if root.is_dir() else 'SKILL.md'
        if any(p.startswith('.') for p in Path(relative).parts):
            continue
        if root.is_dir() and not path.resolve().is_relative_to(resolved):
            raise ValueError('file escapes skill directory')
        size += path.stat().st_size
        if size > MAX_BUNDLE or len(files) >= 512:
            raise ValueError('bundle exceeds 4 MiB or 512 files')
        files.append({'path': relative, 'data': base64.b64encode(path.read_bytes()).decode(),
                      'executable': bool(path.stat().st_mode & 0o111)})
    return files


def scan(home, project=None):
    copies, errors, size = [], [], 0
    roots = [(agent, home / directory / 'skills', 'global') for agent, directory in ROOTS.items()]
    roots.append(('library', home / '.hey-boss/skills', 'global'))
    if project:
        roots += [(agent, Path(project) / directory / 'skills', 'project') for agent, directory in ROOTS.items()]
        roots.append(('project', Path(project) / 'skills', 'project'))
    for agent, directory, scope in roots:
        if not directory.is_dir():
            continue
        try:
            entries = sorted(directory.iterdir())
        except OSError as error:
            errors.append(str(error))
            continue
        for entry in entries:
            name = entry.name if entry.is_dir() else entry.stem
            if not valid_name(name) or (entry.is_dir() and not (entry / 'SKILL.md').is_file()):
                continue
            if entry.is_file() and entry.suffix != '.md':
                continue
            try:
                files = bundle(entry)
                text = next(base64.b64decode(f['data']).decode('utf-8') for f in files if f['path'] == 'SKILL.md')
                size += sum(len(f['data']) for f in files)
                if size > MAX_SCAN:
                    raise ValueError('machine inventory exceeds 64 MiB')
                copies.append({'name': name, 'agent': agent, 'scope': scope, 'path': str(entry),
                               'digest': digest(files), 'text': text, 'files': files})
            except (OSError, ValueError, StopIteration) as error:
                errors.append(f'{entry}: {error}')
    return {'copies': copies, 'errors': errors, 'hostname': __import__('socket').gethostname()}


def validate(source):
    if not valid_name(source['name']) or source.get('scope', 'global') != 'global':
        raise ValueError('Invalid global skill')
    files = source['files']
    size, names = 0, set()
    for file in files:
        path = Path(file['path'])
        if path.is_absolute() or not path.parts or any(p in ('.', '..') or p.startswith('.') for p in path.parts):
            raise ValueError('Invalid bundle path')
        if file['path'] in names:
            raise ValueError('Duplicate bundle path')
        names.add(file['path'])
        size += len(base64.b64decode(file['data'], validate=True))
    if 'SKILL.md' not in names or size > MAX_BUNDLE or len(files) > 512 or digest(files) != source['digest']:
        raise ValueError('Invalid or oversized bundle')


def install(home, sources, expected):
    # Validate the complete request before changing any destination.
    for source in sources:
        validate(source)
    current = scan(home)
    selected = {s['name']: s for s in sources}
    relevant_errors = [e for e in current['errors'] if '/skills/' not in e or any('/skills/' + name + ':' in e or '/skills/' + name + '/' in e for name in selected)]
    if relevant_errors:
        raise ValueError('Cannot safely inspect destination: ' + '; '.join(relevant_errors))
    actual = {}
    for name, source in selected.items():
        for agent in ROOTS:
            def signatures(copies):
                return {c['path']: c['digest'] for c in copies if c['name'] == name and c['agent'] == agent and c['scope'] == 'global'}
            observed, found = signatures(expected), signatures(current['copies'])
            same = bool(found) and all(value == source['digest'] for value in found.values())
            if found != observed and not same:
                raise ValueError(f'{name} on {agent} changed since scanning; scan again')
            actual[(name, agent)] = source['digest'] if same else None
    backup = home / '.hey-boss/skill-backups' / str(time.time_ns())
    for name, source in selected.items():
        for agent, directory in ROOTS.items():
            if actual.get((name, agent)) == source['digest']:
                continue
            parent = home / directory / 'skills'
            parent.mkdir(parents=True, exist_ok=True)
            destination = parent / name
            stage = Path(tempfile.mkdtemp(prefix='.skill-', dir=parent))
            old = backup / directory / name
            try:
                for file in source['files']:
                    target = stage / file['path']
                    target.parent.mkdir(parents=True, exist_ok=True)
                    target.write_bytes(base64.b64decode(file['data']))
                    target.chmod(0o755 if file['executable'] else 0o644)
                if destination.exists() or destination.is_symlink():
                    old.parent.mkdir(parents=True, exist_ok=True)
                    os.replace(destination, old)
                try:
                    os.replace(stage, destination)
                except OSError:
                    if old.exists() or old.is_symlink():
                        os.replace(old, destination)
                    raise
                flat = parent / (name + '.md')
                if flat.is_file():
                    old.parent.mkdir(parents=True, exist_ok=True)
                    os.replace(flat, old.with_suffix('.md'))
            finally:
                if stage.exists():
                    shutil.rmtree(stage)
    return scan(home)


def main():
    request = json.load(sys.stdin)
    home = Path.home()
    if request['action'] == 'scan':
        result = scan(home, request.get('project'))
    elif request['action'] == 'install':
        import fcntl
        lock = home / '.hey-boss/skill-install.lock'
        lock.parent.mkdir(parents=True, exist_ok=True)
        with lock.open('w') as handle:
            fcntl.flock(handle, fcntl.LOCK_EX)
            result = install(home, request['sources'], request['expected'])
    else:
        raise ValueError('Unknown transport operation')
    print(json.dumps(result))


if __name__ == '__main__':
    try:
        main()
    except Exception as error:
        print(str(error), file=sys.stderr)
        sys.exit(1)
