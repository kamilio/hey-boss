#!/usr/bin/env python3
"""Source-based fleet upgrades. Embedded in the CLI; requires Python 3 and Rust."""
import argparse
import fcntl
import io
import json
import os
import pathlib
import plistlib
import re
import shlex
import shutil
import subprocess
import sys
import tarfile
import tempfile
import time

IDENTITY = ('Cargo.toml', 'Cargo.lock', 'build.rs', 'src', 'worker-tui/Cargo.toml', 'worker-tui/src', 'skills/hey-boss',
            'tools/upgrade_hey_boss.py', 'tools/fleet_hey_boss.py', 'tools/drain_github_issues.py', 'hey_boss_daemon.swift',
            'package_hey_boss.swift', 'setup_hey_boss.swift', 'assets')
PAYLOAD = IDENTITY + ('tests', 'README.md', 'LICENSE')
UPSTREAM = 'https://github.com/kamilio/hey-boss.git'


def build_id(source):
    files = []
    for name in IDENTITY:
        path = source / name
        if path.is_dir():
            files.extend(p for p in path.rglob('*') if p.is_file())
        elif path.is_file():
            files.append(path)
        else:
            raise RuntimeError('Missing upgrade source: ' + name)
    value = 0xcbf29ce484222325
    for path in sorted(files, key=lambda p: p.relative_to(source).as_posix()):
        data = path.relative_to(source).as_posix().encode() + b'\0' + path.read_bytes() + b'\0'
        for byte in data:
            value = ((value ^ byte) * 0x100000001b3) & 0xffffffffffffffff
    return format(value, '016x')


def run(command, **kwargs):
    result = subprocess.run(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE, **kwargs)
    if result.returncode:
        detail = result.stderr.decode(errors='replace')[-6000:]
        raise RuntimeError('Command failed: ' + shlex.join(map(str, command[:3])) + '\n' + detail)
    return result.stdout


def installed_id(binary, ssh=None):
    command = [str(binary), '--version']
    if ssh:
        command = ssh + ["\"$HOME/.local/bin/hey-boss\" --version"]
    try:
        output = run(command, timeout=30).decode(errors='replace')
    except (OSError, subprocess.TimeoutExpired, RuntimeError):
        if ssh:
            # Distinguish a reachable host with an old/missing CLI from SSH failure.
            run(ssh + ['true'], timeout=30)
        return None
    match = re.search(r'\(build ([0-9a-f]{16})\)', output)
    return match.group(1) if match else None


def atomic_copy(source, destination, mode=0o755):
    destination.parent.mkdir(parents=True, exist_ok=True)
    fd, temporary = tempfile.mkstemp(prefix='.hey-boss-upgrade-', dir=destination.parent)
    os.close(fd)
    try:
        shutil.copy2(source, temporary)
        os.chmod(temporary, mode)
        os.replace(temporary, destination)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)


def install_shortcut(binary):
    shortcut = binary.with_name('hb')
    try:
        shortcut.symlink_to(binary.name)
    except FileExistsError:
        # Preserve other commands, including dangling symlinks.
        pass


def desktop_app():
    plist = pathlib.Path.home() / 'Library/LaunchAgents/local.hey-boss.plist'
    if sys.platform != 'darwin' or not plist.exists():
        return None
    arguments = plistlib.loads(plist.read_bytes()).get('ProgramArguments', [])
    if not arguments:
        return None
    daemon = pathlib.Path(arguments[0])
    if daemon.parent.name != 'MacOS' or daemon.parents[2].suffix != '.app':
        raise RuntimeError('Desktop installation needs reinstalling before upgrading')
    return daemon.parents[2]


def apply(source, binary, expected):
    state = pathlib.Path.home() / '.local/share/hey-boss'
    state.mkdir(parents=True, exist_ok=True)
    with (state / 'upgrade.lock').open('a') as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            raise RuntimeError('Another upgrade is already running on this machine')
        environment = os.environ.copy()
        environment['PATH'] = str(pathlib.Path.home() / '.cargo/bin') + ':/opt/homebrew/bin:' + environment.get('PATH', '')
        cache = pathlib.Path.home() / '.cache/hey-boss/build'
        environment['CARGO_TARGET_DIR'] = str(cache)
        run(['cargo', 'build', '--quiet', '--locked', '--release', '--manifest-path', str(source / 'Cargo.toml')], env=environment)
        built = cache / 'release/hey-boss'
        if installed_id(built) != expected:
            raise RuntimeError('Built CLI does not match the source snapshot')
        companion = binary.with_name('hey-boss.companion').exists()
        app = None if companion else desktop_app()
        with tempfile.TemporaryDirectory(prefix='hey-boss-desktop-') as directory:
            directory = pathlib.Path(directory)
            staged_app = directory / 'Hey Boss.app'
            if app:
                run(['/usr/bin/xcrun', 'swiftc', '-O', '-whole-module-optimization', '-parse-as-library',
                     str(source / 'hey_boss_daemon.swift'), '-o', str(directory / 'daemon')])
                run(['/usr/bin/swift', str(source / 'package_hey_boss.swift'), str(directory / 'daemon'), str(staged_app)])
                run(['/usr/bin/codesign', '--verify', '--strict', str(staged_app)])
            backup = state / 'upgrade-backups'
            backup.mkdir(mode=0o700, exist_ok=True)
            previous = backup / 'hey-boss.previous'
            shutil.copy2(binary, previous)
            app_backup = app.with_name(app.name + '.upgrade-previous') if app else None
            if app_backup and app_backup.exists():
                raise RuntimeError('Previous desktop rollback exists: ' + str(app_backup))
            replaced_app = False
            try:
                atomic_copy(built, binary)
                if app:
                    # Stage on the destination filesystem before swapping directories.
                    adjacent = app.with_name(app.name + '.upgrade-new')
                    if adjacent.exists():
                        raise RuntimeError('Desktop upgrade staging already exists: ' + str(adjacent))
                    shutil.copytree(staged_app, adjacent)
                    os.rename(app, app_backup)
                    try:
                        os.rename(adjacent, app)
                    except BaseException:
                        os.rename(app_backup, app)
                        raise
                    replaced_app = True
                    run(['/bin/launchctl', 'kickstart', '-k', 'gui/' + str(os.getuid()) + '/local.hey-boss'])
                    for attempt in range(10):
                        try:
                            run([str(binary), 'overview', '--json'], timeout=15)
                            break
                        except (RuntimeError, subprocess.TimeoutExpired):
                            if attempt == 9:
                                raise
                            time.sleep(1)
                elif companion:
                    if sys.platform == 'darwin':
                        run(['/bin/launchctl', 'kickstart', '-k', 'gui/' + str(os.getuid()) + '/local.hey-boss-broker'])
                    elif shutil.which('systemctl') and subprocess.run(['systemctl', '--user', 'is-active', '--quiet', 'hey-boss-companion.service']).returncode == 0:
                        run(['systemctl', '--user', 'restart', 'hey-boss-companion.service'])
                        run(['systemctl', '--user', 'is-active', '--quiet', 'hey-boss-companion.service'])
                if installed_id(binary) != expected:
                    raise RuntimeError('Installed CLI failed build verification')
                install_shortcut(binary)
                for root in ('.codex', '.agents', '.claude'):
                    skill = pathlib.Path.home() / root / 'skills/hey-boss/SKILL.md'
                    atomic_copy(source / 'skills/hey-boss/SKILL.md', skill, 0o644)
            except BaseException:
                atomic_copy(previous, binary)
                if replaced_app:
                    shutil.rmtree(app)
                    os.rename(app_backup, app)
                    subprocess.run(['/bin/launchctl', 'kickstart', '-k', 'gui/' + str(os.getuid()) + '/local.hey-boss'], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
                elif companion:
                    restart = ['/bin/launchctl', 'kickstart', '-k', 'gui/' + str(os.getuid()) + '/local.hey-boss-broker'] if sys.platform == 'darwin' else ['systemctl', '--user', 'try-restart', 'hey-boss-companion.service']
                    subprocess.run(restart, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
                raise
            if app_backup:
                shutil.rmtree(app_backup)


def ssh_command(host):
    if not re.fullmatch(r'[a-zA-Z0-9@._:\[\]-]+', host) or host.startswith('-'):
        raise RuntimeError('Invalid SSH host: ' + host)
    return ['ssh', '-T', '-o', 'BatchMode=yes', '-o', 'ConnectTimeout=8',
            '-o', 'ServerAliveInterval=15', '-o', 'ServerAliveCountMax=3', host]


def remote_apply(source, host, expected):
    archive = io.BytesIO()
    with tarfile.open(fileobj=archive, mode='w:gz', format=tarfile.USTAR_FORMAT) as tar:
        for name in PAYLOAD:
            tar.add(source / name, arcname=name)
    script = '''import pathlib,sys,tarfile,tempfile,subprocess
with tempfile.TemporaryDirectory(prefix="hey-boss-upgrade-") as stage:
    with tarfile.open(fileobj=sys.stdin.buffer,mode="r|gz") as tar:
        for member in tar:
            path=pathlib.Path(stage)/member.name
            if member.name.startswith("/") or ".." in pathlib.PurePosixPath(member.name).parts: raise RuntimeError("Invalid archive path")
            if member.isdir(): path.mkdir(parents=True,exist_ok=True)
            elif member.isfile():
                path.parent.mkdir(parents=True,exist_ok=True)
                path.write_bytes(tar.extractfile(member).read())
            else: raise RuntimeError("Unsupported archive entry")
    binary=pathlib.Path.home()/".local/bin/hey-boss"
    result=subprocess.run([sys.executable,str(pathlib.Path(stage)/"tools/upgrade_hey_boss.py"),"--apply", "--source",stage,"--binary",str(binary),"--expected",sys.argv[1]])
    sys.exit(result.returncode)
'''
    environment = os.environ.copy()
    environment['SFT_NO_BROWSER'] = '1'
    run(ssh_command(host) + ['python3 -c ' + shlex.quote(script) + ' ' + shlex.quote(expected)], input=archive.getvalue(), env=environment)


def source_checkout(args, state):
    saved = state / 'upgrade-source'
    if args.source:
        return args.source.resolve()
    if saved.exists():
        return pathlib.Path(saved.read_text().strip()).resolve()
    cache = state / 'upgrade-checkout'
    if not cache.exists():
        run(['git', 'clone', '--depth', '1', UPSTREAM, str(cache)])
    else:
        run(['git', '-C', str(cache), 'fetch', '--depth', '1', 'origin', 'main'])
        run(['git', '-C', str(cache), 'checkout', '--detach', 'FETCH_HEAD'])
    return cache


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--source', type=pathlib.Path)
    parser.add_argument('--check', action='store_true')
    parser.add_argument('--force', action='store_true')
    parser.add_argument('--local-only', action='store_true')
    parser.add_argument('--host', action='append', default=[])
    parser.add_argument('--json', action='store_true')
    parser.add_argument('--apply', action='store_true', help=argparse.SUPPRESS)
    parser.add_argument('--binary', type=pathlib.Path, help=argparse.SUPPRESS)
    parser.add_argument('--expected', help=argparse.SUPPRESS)
    args = parser.parse_args()
    os.environ.setdefault('SFT_NO_BROWSER', '1')
    binary = (args.binary or pathlib.Path(os.environ['HEY_BOSS_UPGRADE_BINARY'])).resolve()
    if args.apply:
        if build_id(args.source) != args.expected:
            raise RuntimeError('Received source does not match the expected build')
        apply(args.source, binary, args.expected)
        return 0
    state = pathlib.Path.home() / '.local/share/hey-boss'
    state.mkdir(parents=True, exist_ok=True)
    source = source_checkout(args, state)
    registry = state / 'companion-hosts'
    hosts = [] if args.local_only else (args.host or (registry.read_text().splitlines() if registry.exists() else []))
    hosts = list(dict.fromkeys(hosts))
    report = []
    with tempfile.TemporaryDirectory(prefix='hey-boss-upgrade-') as temporary:
        snapshot = pathlib.Path(temporary)
        for name in PAYLOAD:
            path = source / name
            target = snapshot / name
            target.parent.mkdir(parents=True, exist_ok=True)
            if path.is_dir():
                shutil.copytree(path, target)
            else:
                shutil.copy2(path, target)
        expected = build_id(snapshot)
        for host in ['local'] + hosts:
            entry = {'host': host, 'build': expected}
            try:
                current = installed_id(binary, None if host == 'local' else ssh_command(host))
                entry['installed_build'] = current
                if current == expected and not args.force:
                    entry['status'] = 'current'
                elif args.check:
                    entry['status'] = 'outdated'
                else:
                    if not args.json:
                        print(host + ': building and installing ' + expected, flush=True)
                    if host == 'local':
                        apply(snapshot, binary, expected)
                    else:
                        remote_apply(snapshot, host, expected)
                        if installed_id(binary, ssh_command(host)) != expected:
                            raise RuntimeError('Remote build verification failed')
                    entry['status'] = 'updated'
            except (OSError, RuntimeError, subprocess.TimeoutExpired) as error:
                entry.update(status='failed', error=str(error))
            report.append(entry)
            if not args.json:
                print(host + ': ' + entry['status'] + (' — ' + entry['error'] if 'error' in entry else ''), flush=True)
        if args.source and not args.check and report[0]['status'] != 'failed':
            saved = state / 'upgrade-source'
            saved.write_text(str(source) + '\n')
        if args.json:
            print(json.dumps({'build': expected, 'machines': report}))
    if any(entry['status'] == 'failed' for entry in report):
        return 1
    return 2 if args.check and any(entry['status'] == 'outdated' for entry in report) else 0


if __name__ == '__main__':
    try:
        sys.exit(main())
    except (OSError, RuntimeError, subprocess.TimeoutExpired) as error:
        print('Upgrade: ' + str(error), file=sys.stderr)
        sys.exit(1)
