#!/usr/bin/env python3
"""One bounded, read-only health sample. Schedule every five minutes if desired.

Example: monitor_harvester.py --host local --host devbox --directory REPORT_DIR
Optional --until is a Unix timestamp; --notify pages only on persistent problems.
Use --ssh-control HOST=/absolute/socket to reuse an existing fleet connection.
The harvester remains responsible for every cleanup and process action.
"""
import argparse
import concurrent.futures
import datetime
import fcntl
import json
import os
import pathlib
import re
import shlex
import signal
import subprocess
import time


# Automatic scheduling alone does not prove the requested cleaners are enabled.
CLEANERS = {
    "harvest_processes": "process_cleanup_disabled",
    "clean_worktrees": "worktree_cleanup_disabled",
    "clean_caches": "cache_cleanup_disabled",
    "trim_worker_logs": "worker_log_cleanup_disabled",
    "aggressive": "aggressive_cleanup_disabled",
}


# Runs on the observed machine; only reads health, executable and scheduler state.
PROBE = r'''
import hashlib,json,os,pathlib,subprocess,sys,time
binary=pathlib.Path.home()/".local/bin/hey-harvester"
def run(args):
    p=subprocess.run(args,capture_output=True,text=True,timeout=35)
    return {"returncode":p.returncode,"stdout":p.stdout,"stderr":p.stderr[-2000:]}
status=run([str(binary),"status","--json"])
if status["returncode"]:
    raise RuntimeError(status["stderr"])
snapshot=json.loads(status["stdout"])
if sys.platform=="darwin":
    scheduler=run(["/bin/launchctl","print",f"gui/{os.getuid()}/local.hey-boss.health"])
else:
    scheduler=run(["systemctl","--user","is-active","hey-boss-health.timer"])
print(json.dumps({"snapshot":snapshot,"scheduler":scheduler,
    "binary_sha256":hashlib.sha256(binary.read_bytes()).hexdigest(),"machine_time":int(time.time())}))
'''


def expected_access_denial(error):
    """Only observed OS privacy boundaries are warnings; other failures still page."""
    caches = {"FamilyCircle", "CloudKit", "com.apple.HomeKit", "com.apple.Safari",
              "com.apple.findmy.imagecache", "com.apple.findmy.fmfcore",
              "com.apple.containermanagerd", "com.apple.homed",
              "com.apple.findmy.fmipcore", "com.apple.ap.adprivacyd"}
    services = {"com.apple.passd", "com.apple.chrono", "duetexpertd",
                "com.apple.studentd", "com.apple.parsecd", "com.apple.identityservicesd",
                "com.apple.bluetoothuserd", "com.apple.imdpersistence.IMDPersistenceAgent",
                "com.apple.CloudDocs.iCloudDriveFileProvider", "com.apple.appleaccountd",
                "com.apple.syncdefaultsd", "com.apple.amsengagementd",
                "com.apple.icloud.searchpartyuseragent", "com.apple.transparencyd",
                "com.apple.triald", "com.apple.ap.promotedcontentd", "homed",
                "com.apple.pluginkit", "com.apple.donotdisturbd", "com.apple.securityuploadd",
                "com.apple.appstoreagent", "com.apple.imtransferservices.IMTransferAgent"}
    for part in error.removeprefix("24-hour cache expiration: ").split("; "):
        suffix = ": Operation not permitted (os error 1)"
        if not part.endswith(suffix):
            return False
        path = part[:-len(suffix)]
        cache = re.fullmatch(r"/Users/[^/]+/Library/Caches/([^/]+)", path)
        temporary = re.fullmatch(r"/private/var/folders/[^/]+/[^/]+/T/([^/]+)/TemporaryItems", path)
        if not ((cache and cache[1] in caches) or (temporary and temporary[1] in services)):
            return False
    return True


def completed_cycles(snapshot):
    """Capture each retained result before later cycles evict its error details."""
    cycles, errors = [], []
    for event in snapshot.get("activity", []):
        category, message = event.get("category"), event.get("message", "")
        if category == "error":
            errors.append(message)
        if category != "scan":
            continue
        if message == "Cleanup check started" or message.startswith("Inspection started;"):
            errors = []
        elif message.startswith("Finished:"):
            count = re.search(r"; (\d+) errors\.$", message)
            details = list(dict.fromkeys(errors)) if not count or int(count[1]) else []
            if not count or len(details) < int(count[1]):
                details.append("Completed cleanup has unavailable error details")
            cycles.append({**event, "errors": details})
            errors = []
    return cycles


def last_completed_errors(snapshot):
    """Retain the last result while a new cycle has cleared snapshot.errors."""
    cycles = completed_cycles(snapshot)
    return cycles[-1]["errors"] if cycles else []


def problems(sample, now):
    if "error" in sample:
        return ["unreachable"]
    s = sample["snapshot"]
    result = []
    if sample["scheduler"]["returncode"]:
        result.append("scheduler_unavailable")
    config = s.get("config") or {}
    if config.get("automatic") is not True:
        result.append("cleanup_disabled")
    result.extend(problem for setting, problem in CLEANERS.items()
                  if config.get(setting) is not True)
    if now - (s.get("last_cleanup_at") or 0) > 900:
        result.append("cleanup_stale")
    if "cache_progress" not in s:
        result.append("cache_telemetry_missing")
    progress = s.get("cache_progress") or {}
    if (progress.get("pass_started_at") and
            now - progress["pass_started_at"] > 86400 and
            (progress.get("roots_pending") or progress.get("discovery_pending"))):
        result.append("cache_pass_overdue")
    m = s.get("metrics", {})
    if m.get("disk_available_bytes") is not None and m["disk_available_bytes"] < 25_000_000_000:
        result.append("disk_low")
    if m.get("memory_pressure", "").lower() == "critical":
        result.append("memory_critical")
    if any(not expected_access_denial(error)
           for error in s.get("errors", []) + last_completed_errors(s)):
        result.append("cleanup_errors")
    return result


def stop_probe_group(process):
    for sig in (signal.SIGTERM, signal.SIGKILL):
        try:
            os.killpg(process.pid, sig)
        except ProcessLookupError:
            break
        if sig == signal.SIGTERM:
            time.sleep(0.1)


def run_probe(args, timeout=100):
    # SSH may exit while its authentication proxy still holds the output pipes.
    # Own a separate process group so timeout cleanup cannot reach other jobs.
    with subprocess.Popen(args, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                          text=True, start_new_session=True) as process:
        try:
            stdout, stderr = process.communicate(timeout=timeout)
        except subprocess.TimeoutExpired:
            stop_probe_group(process)
            process.communicate(timeout=2)
            raise
        if process.returncode:
            # Failed SSH can close its pipes before its proxy notices the exit.
            stop_probe_group(process)
        return subprocess.CompletedProcess(args, process.returncode, stdout, stderr)


def probe(host, control=None):
    args = ["python3", "-c", PROBE]
    if host != "local":
        args = ["ssh", "-T", "-o", "BatchMode=yes", "-o", "ConnectTimeout=15",
                "-o", "ServerAliveInterval=10", "-o", "ServerAliveCountMax=2",
                "-o", "ControlMaster=no",
                *(["-S", str(control), "-o", "ProxyCommand=false", "-o", "ProxyJump=none"]
                  if control else []), host, shlex.join(args)]
    try:
        p = run_probe(args)
        if p.returncode:
            return {"error": p.stderr[-2000:] or f"probe exited {p.returncode}"}
        return json.loads(p.stdout)
    except (subprocess.TimeoutExpired, OSError, ValueError) as e:
        return {"error": str(e)}


def save(path, data):
    temporary = path.with_suffix(".next")
    temporary.write_text(json.dumps(data, indent=2) + "\n")
    temporary.replace(path)


def compact(host, sample, now, completed_after=0):
    result = {"host": host, "collected_at": now,
              "problems": problems(sample, sample.get("machine_time", now))}
    if "error" in sample:
        result["error"] = sample["error"]
        return result
    s = sample["snapshot"]
    result["cleanup_settings"] = {
        key: (s.get("config") or {}).get(key) for key in ("automatic", *CLEANERS)
    }
    result["last_completed_errors"] = last_completed_errors(s)
    cycles = completed_cycles(s)
    result["completed_cycles"] = cycles
    result["completed_through"] = max([completed_after] + [c["at"] for c in cycles])
    result["new_completed_errors"] = list(dict.fromkeys(
        error for cycle in cycles if cycle["at"] > completed_after
        for error in cycle["errors"]))
    if (any(not expected_access_denial(e) for e in result["new_completed_errors"])
            and "cleanup_errors" not in result["problems"]):
        result["problems"].append("cleanup_errors")
    result["warnings"] = (["protected_os_cache"]
                          if any(expected_access_denial(e) for e in
                                 s.get("errors", []) + result["last_completed_errors"] +
                                 result["new_completed_errors"]) else [])
    for key in ("observed_at", "last_cleanup_at", "metrics", "cache_progress",
                "harvested_processes", "removed_worktrees", "removed_caches",
                "errors", "running", "phase"):
        result[key] = s.get(key)
    result["binary_sha256"] = sample["binary_sha256"]
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--host", action="append", default=[])
    parser.add_argument("--directory", type=pathlib.Path, required=True)
    parser.add_argument("--until", type=int)
    parser.add_argument("--notify", type=pathlib.Path)
    parser.add_argument("--ssh-control", action="append", default=[], metavar="HOST=PATH")
    options = parser.parse_args()
    hosts = options.host or ["local"]
    if any(not re.fullmatch(r"[a-zA-Z0-9][a-zA-Z0-9@._-]*", h) for h in hosts):
        parser.error("Invalid host name")
    controls = {}
    for item in options.ssh_control:
        host, separator, path = item.partition("=")
        if (not separator or host not in hosts or host == "local" or host in controls or
                not pathlib.Path(path).is_absolute()):
            parser.error("Each --ssh-control needs a selected remote host and an absolute socket path")
        controls[host] = pathlib.Path(path)
    os.umask(0o077)
    options.directory.mkdir(parents=True, exist_ok=True)
    with (options.directory / "monitor.lock").open("a") as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            return
        now = int(time.time())
        if options.until and now >= options.until:
            save(options.directory / "ended.json", {"ended_at": now})
            return
        state_path = options.directory / "monitor-state.json"
        state = json.loads(state_path.read_text()) if state_path.exists() else {}
        rows = []
        with concurrent.futures.ThreadPoolExecutor(max_workers=min(3, len(hosts))) as pool:
            for host, sample in zip(hosts, pool.map(probe, hosts, (controls.get(h) for h in hosts))):
                sample["collected_at"] = now
                # An unreachable machine replaces its latest record, so stale success cannot masquerade as current.
                save(options.directory / f"{host}-latest.json", sample)
                previous = state.get(host, {})
                row = compact(host, sample, now, previous.get("completed_through", 0))
                rows.append(row)
                issues = row["problems"]
                same = issues == previous.get("problems")
                since = previous.get("since", now) if same else now
                last_alert = previous.get("last_alert", 0) if same else 0
                # Disconnected fleet machines are routine; retain evidence without paging.
                alert_issues = [issue for issue in issues if issue != "unreachable"]
                # Require two samples for transient inspection failures; low disk pages immediately.
                actionable = "disk_low" in issues or (same and now - since >= 240)
                if options.notify and alert_issues and actionable and now - last_alert >= 21600:
                    text = f"{host}: {', '.join(alert_issues)}. Disk free: {row.get('metrics', {}).get('disk_available_bytes', 'unknown')} bytes. Evidence: {options.directory}"
                    try:
                        sent = subprocess.run([str(options.notify), "notif", "alert", "--title",
                                               "Harvester needs attention", text], capture_output=True,
                                              text=True, timeout=20)
                        if sent.returncode == 0:
                            last_alert = now
                        else:
                            row["notification_error"] = sent.stderr[-1000:]
                    except (OSError, subprocess.TimeoutExpired) as e:
                        row["notification_error"] = str(e)
                state[host] = {"problems": issues, "since": since, "last_alert": last_alert,
                               "completed_through": row.get("completed_through",
                                                             previous.get("completed_through", 0))}
        day = datetime.datetime.fromtimestamp(now, datetime.timezone.utc).strftime("%Y%m%d")
        with (options.directory / f"history-{day}.jsonl").open("a") as history:
            for row in rows:
                history.write(json.dumps(row) + "\n")
        save(options.directory / "latest-summary.json", rows)
        save(state_path, state)
        for row in rows:
            m = row.get("metrics") or {}
            print(json.dumps({"host": row["host"], "problems": row["problems"],
                              "warnings": row.get("warnings", []),
                              "disk_available_bytes": m.get("disk_available_bytes"),
                              "memory_pressure": m.get("memory_pressure")}))


if __name__ == "__main__":
    main()
