#!/usr/bin/env python3
"""Serve an existing isolated mindmap fixture with a synthetic read-only Inbox.

Only databases under this checkout's out/ directory are accepted. The server
reloads after binary changes. Optional --tasks JSON is reread for each Inbox list.
"""
import argparse
import json
import os
import pathlib
import socket
import stat
import subprocess
import threading
import time


def main():
    root = pathlib.Path(__file__).resolve().parents[1]
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("database", type=pathlib.Path)
    parser.add_argument("--project", default="Atlas")
    parser.add_argument("--port", type=int, default=59479)
    parser.add_argument("--tasks", type=pathlib.Path)
    args = parser.parse_args()
    database = args.database.resolve(strict=True)
    if root / "out" not in database.parents:
        parser.error("Use an isolated database under this checkout's out/ directory")
    binary = root / "target/debug/hey-boss"
    socket_path = database.parent / "inbox-fixture.sock"
    if socket_path.exists():
        if not stat.S_ISSOCK(socket_path.lstat().st_mode):
            parser.error("Fixture socket path is occupied by a regular file")
        socket_path.unlink()
    listener = socket.socket(socket.AF_UNIX)
    listener.bind(str(socket_path))
    listener.listen()
    listener.settimeout(0.2)
    stop = threading.Event()

    def inbox():
        while not stop.is_set():
            try:
                client, _ = listener.accept()
            except socket.timeout:
                continue
            with client:
                client.settimeout(5)
                try:
                    chunks = []
                    while chunk := client.recv(65536):
                        chunks.append(chunk)
                    request = json.loads(b"".join(chunks))
                    with (database.parent / "inbox-requests.jsonl").open("a") as record:
                        record.write(json.dumps(request) + "\n")
                    if request.get("command") != "inbox_list":
                        reply = {"status": "error", "error": "Synthetic Inbox only supports reads"}
                    else:
                        tasks = json.loads(args.tasks.read_text()) if args.tasks else []
                        reply = {"status": "ok", "result": json.dumps({"tasks": tasks})}
                    client.sendall(json.dumps(reply).encode())
                except (BrokenPipeError, ConnectionResetError, socket.timeout):
                    pass
                except (ValueError, OSError):
                    try:
                        client.sendall(json.dumps({"status": "error", "error": "Synthetic Inbox snapshot is unavailable"}).encode())
                    except OSError:
                        pass

    thread = threading.Thread(target=inbox)
    thread.start()
    env = dict(os.environ, HEY_BOSS_ISSUE_DB=str(database), HEY_BOSS_INBOX_SOCKET=str(socket_path))
    env.pop("HEY_BOSS_ISSUE_HOST", None)
    env.pop("HEY_BOSS_ISSUE_PROJECT", None)
    command = [str(binary), "mm", "--project", args.project, "--agent", "human:mindmap-demo", "--json", "web", "--port", str(args.port), "--no-discovery"]
    process = None
    try:
        failures = 0
        signature = None
        while True:
            current = (binary.stat().st_mtime_ns, binary.stat().st_size)
            changed = current != signature
            if process is None or changed:
                if process is not None and process.poll() is None:
                    process.terminate()
                    process.wait(timeout=5)
                process = subprocess.Popen(command, cwd=root, env=env)
                signature = current
                if changed:
                    failures = 0
            if process.poll() is not None:
                failures += 1
                if failures >= 3:
                    raise RuntimeError("Fixture web server exited three times without a binary change")
                process = None
                time.sleep(1)
            time.sleep(0.2)
    finally:
        if process is not None and process.poll() is None:
            process.terminate()
            process.wait(timeout=5)
        stop.set()
        thread.join(timeout=6)
        listener.close()
        socket_path.unlink(missing_ok=True)


if __name__ == "__main__":
    main()
