#!/usr/bin/env bash
set -euo pipefail

if [[ $# != 1 || ! -f "$1/Cargo.toml" || ! -f "$1/src/dashboard.rs" ]]; then
  echo "Usage: $0 /path/to/existing/hey-gh/source" >&2
  exit 2
fi

patch_file="$(cd "$(dirname "$0")/../patches/hey-gh" && pwd)/issue64-feed-recovery.patch"
source_dir="$(cd "$1" && pwd)"

# Validate every hunk before writing. A changed source fails rather than
# accepting a partially applied fix; identical retries are harmless.
if patch --directory "$source_dir" --batch --forward --dry-run -p1 < "$patch_file" >/dev/null 2>&1; then
  patch --directory "$source_dir" --batch --forward -p1 < "$patch_file"
elif patch --directory "$source_dir" --batch --reverse --dry-run -p1 < "$patch_file" >/dev/null 2>&1; then
  echo "Issue 64 patch already applied."
else
  echo "hey-gh source differs from the issue 64 baseline; no files changed." >&2
  exit 1
fi
