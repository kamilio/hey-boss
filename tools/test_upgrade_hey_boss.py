import contextlib
import importlib.util
import io
import json
import os
import pathlib
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('upgrade', pathlib.Path(__file__).with_name('upgrade_hey_boss.py'))
upgrade = importlib.util.module_from_spec(spec)
spec.loader.exec_module(upgrade)


class UpgradeTests(unittest.TestCase):
    def source(self, root):
        for name in upgrade.PAYLOAD:
            path = root / name
            if name in ('src', 'skills/hey-boss', 'assets', 'tests'):
                path.mkdir(parents=True)
                (path / 'fixture').write_text(name)
            else:
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text(name)
        (root / 'skills/hey-boss/SKILL.md').write_text('Canonical skill')

    def test_identity_ignores_checkout_path_and_detects_changes(self):
        with tempfile.TemporaryDirectory() as directory:
            left, right = pathlib.Path(directory) / 'left', pathlib.Path(directory) / 'right'
            self.source(left)
            self.source(right)
            self.assertEqual(upgrade.build_id(left), upgrade.build_id(right))
            (right / 'tools/upgrade_hey_boss.py').write_text('Changed updater')
            self.assertNotEqual(upgrade.build_id(left), upgrade.build_id(right))

    def test_unreachable_host_does_not_stop_remaining_rollout(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            source, binary = root / 'source', root / 'hey-boss'
            self.source(source)
            binary.write_text('old')
            output = io.StringIO()
            argv = ['upgrade', '--source', str(source), '--host', 'offline', '--host', 'online', '--json']
            with patch.dict(os.environ, HOME=directory, HEY_BOSS_UPGRADE_BINARY=str(binary)), patch('sys.argv', argv), patch.object(upgrade, 'apply') as local, patch.object(upgrade, 'remote_apply') as remote, contextlib.redirect_stdout(output):
                def installed(binary, ssh=None):
                    if ssh and ssh[-1] == 'offline':
                        raise RuntimeError('unreachable')
                    return upgrade.build_id(source) if ssh and remote.called else None
                with patch.object(upgrade, 'installed_id', side_effect=installed):
                    self.assertEqual(upgrade.main(), 1)
            local.assert_called_once()
            self.assertEqual(remote.call_args.args[1], 'online')
            self.assertEqual([m['status'] for m in json.loads(output.getvalue())['machines']], ['updated', 'failed', 'updated'])

    def test_check_never_installs_or_saves_source(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            self.source(root / 'source')
            binary = root / 'hey-boss'
            binary.write_text('old')
            argv = ['upgrade', '--source', str(root / 'source'), '--check', '--local-only']
            with patch.dict(os.environ, HOME=directory, HEY_BOSS_UPGRADE_BINARY=str(binary)), patch('sys.argv', argv), patch.object(upgrade, 'installed_id', return_value=None), patch.object(upgrade, 'apply') as install, contextlib.redirect_stdout(io.StringIO()):
                self.assertEqual(upgrade.main(), 2)
            install.assert_not_called()
            self.assertFalse((root / '.local/share/hey-boss/upgrade-source').exists())

    def test_matching_build_skips_rebuild(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            source, binary = root / 'source', root / 'hey-boss'
            self.source(source)
            binary.write_text('current')
            with patch.dict(os.environ, HOME=directory, HEY_BOSS_UPGRADE_BINARY=str(binary)), patch('sys.argv', ['upgrade', '--source', str(source), '--local-only']), patch.object(upgrade, 'installed_id', return_value=upgrade.build_id(source)), patch.object(upgrade, 'apply') as install, contextlib.redirect_stdout(io.StringIO()):
                self.assertEqual(upgrade.main(), 0)
            install.assert_not_called()

    def test_post_install_failure_restores_previous_binary(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            source, binary = root / 'source', root / 'bin/hey-boss'
            self.source(source)
            binary.parent.mkdir()
            binary.write_text('previous binary')
            built = root / '.cache/hey-boss/build/release/hey-boss'
            built.parent.mkdir(parents=True)
            built.write_text('replacement binary')
            with patch.dict(os.environ, HOME=directory), patch.object(upgrade, 'run'), patch.object(upgrade, 'desktop_app', return_value=None), patch.object(upgrade, 'installed_id', side_effect=['1234567890abcdef', 'bad']):
                with self.assertRaisesRegex(RuntimeError, 'Installed CLI failed'):
                    upgrade.apply(source, binary, '1234567890abcdef')
            self.assertEqual(binary.read_text(), 'previous binary')

    def test_build_failure_preserves_installed_binary(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            binary = root / 'hey-boss'
            binary.write_text('previous binary')
            with patch.dict(os.environ, HOME=directory), patch.object(upgrade, 'run', side_effect=RuntimeError('compile failed')):
                with self.assertRaisesRegex(RuntimeError, 'compile failed'):
                    upgrade.apply(root, binary, '1234567890abcdef')
            self.assertEqual(binary.read_text(), 'previous binary')

    def test_ssh_host_cannot_inject_options(self):
        for host in ('-oProxyCommand=bad', 'devbox;touch bad', 'devbox bad', ''):
            with self.assertRaises(RuntimeError):
                upgrade.ssh_command(host)
        self.assertEqual(upgrade.ssh_command('user@devbox')[-1], 'user@devbox')


if __name__ == '__main__':
    unittest.main()
