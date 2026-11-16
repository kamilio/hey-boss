#!/bin/bash
# macOS Folder Action: pass input as arguments after PROJECT PROCESSOR.
set -u
if [ "$#" -lt 3 ]; then
  echo "usage: file-created-job.sh PROJECT PROCESSOR FILE..." >&2
  exit 2
fi
project=$1
processor=$2
shift 2
export HEY_BOSS_AGENT_ID=folder-action:files
export PATH="$PATH:/opt/homebrew/bin:/usr/local/bin"
set -o pipefail
failed=0
for file in "$@"; do
  [ -f "$file" ] && [ ! -L "$file" ] || continue
  # Redelivery of the same published file must reuse its execution.
  identity=$(stat -f '%d:%i:%B' -- "$file") || { failed=1; continue; }
  key=$(printf '%s\0' "$processor" "$file" "$identity" | shasum -a 256) || { failed=1; continue; }
  hey-boss exec submit --project "$project" --request-id "arrival-${key%% *}" \
    --input "$file" -- "$processor" input.tar.gz "${file##*/}" || failed=1
done
exit "$failed"
