import importlib.util
import pathlib
import unittest

spec = importlib.util.spec_from_file_location(
    "monitor", pathlib.Path(__file__).with_name("monitor_harvester.py")
)
monitor = importlib.util.module_from_spec(spec)
spec.loader.exec_module(monitor)


class MonitorTests(unittest.TestCase):
    def sample(self):
        return {"snapshot": {
            "observed_at": 1900, "last_cleanup_at": 1900,
            "running": False, "config": {"automatic": True},
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

    def test_running_cycles_do_not_hide_a_cache_pass_over_a_day(self):
        sample = self.sample()
        sample["snapshot"]["last_cleanup_at"] = 100000
        sample["snapshot"]["cache_progress"] = {"pass_started_at": 1, "discovery_pending": True}
        self.assertIn("cache_pass_overdue", monitor.problems(sample, 100000))
        sample["snapshot"]["cache_progress"]["discovery_pending"] = False
        self.assertNotIn("cache_pass_overdue", monitor.problems(sample, 100000))


if __name__ == "__main__":
    unittest.main()
