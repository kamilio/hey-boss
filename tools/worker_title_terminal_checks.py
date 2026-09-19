#!/usr/bin/env python3
"""Verify worker context and tab naming against any built/installed CLI in a PTY."""
import fcntl
import json
import os
import pathlib
import pty
import select
import shlex
import shutil
import struct
import subprocess
import sys
import tempfile
import termios
import time


class Terminal:
    def __init__(self, binary, args, root, env):
        self.master, self.slave = pty.openpty()
        fcntl.ioctl(self.slave, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 120, 0, 0))
        self.attributes = termios.tcgetattr(self.slave)
        self.output = b""
        self.child = subprocess.Popen([binary, "worker", *args], cwd=root, env=env,
                                      stdin=self.slave, stdout=self.slave, stderr=self.slave)

    def read(self, seconds=0.1):
        if select.select([self.master], [], [], seconds)[0]:
            self.output += os.read(self.master, 65536)

    def wait(self, condition, seconds=20):
        deadline = time.monotonic() + seconds
        while not condition():
            self.read()
            if condition():
                break
            assert self.child.poll() is None, self.output.decode(errors="replace")
            assert time.monotonic() < deadline, self.output.decode(errors="replace")

    def close(self):
        try:
            os.write(self.master, b"q")
            self.wait(lambda: self.child.poll() is not None, seconds=3)
            assert self.child.returncode == 0, self.output.decode(errors="replace")
            assert termios.tcgetattr(self.slave) == self.attributes, "Terminal was not restored"
        finally:
            if self.child.poll() is None:
                self.child.kill()
                self.child.wait()
            os.close(self.master)
            os.close(self.slave)


def check(binary, mode):
    with tempfile.TemporaryDirectory(prefix="hey-boss-title-") as directory:
        root = pathlib.Path(directory).resolve()
        env = {key: value for key, value in os.environ.items()
               if not key.startswith("HEY_BOSS_") and key not in ("TMUX", "TMUX_PANE", "TERM_PROGRAM")}
        # The empty queue never launches an agent; don't depend on a real Codex install.
        env.update(HEY_BOSS_ISSUE_DB=str(root / "issues.db"), HEY_BOSS_CODEX=shutil.which("false"),
                   TERM="xterm-256color")
        log = root / "tmux.args"
        fake = root / "bin"
        fake.mkdir()
        tmux = fake / "tmux"
        tmux.write_text("#!/bin/sh\n"
                        f"printf '%s\\n' \"$@\" >> {shlex.quote(str(log))}\n"
                        f"if test -e {shlex.quote(str(root / 'hang'))}; then exec sleep 10; fi\n"
                        f"if test -e {shlex.quote(str(root / 'fail'))}; then exit 1; fi\n")
        tmux.chmod(0o755)
        env["PATH"] = str(fake) + os.pathsep + env.get("PATH", "")
        env["TERM_PROGRAM"] = "iTerm.app"
        if mode.startswith("tmux"):
            env["TMUX"] = "synthetic-socket"
            if mode != "tmux-no-pane":
                env["TMUX_PANE"] = "%42"
            if mode == "tmux-timeout":
                (root / "hang").touch()
            if mode == "tmux-failure":
                (root / "fail").touch()
        project = "Title fixture"
        title = f"hey-boss · {project} · {root}"
        terminal = Terminal(binary, ["--project", project, "--directory", str(root)], root, env)
        dashboard = None
        try:
            terminal.wait(lambda: b"AVAILABLE" in terminal.output)
            if mode == "iterm":
                assert f"\x1b]1;{title}\x07".encode() in terminal.output
                assert b"\x1b]0;" not in terminal.output
            elif mode == "tmux-no-pane":
                assert not log.exists(), "Missing pane renamed another window"
                assert b"\x1b]1;" not in terminal.output
            else:
                assert log.read_text().splitlines() == ["rename-window", "-t", "%42", "--", title]
                assert b"\x1b]1;" not in terminal.output
            status = subprocess.run([binary, "worker", "--json", "status"], cwd=root, env=env,
                                    capture_output=True, check=True)
            value = json.loads(status.stdout)
            assert value["projects"] == [{"id": "named:" + project, "name": project}]
            # Viewing status from a different directory must still name the worker's checkout.
            if mode == "iterm":
                dashboard = Terminal(binary, ["--id", value["worker_id"], "status"], fake, env)
                dashboard.wait(lambda: f"\x1b]1;{title}\x07".encode() in dashboard.output)
                dashboard.close()
                dashboard = None
            # Refreshes must not repeatedly spawn tmux or emit the same OSC title.
            deadline = time.monotonic() + 2.5
            while time.monotonic() < deadline:
                terminal.read()
            if mode == "iterm":
                assert terminal.output.count(f"\x1b]1;{title}\x07".encode()) == 1
            elif mode != "tmux-no-pane":
                assert len(log.read_text().splitlines()) == 5
            plain = subprocess.run([binary, "worker", "status"], cwd=root, env=env,
                                   capture_output=True, check=True)
            assert b"\x1b" not in plain.stdout, "Redirected status contains terminal controls"
        finally:
            if dashboard:
                dashboard.close()
            terminal.close()


if __name__ == "__main__":
    binary = shutil.which(sys.argv[1] if len(sys.argv) > 1 else "hey-boss")
    assert binary, "CLI was not found"
    binary = str(pathlib.Path(binary).resolve())
    for mode in ("iterm", "tmux", "tmux-no-pane", "tmux-timeout", "tmux-failure"):
        check(binary, mode)
        print(mode + ": passed", flush=True)
