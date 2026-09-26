import importlib.util
import pathlib
import unittest
from unittest import mock

spec = importlib.util.spec_from_file_location(
    "monitor", pathlib.Path(__file__).with_name("monitor_harvester.py")
)
monitor = importlib.util.module_from_spec(spec)
spec.loader.exec_module(monitor)


class MonitorTests(unittest.TestCase):
    def test_remote_probe_reuses_only_its_configured_control_socket(self):
        run = mock.Mock(return_value=mock.Mock(returncode=0, stdout='{}'))
        control = pathlib.Path('/tmp/fixture ssh.sock')
        with mock.patch.object(monitor.subprocess, 'run', run):
            self.assertEqual(monitor.probe('devbox', control), {})
        args = run.call_args.args[0]
        self.assertEqual(args[args.index('-S') + 1], str(control))
        self.assertEqual(args[-2], 'devbox')

    def test_local_probe_does_not_use_ssh_control(self):
        run = mock.Mock(return_value=mock.Mock(returncode=0, stdout='{}'))
        with mock.patch.object(monitor.subprocess, 'run', run):
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
            "/private/var/folders/dd/test_user/T/com.apple.unknown/TemporaryItems: Operation not permitted (os error 1)",
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

    def test_newly_observed_apple_privacy_paths_remain_visible(self):
        paths = [
            "/private/var/folders/dd/test_user/T/com.apple.transparencyd/TemporaryItems",
            "/private/var/folders/dd/test_user/T/com.apple.triald/TemporaryItems",
            "/private/var/folders/dd/test_user/T/com.apple.ap.promotedcontentd/TemporaryItems",
            "/private/var/folders/dd/test_user/T/homed/TemporaryItems",
            "/private/var/folders/dd/test_user/T/com.apple.pluginkit/TemporaryItems",
            "/private/var/folders/dd/test_user/T/com.apple.donotdisturbd/TemporaryItems",
            "/private/var/folders/dd/test_user/T/com.apple.securityuploadd/TemporaryItems",
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


if __name__ == "__main__":
    unittest.main()
