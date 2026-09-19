#!/bin/bash
# Install only the health worker; preserve companion services and running agents.
set -euo pipefail
task_host=${1:?Usage: install-health-worker.sh SSH_HOST}
task_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
task_ssh=(-T -o BatchMode=yes -o ConnectTimeout=5 -o ServerAliveInterval=10 -o ServerAliveCountMax=2)
if [[ -n ${HEY_BOSS_SSH_CONTROL_PATH:-} ]]; then task_ssh+=(-S "$HEY_BOSS_SSH_CONTROL_PATH"); fi
if [[ $task_host == -* || $task_host == *[!a-zA-Z0-9@._:\[\]-]* ]]; then printf '%s\n' 'Invalid SSH host' >&2; exit 1; fi
COPYFILE_DISABLE=1 tar --no-xattrs -czf - -C "$task_root" Cargo.toml Cargo.lock build.rs src skills/hey-boss README.md LICENSE tools/upgrade_hey_boss.py hey_boss_daemon.swift package_hey_boss.swift setup_hey_boss.swift assets |
    ssh "${task_ssh[@]}" "$task_host" 'set -eu
umask 077
stage=$(mktemp -d)
trap '\''rm -rf "$stage"'\'' EXIT HUP INT TERM
tar -xzf - -C "$stage"
export PATH="$HOME/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin:$PATH"
export CARGO_TARGET_DIR="$stage/target"
# Process/lock fixtures inspect global OS state; run them serially.
cargo test --locked --manifest-path "$stage/Cargo.toml" --lib health -- --test-threads=1
cargo build --locked --release --manifest-path "$stage/Cargo.toml"
python3 - "$CARGO_TARGET_DIR/release/hey-boss" <<'\''PY'\''
import fcntl, os, pathlib, shutil, sys, tempfile
home = pathlib.Path.home()
state = home / ".local/share/hey-boss/health"
state.mkdir(mode=0o700, parents=True, exist_ok=True)
if state.is_symlink() or state.stat().st_uid != os.geteuid():
    raise RuntimeError("Health state must be an owned directory")
fd = os.open(state / "maintenance.lock", os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
with os.fdopen(fd, "r+") as lock:
    fcntl.flock(lock, fcntl.LOCK_EX)
    target = home / ".local/bin/hey-boss-health"
    target.parent.mkdir(parents=True, exist_ok=True)
    temp_fd, temporary = tempfile.mkstemp(prefix=".hey-boss-health-", dir=target.parent)
    os.close(temp_fd)
    try:
        shutil.copyfile(sys.argv[1], temporary)
        os.chmod(temporary, 0o700)
        os.replace(temporary, target)
    finally:
        if os.path.exists(temporary): os.unlink(temporary)
print("Health worker installed; existing services and agents kept running.")
PY
'
