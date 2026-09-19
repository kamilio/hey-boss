"""Copy GitHub issues to hey-boss before deleting them. Requires gh and Python 3."""

import argparse
import json
import os
import subprocess
import sys
from urllib.parse import urlencode, urlparse

BODY_LIMIT = 1024 * 1024
IMPORT_AGENT = "github-import"


def run_json(command, body=None):
    result = subprocess.run(command, input=body, capture_output=True, text=True, timeout=120)
    if result.returncode:
        raise RuntimeError(result.stderr.strip() or result.stdout.strip() or "Command failed")
    value = json.loads(result.stdout)
    if isinstance(value, dict) and (value.get("ok") is False or value.get("errors")):
        raise RuntimeError(json.dumps(value.get("error", value.get("errors"))))
    return value


class Github:
    def __init__(self, repo=None):
        command = ["gh", "repo", "view"]
        if repo:
            command.append(repo)
        info = run_json(command + ["--json", "nameWithOwner,url"])
        self.repo = info["nameWithOwner"]
        self.host = urlparse(info["url"]).hostname
        if not self.host or len(self.repo.split("/")) != 2:
            raise RuntimeError("Could not resolve a GitHub repository")

    def api(self, path, paginate=False, fields=None):
        command = ["gh", "api", "--hostname", self.host, path]
        if paginate:
            command += ["--paginate", "--slurp"]
        for key, value in (fields or {}).items():
            command += ["-f", f"{key}={value}"]
        result = run_json(command)
        if paginate:
            return [item for page in result for item in page]
        return result

    def issues(self, author, state):
        params = {"state": state, "per_page": 100, "sort": "created", "direction": "asc"}
        if author:
            params["creator"] = author
        return [issue for issue in self.api(f"repos/{self.repo}/issues?{urlencode(params)}", True)
                if "pull_request" not in issue]

    def snapshot(self, number):
        issue = self.api(f"repos/{self.repo}/issues/{number}")
        if "pull_request" in issue:
            raise RuntimeError("Refusing to drain a pull request")
        comments = self.api(f"repos/{self.repo}/issues/{number}/comments?per_page=100", True)
        if len(comments) != issue["comments"]:
            raise RuntimeError("GitHub comment count changed while reading; retry")
        # Keep all portable source fields, without volatile reaction counts/API URLs.
        keys = ("node_id", "number", "title", "body", "state", "state_reason", "html_url",
                "created_at", "updated_at", "closed_at")
        value = {key: issue.get(key) for key in keys}
        value["author"] = (issue.get("user") or {}).get("login", "ghost")
        value["labels"] = sorted(label["name"] for label in issue["labels"])
        value["assignees"] = sorted(user["login"] for user in issue.get("assignees", []))
        value["milestone"] = issue.get("milestone")
        value["comments"] = [{key: comment.get(key) for key in
                              ("id", "body", "html_url", "created_at", "updated_at")}
                             | {"author": (comment.get("user") or {}).get("login", "ghost")}
                             for comment in comments]
        if not value["node_id"] or value["state"] not in ("open", "closed"):
            raise RuntimeError("Invalid GitHub issue snapshot")
        return value

    def delete(self, node_id):
        result = self.api("graphql", fields={
            "query": "mutation($id:ID!){deleteIssue(input:{issueId:$id}){clientMutationId}}",
            "id": node_id,
        })
        if not isinstance(result.get("data", {}).get("deleteIssue"), dict):
            raise RuntimeError("GitHub did not confirm deletion; the saved copy remains")


class Destination:
    def __init__(self, binary, project=None, host=None):
        self.command = [binary, "issue", "--json", "--agent", IMPORT_AGENT]
        if host:
            self.command += ["--host", host]
        if project:
            self.command += ["--project", project]
        # Resolve once, then pin all operations to the full project ID.
        self.project = self.call(["whoami"])["project"]
        self.command += ["--project", self.project["id"]] if not project else []
        if project:
            self.command[-1] = self.project["id"]

    def call(self, args, body=None):
        result = run_json(self.command + args, body)
        if result.get("ok") is not True:
            raise RuntimeError("Destination did not acknowledge the operation")
        return result

    def copy(self, source):
        metadata = {key: value for key, value in source.items() if key not in ("body", "comments")}
        body = (source["body"] or "") + "\n\n---\nGitHub source metadata:\n\n" + json.dumps(
            metadata, ensure_ascii=False, indent=2)
        for comment in source["comments"]:
            info = {key: value for key, value in comment.items() if key != "body"}
            body += "\n\n---\nGitHub comment:\n\n" + json.dumps(info, ensure_ascii=False, indent=2)
            body += "\n\n" + (comment["body"] or "")
        if len(body.encode("utf-8")) > BODY_LIMIT:
            raise RuntimeError("GitHub issue and comments exceed the destination's 1 MiB limit")
        key = "github-drain:" + source["node_id"]
        args = ["create", "--title=" + source["title"], "--body", "-", "--request-id", key]
        for label in source["labels"]:
            args += ["--label=" + label]
        saved = self.call(args, body)["issue"]
        number = saved["number"]
        if source["state"] == "closed":
            self.call(["close", str(number), "--request-id", key + ":close"])
        return number, body

    def verify(self, source, number, body):
        saved = self.call(["view", str(number)])["issue"]
        if (saved["title"] != source["title"] or saved["body"] != body
                or sorted(saved["labels"]) != source["labels"]
                or saved["state"] != source["state"] or saved.get("deleted_at") is not None):
            raise RuntimeError("Destination copy differs from GitHub; original was retained")


def drain_one(github, destination, listed, author, state):
    source = github.snapshot(listed["number"])
    if author and source["author"].casefold() != author.casefold():
        raise RuntimeError("GitHub author no longer matches the filter")
    if state != "all" and source["state"] != state:
        raise RuntimeError("GitHub state changed since listing; retry")
    number, body = destination.copy(source)
    destination.verify(source, number, body)
    if github.snapshot(source["number"]) != source:
        raise RuntimeError(f"GitHub changed after copying to #{number}; original was retained")
    # Verify again after the network read, as workers may edit the saved copy.
    destination.verify(source, number, body)
    github.delete(source["node_id"])
    return number


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", help="GitHub owner/repo; default: current checkout")
    authors = parser.add_mutually_exclusive_group()
    authors.add_argument("--author", help="Creator login; default: authenticated gh user")
    authors.add_argument("--all-authors", action="store_true")
    parser.add_argument("--state", choices=("open", "closed", "all"), default="open")
    parser.add_argument("--project", help="Destination hey-boss project")
    parser.add_argument("--host", help="Authoritative SSH issue host")
    parser.add_argument("--dry-run", action="store_true")
    parser.add_argument("--json", action="store_true")
    args = parser.parse_args(argv)
    results = []
    report = {"ok": True, "dry_run": args.dry_run, "results": results}
    try:
        github = Github(args.repo)
        author = None if args.all_authors else args.author or github.api("user")["login"]
        if not args.all_authors and not author:
            raise RuntimeError("Could not resolve a GitHub author; specify --author")
        report.update(repository=github.repo, author=author)
        issues = github.issues(author, args.state)
        destination = None
        if issues and not args.dry_run:
            destination = Destination(os.environ.get("HEY_BOSS_DRAIN_BINARY", "hey-boss"),
                                      args.project, args.host)
            report["project"] = destination.project
        for issue in issues:
            result = {"github_number": issue["number"], "url": issue["html_url"]}
            try:
                if args.dry_run:
                    result["status"] = "would_move"
                else:
                    result["number"] = drain_one(github, destination, issue, author, args.state)
                    result["status"] = "moved"
            except (RuntimeError, OSError, ValueError, KeyError, TypeError, subprocess.TimeoutExpired) as error:
                report["ok"] = False
                result.update(status="failed", error=str(error))
            results.append(result)
    except (RuntimeError, OSError, ValueError, KeyError, TypeError, subprocess.TimeoutExpired) as error:
        report.update(ok=False, error=str(error))
    if args.json:
        print(json.dumps(report, ensure_ascii=False))
    else:
        for result in results:
            message = f'{result["url"]}: {result["status"]}'
            if "number" in result:
                message += f' to hey-boss #{result["number"]}'
            if "error" in result:
                message += f' ({result["error"]})'
            print(message)
        if "error" in report:
            print(report["error"], file=sys.stderr)
        elif not results:
            print("No matching GitHub issues.")
    return 0 if report["ok"] else 1


if __name__ == "__main__":
    sys.exit(main())
