import contextlib
import importlib.util
import io
import json
import os
import pathlib
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import Mock, patch

spec = importlib.util.spec_from_file_location("drain", pathlib.Path(__file__).with_name("drain_github_issues.py"))
drain = importlib.util.module_from_spec(spec)
spec.loader.exec_module(drain)


def snapshot():
    return {"node_id": "I_source", "number": 7, "title": "Keep this issue", "body": "## Problem\nDetails",
            "state": "open", "state_reason": None, "html_url": "https://github.com/test/repo/issues/7",
            "author": "boss", "created_at": "2026-01-01", "updated_at": "2026-01-02", "closed_at": None,
            "labels": ["bug"], "assignees": ["boss"], "milestone": None,
            "comments": [{"id": 99, "body": "A comment with unicode: café", "author": "other",
                          "html_url": "https://github.com/test/repo/issues/7#issuecomment-99",
                          "created_at": "2026-01-02", "updated_at": "2026-01-02"}]}


class DrainTests(unittest.TestCase):
    def setup_move(self):
        github = Mock()
        github.snapshot.return_value = snapshot()
        dest = Mock()
        dest.copy.return_value = (1, "saved body")
        return github, dest

    def test_deletion_follows_two_verified_reads(self):
        github, dest = self.setup_move()
        calls = Mock()
        calls.attach_mock(github, "github")
        calls.attach_mock(dest, "dest")
        self.assertEqual(drain.drain_one(github, dest, {"number": 7}, "boss", "open"), 1)
        self.assertEqual([c[0] for c in calls.mock_calls],
                         ["github.snapshot", "dest.copy", "dest.verify", "github.snapshot", "dest.verify", "github.delete"])
        github.delete.assert_called_once_with("I_source")

    def test_changed_issue_or_comment_prevents_deletion(self):
        for target in ("body", "comments"):
            with self.subTest(target=target):
                github, dest = self.setup_move()
                changed = snapshot()
                if target == "body":
                    changed["body"] = "Edited during import"
                else:
                    changed["comments"][0]["body"] = "Edited comment"
                github.snapshot.side_effect = [snapshot(), changed]
                with self.assertRaisesRegex(RuntimeError, "changed after copying"):
                    drain.drain_one(github, dest, {"number": 7}, "boss", "open")
                github.delete.assert_not_called()

    def test_destination_failures_never_delete_source(self):
        for operation in ("copy", "verify"):
            github, dest = self.setup_move()
            getattr(dest, operation).side_effect = RuntimeError("unavailable")
            with self.assertRaisesRegex(RuntimeError, "unavailable"):
                drain.drain_one(github, dest, {"number": 7}, "boss", "open")
            github.delete.assert_not_called()

    def test_author_and_state_rechecked(self):
        for author, state in (("somebody-else", "open"), ("boss", "closed")):
            github, dest = self.setup_move()
            with self.assertRaises(RuntimeError):
                drain.drain_one(github, dest, {"number": 7}, author, state)
            dest.copy.assert_not_called()
            github.delete.assert_not_called()

    def test_default_author_and_dry_run_do_not_write(self):
        github = Mock(repo="test/repo")
        github.api.return_value = {"login": "boss"}
        github.issues.return_value = [{"number": 7, "html_url": snapshot()["html_url"]}]
        with patch.object(drain, "Github", return_value=github), patch.object(drain, "Destination") as dest, contextlib.redirect_stdout(io.StringIO()) as output:
            self.assertEqual(drain.main(["--dry-run", "--json"]), 0)
        github.issues.assert_called_once_with("boss", "open")
        github.delete.assert_not_called()
        dest.assert_not_called()
        self.assertEqual(json.loads(output.getvalue())["results"][0]["status"], "would_move")

    def test_author_overrides(self):
        for args, author in ((["--author", "other"], "other"), (["--all-authors"], None)):
            github = Mock(repo="test/repo")
            github.issues.return_value = []
            with patch.object(drain, "Github", return_value=github), contextlib.redirect_stdout(io.StringIO()):
                self.assertEqual(drain.main(args), 0)
            github.api.assert_not_called()
            github.issues.assert_called_once_with(author, "open")

    def test_failed_issue_does_not_prevent_remaining_moves(self):
        github = Mock(repo="test/repo")
        github.issues.return_value = [{"number": n, "html_url": f"https://github.com/test/repo/issues/{n}"}
                                      for n in (1, 2)]
        dest = Mock(project={"id": "target", "name": "target"})
        with patch.object(drain, "Github", return_value=github), patch.object(drain, "Destination", return_value=dest), patch.object(drain, "drain_one", side_effect=[RuntimeError("first retained"), 2]), contextlib.redirect_stdout(io.StringIO()) as output:
            self.assertEqual(drain.main(["--all-authors", "--json"]), 1)
        report = json.loads(output.getvalue())
        self.assertFalse(report["ok"])
        self.assertEqual([item["status"] for item in report["results"]], ["failed", "moved"])

    def test_pagination_and_pull_requests(self):
        github = object.__new__(drain.Github)
        github.host, github.repo = "github.com", "test/repo"
        with patch.object(drain, "run_json", return_value=[[{"number": 1}], [{"number": 2, "pull_request": {}}], [{"number": 3}]]) as run:
            self.assertEqual([issue["number"] for issue in github.issues("boss", "all")], [1, 3])
        command = run.call_args.args[0]
        self.assertIn("--paginate", command)
        self.assertIn("--slurp", command)
        self.assertIn("creator=boss", command[4])

    def test_missing_comments_and_graphql_errors_fail_closed(self):
        github = object.__new__(drain.Github)
        github.repo = "test/repo"
        github.api = Mock(side_effect=[dict(snapshot(), comments=2, user={"login": "boss"}), []])
        with self.assertRaisesRegex(RuntimeError, "comment count"):
            github.snapshot(7)
        github.api = Mock(return_value={"data": {"deleteIssue": None}})
        with self.assertRaisesRegex(RuntimeError, "did not confirm"):
            github.delete("I_source")
        result = subprocess.CompletedProcess([], 0, '{"errors":[{"message":"denied"}]}', "")
        with patch.object(drain.subprocess, "run", return_value=result):
            with self.assertRaisesRegex(RuntimeError, "denied"):
                drain.run_json(["gh", "api", "graphql"])


class DestinationTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        binary = os.environ.get("HEY_BOSS_TEST_BINARY")
        if not binary:
            raise unittest.SkipTest("Set HEY_BOSS_TEST_BINARY to test against a built CLI")
        cls.binary = str(pathlib.Path(binary).resolve())

    def setUp(self):
        self.root = tempfile.TemporaryDirectory()
        self.addCleanup(self.root.cleanup)
        environment = patch.dict(os.environ, {
            "HEY_BOSS_ISSUE_DB": str(pathlib.Path(self.root.name) / "issues.db"),
            "HEY_BOSS_ISSUE_HOST": "", "HEY_BOSS_ISSUE_PROJECT": "",
        })
        environment.start()
        self.addCleanup(environment.stop)
        self.dest = drain.Destination(self.binary, "github-drain-test")

    def test_retries_reuse_copy_and_preserve_all_content(self):
        source = snapshot()
        number, body = self.dest.copy(source)
        self.dest.verify(source, number, body)
        retry = drain.Destination(self.binary, "github-drain-test")
        self.assertEqual(retry.copy(source), (number, body))
        self.assertIn(source["body"], body)
        self.assertIn(source["comments"][0]["body"], body)
        self.assertIn(source["comments"][0]["html_url"], body)
        self.assertEqual(len(self.dest.call(["list"])["issues"]), 1)

    def test_changed_source_conflicts_without_duplicate(self):
        source = snapshot()
        self.dest.copy(source)
        source["body"] = "new content"
        with self.assertRaisesRegex(RuntimeError, "different operation"):
            self.dest.copy(source)
        self.assertEqual(len(self.dest.call(["list"])["issues"]), 1)

    def test_edited_or_deleted_destination_fails_verification(self):
        source = snapshot()
        number, body = self.dest.copy(source)
        self.dest.call(["edit", str(number), "--body", "edited"])
        with self.assertRaisesRegex(RuntimeError, "copy differs"):
            self.dest.verify(source, number, body)
        self.dest.call(["delete", str(number)])
        with self.assertRaises(RuntimeError):
            self.dest.verify(source, number, body)

    def test_closed_issues_and_oversized_imports(self):
        source = snapshot()
        source["state"] = "closed"
        number, body = self.dest.copy(source)
        self.dest.verify(source, number, body)
        self.assertEqual(self.dest.copy(source), (number, body))
        source = snapshot()
        source["body"] = "é" * drain.BODY_LIMIT
        with self.assertRaisesRegex(RuntimeError, "1 MiB"):
            self.dest.copy(source)

    def fake_github(self):
        root = pathlib.Path(self.root.name)
        source = snapshot()
        issue = {key: value for key, value in source.items() if key not in ("author", "labels", "comments")}
        issue.update(user={"login": "boss"}, labels=[{"name": "bug"}], comments=1,
                     assignees=[{"login": "boss"}])
        comment = dict(source["comments"][0], user={"login": "other"})
        (root / "github.json").write_text(json.dumps({"issue": issue, "comment": comment}))
        script = root / "gh"
        script.write_text(f"#!{sys.executable}\n" + '''import json, os, pathlib, sys
root = pathlib.Path(os.environ["FAKE_GITHUB_ROOT"])
data = json.loads((root / "github.json").read_text())
if sys.argv[1:3] == ["repo", "view"]:
    result = {"nameWithOwner": "test/repo", "url": "https://github.com/test/repo"}
else:
    endpoint = sys.argv[4]
    if endpoint == "user": result = {"login": "boss"}
    elif endpoint == "graphql":
        (root / "delete.called").write_text("yes")
        result = {"errors": [{"message": "permission denied"}]} if os.environ.get("FAKE_DELETE_FAIL") else {"data": {"deleteIssue": {"clientMutationId": None}}}
    elif "/comments?" in endpoint: result = [[data["comment"]]]
    elif "/issues/7" in endpoint: result = data["issue"]
    else: result = [[data["issue"]]]
print(json.dumps(result))
''')
        script.chmod(0o700)
        environment = patch.dict(os.environ, {
            "PATH": str(root) + os.pathsep + os.environ["PATH"],
            "FAKE_GITHUB_ROOT": str(root), "FAKE_DELETE_FAIL": "",
        })
        environment.start()
        self.addCleanup(environment.stop)
        return root

    def cli(self, *args):
        return subprocess.run([self.binary, "issue", "--project", "github-drain-test", "--json", "drain-github", *args],
                              capture_output=True, text=True, timeout=120)

    def test_native_cli_dry_run_never_copies_or_deletes(self):
        root = self.fake_github()
        result = self.cli("--dry-run")
        self.assertEqual(result.returncode, 0, result.stderr + result.stdout)
        self.assertEqual(json.loads(result.stdout)["results"][0]["status"], "would_move")
        self.assertFalse((root / "delete.called").exists())
        self.assertEqual(self.dest.call(["list"])["issues"], [])

    def test_native_cli_deletion_failure_retries_same_copy(self):
        root = self.fake_github()
        with patch.dict(os.environ, FAKE_DELETE_FAIL="yes"):
            result = self.cli()
        self.assertEqual(result.returncode, 1, result.stderr + result.stdout)
        self.assertIn("permission denied", json.loads(result.stdout)["results"][0]["error"])
        self.assertTrue((root / "delete.called").exists())
        result = self.cli()
        self.assertEqual(result.returncode, 0, result.stderr + result.stdout)
        self.assertEqual(json.loads(result.stdout)["results"][0]["status"], "moved")
        self.assertEqual(len(self.dest.call(["list"])["issues"]), 1)


if __name__ == "__main__":
    unittest.main()
