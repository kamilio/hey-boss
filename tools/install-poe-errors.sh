#!/bin/bash
# Install one collector on the Mac supervisor; companions must not duplicate it.
set -euo pipefail
if [[ $(uname -s) != Darwin ]]; then
  echo 'Install the Poe collector on the Mac supervisor.' >&2
  exit 1
fi
source_dir=$(cd "$(dirname "$0")" && pwd)
poe_repo=${1:-"$HOME/Workspace/poe2"}
collector_dir="$HOME/Library/Application Support/hey-boss/collectors"
plist="$HOME/Library/LaunchAgents/com.hey-boss.poe-errors.plist"
bun_binary=$(command -v bun)
hey_boss_binary=$(command -v hey-boss)
mkdir -p "$collector_dir" "$HOME/Library/LaunchAgents"
chmod 700 "$collector_dir"
cp "$source_dir/poe-errors.ts" "$source_dir/poe-errors-core.ts" "$collector_dir/"
chmod 600 "$collector_dir/poe-errors.ts" "$collector_dir/poe-errors-core.ts"
# plutil escapes paths as XML; no credentials are written into the service file.
POE_ERRORS_PLIST="$plist" POE_ERRORS_BUN="$bun_binary" POE_ERRORS_CLI="$hey_boss_binary" POE_ERRORS_REPO="$poe_repo" POE_ERRORS_DIR="$collector_dir" "$bun_binary" -e '
const e=process.env;
const spec={Label:"com.hey-boss.poe-errors",ProgramArguments:[e.POE_ERRORS_BUN,e.POE_ERRORS_DIR+"/poe-errors.ts","--watch"],WorkingDirectory:e.POE_ERRORS_REPO,EnvironmentVariables:{HEY_BOSS_BIN:e.POE_ERRORS_CLI},RunAtLoad:true,KeepAlive:true,ThrottleInterval:30};
const child=Bun.spawn(["/usr/bin/plutil","-convert","xml1","-o",e.POE_ERRORS_PLIST,"--","-"],{stdin:"pipe",stdout:"inherit",stderr:"inherit"});child.stdin.write(JSON.stringify(spec));child.stdin.end();if(await child.exited!==0)throw Error("Could not write collector service");'
chmod 600 "$plist"
launchctl bootout "gui/$(id -u)/com.hey-boss.poe-errors" 2>/dev/null || true
launchctl bootstrap "gui/$(id -u)" "$plist"
echo 'Poe error collector installed. Project enablement is controlled in Errors.'
