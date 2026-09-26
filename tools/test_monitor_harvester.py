import importlib.util
import json
import pathlib
import os
import signal
import subprocess
import sys
import tempfile
import time
import unittest
from unittest import mock

spec = importlib.util.spec_from_file_location(
    "monitor", pathlib.Path(__file__).with_name("monitor_harvester.py")
)
monitor = importlib.util.module_from_spec(spec)
spec.loader.exec_module(monitor)


class MonitorTests(unittest.TestCase):
    def test_known_system_workload_denial_keeps_protection_and_diagnostics(self):
        error = ('Protected workload · PID 68382: Cannot verify workload ownership; '
                 'preserved: /private/var/db/analyticsd/events.allowlist: '
                 'Permission denied (os error 13)')
        sample = self.sample()
        sample['binary_sha256'] = 'fixture'
        sample['snapshot']['errors'] = [error]
        row = monitor.compact('local', sample, 2000)
        self.assertEqual(row['problems'], [])
        self.assertEqual(row['errors'], [error])
        self.assertEqual(row['warnings'], ['protected_os_workload'])
        sample['snapshot']['errors'] = [error, '/project: Input/output error (os error 5)']
        self.assertIn('cleanup_errors', monitor.problems(sample, 2000))
        for unexpected in [
            error.replace('/private/var/db/analyticsd/events.allowlist', '/project/secret'),
            error.replace('Permission denied (os error 13)', 'Input/output error (os error 5)'),
            error.replace('/private/var/db/analyticsd/events.allowlist: ', ''),
            error + '; /project: Permission denied (os error 13)',
        ]:
            sample['snapshot']['errors'] = [unexpected]
            self.assertIn('cleanup_errors', monitor.problems(sample, 2000))

    def test_routine_disconnects_stay_visible_without_paging_and_low_disk_still_alerts(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            (root / 'monitor-state.json').write_text(json.dumps({'devbox': {
                'problems': ['unreachable'], 'since': 1, 'last_alert': 1,
                'completed_through': 1950}}))
            disk = self.sample()
            disk['machine_time'] = 2000
            disk['binary_sha256'] = 'fixture'
            disk['snapshot']['metrics']['disk_available_bytes'] = 10_000_000_000
            notify = mock.Mock(return_value=mock.Mock(returncode=0))
            args = ['monitor', '--directory', directory, '--host', 'devbox',
                    '--notify', '/fixture/hey-boss']
            with mock.patch('sys.argv', args), mock.patch('builtins.print'), \
                    mock.patch.object(monitor, 'probe', side_effect=[{'error': 'offline'}] * 3 + [disk]), \
                    mock.patch.object(monitor.time, 'time', side_effect=[30000, 30300, 60000, 60300]), \
                    mock.patch.object(monitor.subprocess, 'run', notify):
                for _ in range(3):
                    monitor.main()
                notify.assert_not_called()
                latest = json.loads((root / 'latest-summary.json').read_text())[0]
                self.assertEqual(latest['problems'], ['unreachable'])
                self.assertNotIn('metrics', latest)
                self.assertEqual(json.loads((root / 'monitor-state.json').read_text())['devbox']['completed_through'], 1950)
                monitor.main()
                notify.assert_called_once()
                self.assertIn('disk_low', notify.call_args.args[0][-1])
                self.assertNotIn('unreachable', notify.call_args.args[0][-1])

    def test_remote_probe_reuses_only_its_configured_control_socket(self):
        run = mock.Mock(return_value=mock.Mock(returncode=0, stdout='{}'))
        control = pathlib.Path('/tmp/fixture ssh.sock')
        with mock.patch.object(monitor, 'run_probe', run):
            self.assertEqual(monitor.probe('devbox', control), {})
        args = run.call_args.args[0]
        self.assertEqual(args[args.index('-S') + 1], str(control))
        self.assertEqual(args[-2], 'devbox')

    def test_pinned_probe_never_starts_a_replacement_authentication_proxy(self):
        run = mock.Mock(return_value=mock.Mock(returncode=0, stdout='{}'))
        with mock.patch.object(monitor, 'run_probe', run):
            self.assertEqual(monitor.probe('devbox', pathlib.Path('/tmp/gone.sock')), {})
        args = run.call_args.args[0]
        self.assertIn('ProxyCommand=false', args)
        self.assertIn('ProxyJump=none', args)
        self.assertIn('ControlMaster=no', args)

    def test_probe_timeout_reaps_a_proxy_that_outlives_its_ssh_parent(self):
        self.check_proxy_cleanup(timeout=True)

    def test_failed_probe_reaps_a_proxy_with_closed_output_pipes(self):
        self.check_proxy_cleanup(timeout=False)

    def check_proxy_cleanup(self, timeout):
        with tempfile.TemporaryDirectory() as directory:
            receipt = pathlib.Path(directory) / 'proxy.json'
            proxy = ('import os,json,time,signal; from pathlib import Path; '
                     'signal.signal(signal.SIGTERM,signal.SIG_IGN); '
                     f'Path({str(receipt)!r}).write_text(json.dumps([os.getpid(),os.getpgrp()])); '
                     'time.sleep(30)')
            launcher = (f'import subprocess,sys,time,signal; from pathlib import Path; '
                        'signal.signal(signal.SIGTERM,signal.SIG_IGN); '
                        f'subprocess.Popen([sys.executable,"-c",{proxy!r}]'
                        + (')' if timeout else ',stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)'))
            if not timeout:
                launcher += (f'\nfor _ in range(200):\n'
                             f' if Path({str(receipt)!r}).exists(): break\n'
                             ' time.sleep(0.01)\nsys.exit(1)')
            try:
                started = time.monotonic()
                if timeout:
                    with self.assertRaises(subprocess.TimeoutExpired):
                        monitor.run_probe([sys.executable, '-c', launcher], timeout=2)
                else:
                    self.assertEqual(monitor.run_probe([sys.executable, '-c', launcher]).returncode, 1)
                self.assertLess(time.monotonic()-started, 5)
                pid, group = json.loads(receipt.read_text())
                self.assertNotEqual(group, os.getpgrp())
                for _ in range(50):
                    status = subprocess.run(['ps', '-p', str(pid), '-o', 'stat='],
                                            capture_output=True, text=True).stdout.strip()
                    if not status or status.startswith('Z'):
                        break
                    time.sleep(0.02)
                else:
                    self.fail('Timed-out authentication proxy is still running')
            finally:
                if receipt.exists():
                    _, group = json.loads(receipt.read_text())
                    try:
                        os.killpg(group, signal.SIGKILL)
                    except ProcessLookupError:
                        pass

    def test_local_probe_does_not_use_ssh_control(self):
        run = mock.Mock(return_value=mock.Mock(returncode=0, stdout='{}'))
        with mock.patch.object(monitor, 'run_probe', run):
            self.assertEqual(monitor.probe('local', pathlib.Path('/tmp/unused.sock')), {})
        self.assertEqual(run.call_args.args[0][:2], ['python3', '-c'])

    def sample(self):
        return {"snapshot": {
            "observed_at": 1900, "last_cleanup_at": 1900,
            "running": False, "config": {
                "automatic": True, "aggressive": True, "harvest_processes": True,
                "clean_worktrees": True, "clean_caches": True, "trim_worker_logs": True,
            },
            "metrics": {"disk_available_bytes": 100_000_000_000,
                        "memory_pressure": "Warning"},
            "cache_progress": {}, "errors": [],
        }, "scheduler": {"returncode": 0}}

    def test_waiting_between_cycles_is_healthy(self):
        self.assertEqual(monitor.problems(self.sample(), 2000), [])

    def test_recent_observation_does_not_hide_stalled_cleanup(self):
        sample = self.sample()
        sample["snapshot"].update(observed_at=1999, last_cleanup_at=1000, running=True)
        self.assertIn("cleanup_stale", monitor.problems(sample, 2000))

    def test_failed_connection_is_not_replaced_by_old_success(self):
        self.assertEqual(monitor.problems({"error": "timeout"}, 2000), ["unreachable"])

    def test_missing_telemetry_and_scheduler_are_visible(self):
        sample = self.sample()
        del sample["snapshot"]["cache_progress"]
        sample["scheduler"]["returncode"] = 1
        self.assertEqual(monitor.problems(sample, 2000), ["scheduler_unavailable", "cache_telemetry_missing"])

    def test_resource_thresholds_and_disabled_cleanup(self):
        sample = self.sample()
        sample["snapshot"]["metrics"].update(disk_available_bytes=20_000_000_000, memory_pressure="Critical")
        sample["snapshot"]["config"]["automatic"] = False
        self.assertEqual(monitor.problems(sample, 2000), ["cleanup_disabled", "disk_low", "memory_critical"])

    def test_independent_cleaners_cannot_silently_stop(self):
        for setting, problem in [
            ("harvest_processes", "process_cleanup_disabled"),
            ("clean_worktrees", "worktree_cleanup_disabled"),
            ("clean_caches", "cache_cleanup_disabled"),
            ("trim_worker_logs", "worker_log_cleanup_disabled"),
            ("aggressive", "aggressive_cleanup_disabled"),
        ]:
            for missing in [False, True]:
                with self.subTest(setting=setting, missing=missing):
                    sample = self.sample()
                    sample["binary_sha256"] = "fixture"
                    if missing:
                        del sample["snapshot"]["config"][setting]
                    else:
                        sample["snapshot"]["config"][setting] = False
                    row = monitor.compact("host", sample, 2000)
                    self.assertEqual(row["problems"], [problem])
                    self.assertIs(row["cleanup_settings"][setting], None if missing else False)
                    sample["snapshot"]["config"][setting] = True
                    self.assertEqual(monitor.problems(sample, 2000), [])

    def test_running_cycles_do_not_hide_a_cache_pass_over_a_day(self):
        sample = self.sample()
        sample["snapshot"]["last_cleanup_at"] = 100000
        sample["snapshot"]["cache_progress"] = {"pass_started_at": 1, "discovery_pending": True}
        self.assertIn("cache_pass_overdue", monitor.problems(sample, 100000))
        sample["snapshot"]["cache_progress"]["discovery_pending"] = False
        self.assertNotIn("cache_pass_overdue", monitor.problems(sample, 100000))

    def test_known_os_cache_denials_stay_visible_without_paging(self):
        sample = self.sample()
        sample["binary_sha256"] = "fixture"
        errors = ["24-hour cache expiration: /Users/test/Library/Caches/FamilyCircle: Operation not permitted (os error 1); /private/var/folders/dd/test_user/T/com.apple.CloudDocs.iCloudDriveFileProvider/TemporaryItems: Operation not permitted (os error 1)"]
        sample["snapshot"]["errors"] = errors
        row = monitor.compact("mac", sample, 2000)
        self.assertEqual(row["problems"], [])
        self.assertEqual(row["warnings"], ["protected_os_cache"])
        self.assertEqual(row["errors"], errors)

    def test_unexpected_and_mixed_failures_still_page(self):
        known = "24-hour cache expiration: /Users/test/Library/Caches/FamilyCircle: Operation not permitted (os error 1)"
        for error in [
            "/Users/test/Workspace/project/.cache: Operation not permitted (os error 1)",
            "/Users/test/Library/Caches/CustomApp: Operation not permitted (os error 1)",
            "/private/var/folders/dd/test_user/T/com.example.unknown/TemporaryItems: Operation not permitted (os error 1)",
            "/Users/test/Library/Caches/FamilyCircle: Input/output error (os error 5)",
            known + "; /Users/test/Workspace/project/.cache: Input/output error (os error 5)",
        ]:
            with self.subTest(error=error):
                sample = self.sample()
                sample["snapshot"]["errors"] = [error]
                self.assertIn("cleanup_errors", monitor.problems(sample, 2000))

    def test_completed_full_pass_apple_denials_remain_visible_as_warnings(self):
        paths = [
            "/private/var/folders/dd/test_user/T/com.apple.appleaccountd/TemporaryItems",
            "/private/var/folders/dd/test_user/T/com.apple.replayd/TemporaryItems",
            "/Users/test/Library/Caches/com.apple.HomeKit",
            "/Users/test/Library/Caches/CloudKit",
            "/Users/test/Library/Caches/com.apple.Safari",
            "/Users/test/Library/Caches/com.apple.findmy.imagecache",
            "/Users/test/Library/Caches/com.apple.findmy.fmfcore",
            "/Users/test/Library/Caches/com.apple.containermanagerd",
            "/private/var/folders/dd/test_user/T/com.apple.syncdefaultsd/TemporaryItems",
            "/private/var/folders/dd/test_user/T/com.apple.amsengagementd/TemporaryItems",
            "/private/var/folders/dd/test_user/T/com.apple.icloud.searchpartyuseragent/TemporaryItems",
        ]
        error = "24-hour cache expiration: " + "; ".join(
            path + ": Operation not permitted (os error 1)" for path in paths)
        row = monitor.compact("mac", self.completed_sample([error]), 2000)
        self.assertEqual(row["problems"], [])
        self.assertEqual(row["warnings"], ["protected_os_cache"])
        self.assertEqual(row["last_completed_errors"], [error])

    def test_os_temporary_items_privacy_denial_is_narrow_and_visible(self):
        path = "/private/var/folders/dd/test_user/T/TemporaryItems"
        error = "24-hour cache expiration: " + path + ": Operation not permitted (os error 1)"
        row = monitor.compact("mac", self.completed_sample([error]), 2000)
        self.assertEqual(row["problems"], [])
        self.assertEqual(row["warnings"], ["protected_os_cache"])
        self.assertEqual(row["last_completed_errors"], [error])
        for unexpected in [
            error.replace("T/TemporaryItems", "T/project/TemporaryItems"),
            error.replace(path, "/private/tmp/TemporaryItems"),
            error.replace("Operation not permitted (os error 1)", "Input/output error (os error 5)"),
            error + "; /Users/test/Workspace/project: Permission denied (os error 13)",
        ]:
            with self.subTest(error=unexpected):
                sample = self.completed_sample([unexpected])
                self.assertIn("cleanup_errors", monitor.problems(sample, 2000))

    def test_apple_temporary_items_namespace_handles_new_services_without_hiding_other_failures(self):
        prefix = "/private/var/folders/dd/test_user/T/"
        for service in ["com.apple.quicklook.qlmanage", "com.apple.future-service", "homed", "duetexpertd", "icdd"]:
            error = prefix + service + "/TemporaryItems: Operation not permitted (os error 1)"
            row = monitor.compact("mac", self.completed_sample([error]), 2000)
            self.assertEqual(row["problems"], [])
            self.assertEqual(row["warnings"], ["protected_os_cache"])
            self.assertEqual(row["last_completed_errors"], [error])
        for path in [
            prefix + "com.example.service/TemporaryItems",
            prefix + "com.apple-impostor/TemporaryItems",
            prefix + "com.apple./TemporaryItems",
            prefix + "com.apple.service/project-data",
            "/private/tmp/com.apple.service/TemporaryItems",
            "/Users/test/Workspace/com.apple.service/TemporaryItems",
            prefix + "icdd-copy/TemporaryItems",
            prefix + "icdd/TemporaryItems/project",
            "/Users/test/Workspace/icdd/TemporaryItems",
        ]:
            self.assertFalse(monitor.expected_access_denial(path + ": Operation not permitted (os error 1)"))
        error = prefix + "com.apple.quicklook.qlmanage/TemporaryItems: Operation not permitted (os error 1)"
        for unexpected in [
            error.replace("Operation not permitted (os error 1)", "Permission denied (os error 13)"),
            error.replace("Operation not permitted (os error 1)", "Input/output error (os error 5)"),
            error + "; /project: Operation not permitted (os error 1)",
        ]:
            self.assertFalse(monitor.expected_access_denial(unexpected))

    def test_apple_cache_bundle_privacy_denials_remain_visible(self):
        prefix = "/Users/test/Library/Caches/"
        for bundle in ["com.apple.Safari.SafeBrowsing", "com.apple.FutureKit"]:
            error = prefix + bundle + ": Operation not permitted (os error 1)"
            row = monitor.compact("mac", self.completed_sample([error]), 2000)
            self.assertEqual(row["problems"], [])
            self.assertEqual(row["warnings"], ["protected_os_cache"])
            self.assertEqual(row["last_completed_errors"], [error])
        for path in [prefix + "com.example.cache", prefix + "com.apple-impostor",
                     prefix + "com.apple.Safari/project-data",
                     "/Users/test/Workspace/com.apple.Safari.SafeBrowsing"]:
            self.assertFalse(monitor.expected_access_denial(path + ": Operation not permitted (os error 1)"))
        error = prefix + "com.apple.Safari.SafeBrowsing: Operation not permitted (os error 1)"
        self.assertFalse(monitor.expected_access_denial(error.replace("Operation not permitted (os error 1)", "Input/output error (os error 5)")))
        self.assertFalse(monitor.expected_access_denial(error + "; /project: Operation not permitted (os error 1)"))

    def test_newly_observed_apple_privacy_paths_remain_visible(self):
        paths = [
            "/private/var/folders/dd/test_user/T/com.apple.transparencyd/TemporaryItems",
            "/private/var/folders/dd/test_user/T/com.apple.triald/TemporaryItems",
            "/private/var/folders/dd/test_user/T/com.apple.ap.promotedcontentd/TemporaryItems",
            "/private/var/folders/dd/test_user/T/homed/TemporaryItems",
            "/private/var/folders/dd/test_user/T/com.apple.pluginkit/TemporaryItems",
            "/private/var/folders/dd/test_user/T/com.apple.donotdisturbd/TemporaryItems",
            "/private/var/folders/dd/test_user/T/com.apple.securityuploadd/TemporaryItems",
            "/private/var/folders/dd/test_user/T/com.apple.appstoreagent/TemporaryItems",
            "/private/var/folders/dd/test_user/T/com.apple.imtransferservices.IMTransferAgent/TemporaryItems",
            "/Users/test/Library/Caches/com.apple.homed",
            "/Users/test/Library/Caches/com.apple.findmy.fmipcore",
            "/Users/test/Library/Caches/com.apple.ap.adprivacyd",
        ]
        error = "24-hour cache expiration: " + "; ".join(
            path + ": Operation not permitted (os error 1)" for path in paths)
        row = monitor.compact("mac", self.completed_sample([error]), 2000)
        self.assertEqual(row["problems"], [])
        self.assertEqual(row["warnings"], ["protected_os_cache"])
        self.assertEqual(row["last_completed_errors"], [error])

    def completed_sample(self, errors, count=None):
        sample = self.sample()
        sample["binary_sha256"] = "fixture"
        sample["snapshot"].update(running=True, errors=[], activity=[
            {"at": 1800, "category": "scan", "message": "Cleanup check started"},
            *[{"at": 1850, "category": "error", "message": e} for e in errors],
            {"at": 1850, "category": "scan", "message":
             f"Finished: 0 worktrees checked; 0 processes stopped; 0 worktrees removed; 1 caches removed; {len(errors) if count is None else count} errors."},
            {"at": 1900, "category": "scan", "message": "Cleanup check started"},
        ])
        return sample

    def test_running_cycle_retains_previous_completed_failure(self):
        sample = self.completed_sample(["Worktree cleaner: Inspection command timed out"])
        row = monitor.compact("mac", sample, 2000)
        self.assertIn("cleanup_errors", row["problems"])
        self.assertEqual(row["last_completed_errors"], ["Worktree cleaner: Inspection command timed out"])

    def test_completed_os_denials_remain_warnings(self):
        sample = self.completed_sample([
            "24-hour cache expiration: /Users/test/Library/Caches/FamilyCircle: Operation not permitted (os error 1)"])
        row = monitor.compact("mac", sample, 2000)
        self.assertEqual(row["problems"], [])
        self.assertEqual(row["warnings"], ["protected_os_cache"])

    def test_missing_completed_error_detail_is_not_success(self):
        sample = self.completed_sample([], count=1)
        self.assertIn("cleanup_errors", monitor.problems(sample, 2000))

    def test_successful_completed_cycle_clears_historical_failure(self):
        sample = self.completed_sample(["Worktree cleaner: old failure"])
        sample["snapshot"]["activity"] += [
            {"at": 1950, "category": "scan", "message": "Finished: 1 worktrees checked; 0 processes stopped; 0 worktrees removed; 0 caches removed; 0 errors."},
            {"at": 1990, "category": "scan", "message": "Cleanup check started"},
        ]
        self.assertEqual(monitor.problems(sample, 2000), [])

    def test_failure_between_samples_is_saved_after_later_success(self):
        error = "Worktree cleaner: Inspection command timed out"
        sample = self.completed_sample([error])
        sample["snapshot"]["activity"].append({
            "at": 1950, "category": "scan", "message":
            "Finished: 1 worktrees checked; 0 processes stopped; 0 worktrees removed; 0 caches removed; 0 errors."})
        row = monitor.compact("host", sample, 2000)
        self.assertEqual(row["last_completed_errors"], [])
        self.assertIn("cleanup_errors", row["problems"])
        self.assertEqual(row["new_completed_errors"], [error])
        self.assertEqual(row["completed_cycles"][0]["errors"], [error])
        self.assertEqual(row["completed_through"], 1950)
        following = monitor.compact("host", sample, 2300,
                                    completed_after=row["completed_through"])
        self.assertEqual(following["problems"], [])
        self.assertEqual(following["new_completed_errors"], [])
        self.assertEqual(following["completed_cycles"][0]["errors"], [error])

    def test_intervening_protected_denial_is_recorded_as_warning(self):
        error = "24-hour cache expiration: /Users/test/Library/Caches/FamilyCircle: Operation not permitted (os error 1)"
        sample = self.completed_sample([error])
        sample["snapshot"]["activity"].append({
            "at": 1950, "category": "scan", "message":
            "Finished: 1 worktrees checked; 0 processes stopped; 0 worktrees removed; 0 caches removed; 0 errors."})
        row = monitor.compact("host", sample, 2000, completed_after=1800)
        self.assertEqual(row["problems"], [])
        self.assertEqual(row["warnings"], ["protected_os_cache"])
        self.assertEqual(row["new_completed_errors"], [error])

    def test_missing_intervening_details_cannot_masquerade_as_success(self):
        sample = self.completed_sample([], count=1)
        sample["snapshot"]["activity"].append({
            "at": 1950, "category": "scan", "message":
            "Finished: 1 worktrees checked; 0 processes stopped; 0 worktrees removed; 0 caches removed; 0 errors."})
        row = monitor.compact("host", sample, 2000, completed_after=1800)
        self.assertIn("cleanup_errors", row["problems"])
        self.assertTrue(row["completed_cycles"][0]["errors"])

    def test_first_sample_retains_failures_older_than_ten_cycles(self):
        sample = self.completed_sample(["Worktree cleaner: failure"])
        sample["snapshot"]["activity"] += [
            {"at": at, "category": "scan", "message":
             "Finished: 1 worktrees checked; 0 processes stopped; 0 worktrees removed; 0 caches removed; 0 errors."}
            for at in range(1900, 1912)]
        row = monitor.compact("host", sample, 2000, completed_after=0)
        self.assertEqual(len(row["completed_cycles"]), 13)
        self.assertIn("cleanup_errors", row["problems"])

    def test_schedule_persists_failure_cursor_across_unreachable_sample(self):
        sample = self.completed_sample(["Worktree cleaner: failure"])
        sample["snapshot"]["activity"].append({
            "at": 1950, "category": "scan", "message":
            "Finished: 1 worktrees checked; 0 processes stopped; 0 worktrees removed; 0 caches removed; 0 errors."})
        with tempfile.TemporaryDirectory() as directory:
            args = ["monitor", "--directory", directory]
            with mock.patch("sys.argv", args), mock.patch("builtins.print"), \
                    mock.patch.object(monitor, "probe", side_effect=[sample, {"error": "offline"}, sample]), \
                    mock.patch.object(monitor.time, "time", side_effect=[2000, 2300, 2600]):
                monitor.main()
                monitor.main()
                state = json.loads((pathlib.Path(directory) / "monitor-state.json").read_text())
                self.assertEqual(state["local"]["completed_through"], 1950)
                monitor.main()
            history = [json.loads(line) for line in
                       (pathlib.Path(directory) / "history-19700101.jsonl").read_text().splitlines()]
            self.assertEqual([row["problems"] for row in history],
                             [["cleanup_errors"], ["unreachable"], []])
            self.assertEqual(history[-1]["completed_cycles"][0]["errors"],
                             ["Worktree cleaner: failure"])


if __name__ == "__main__":
    unittest.main()
