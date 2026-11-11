#!/bin/bash
# Usage: worktrees.sh /abs/path/to/g /abs/path/to/hey-boss
set -euo pipefail
g=$1
hb=$2
fixture=$(mktemp -d "$PWD/.shelf-check.XXXXXX")
export HEY_BOSS_ISSUE_DB="$fixture/issues.db" HEY_BOSS_FLEET_STATE="$fixture/fleet" HEY_BOSS_AGENT_ID=human:shelf-test
unset HEY_BOSS_ISSUE_HOST HEY_BOSS_ISSUE_PROJECT
db_key=$(printf %s "$HEY_BOSS_ISSUE_DB" | shasum -a 256 | cut -c1-24)
socket="/tmp/hey-boss-db-$(id -u)/$db_key"
"$hb" fleet companion >"$fixture/service.log" 2>&1 &
service=$!
cleanup() {
    kill "$service" 2>/dev/null || true
    wait "$service" 2>/dev/null || true
    rm -f "$socket.sock" "$socket.lock" "$socket.startup" "$socket.log"
    rm -rf "$fixture"
}
trap cleanup EXIT
for ((i=0;i<200;i++)); do
    [[ -S "$socket.sock" ]] && break
    kill -0 "$service" || exit 1
    sleep 0.05
done
test -S "$socket.sock"
mkdir "$fixture/bin"
ln -s "$hb" "$fixture/bin/hey-boss"
export PATH="$fixture/bin:$PATH" GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1
git init -qb main "$fixture/a"
cd "$fixture/a"
git config user.email test@example.invalid
git config user.name Test
git remote add origin https://github.com/example/shelf-fixture.git
echo base > fruit
git add fruit && git commit -qm Base
base=$(git rev-parse HEAD)
git worktree add -qb other "$fixture/b"
git clone -q . "$fixture/clone"
git -C "$fixture/clone" remote set-url origin https://github.com/example/shelf-fixture.git
git -C "$fixture/clone" config user.name Test
git -C "$fixture/clone" config user.email test@example.invalid
"$hb" project init --yes --prs false --worktree true >/dev/null
"$hb" issue create --title 'Review fruit' --body 'Review fixture' >/dev/null
echo apple > fruit
git commit -qam Apples
tip=$(git rev-parse HEAD)
if "$g" stash --task 999; then exit 1; fi
test "$(git rev-parse HEAD)" = "$tip"
"$g" stash
test "$(git rev-parse HEAD)" = "$base"
cd "$fixture/b"
"$g" list | grep Apples
"$g" comment "$tip" --file fruit --line 1 --body 'Try pears'
"$g" link "$tip" --task 1
"$hb" issue commit comments 1 "$tip" --json | jq -e '.comments[0].body=="Try pears"' >/dev/null
"$g" open "$tip" | grep 'Try pears'
echo pears > fruit && git commit -qam Pears
other=$(git rev-parse HEAD)
if "$g" unstash "$tip"; then exit 1; fi
test -n "$(git ls-files -u)"
git cherry-pick --abort
test "$(git rev-parse HEAD)" = "$other"
test "$(cat fruit)" = pears
if "$g" unstash "$tip"; then exit 1; fi
echo resolved > fruit && git add fruit && git -c core.editor=true cherry-pick --continue
test "$(cat fruit)" = resolved
cd "$fixture/clone"
! git cat-file -e "$tip" 2>/dev/null || exit 1
"$g" unstash "$tip"
test "$(cat fruit)" = apple
"$g" list | grep Apples
cd "$fixture/a"
# Dirty index and worktree both survive a refused stash.
echo 'proposal' > notes && git add notes && git commit -qm Notes
notes=$(git rev-parse HEAD)
echo editing >> notes
if "$g" stash; then exit 1; fi
git add notes
if "$g" stash; then exit 1; fi
test "$(git rev-parse HEAD)" = "$notes"
git restore --staged --worktree notes
if "$g" stash "$base"; then exit 1; fi
"$g" stash "$notes" --task 1
# Ignored as well as ordinary untracked files are protected.
echo notes >> .git/info/exclude
echo 'private scratch' > notes
if "$g" unstash "$notes"; then exit 1; fi
test "$(cat notes)" = 'private scratch'
test "$(git rev-parse HEAD)" = "$base"
rm notes
echo 'keep this' > scratch
"$g" unstash "$notes"
test "$(cat scratch)" = 'keep this'
"$hb" issue commit remove 1 "$notes" >/dev/null
"$g" open "$notes" | grep 'proposal'
# Git handles rename/rename, modify/delete and binary conflicts; abort must
# restore exactly the clean divergent branch and leave unrelated files alone.
for kind in rename delete binary; do
  git switch -q -c "$kind" "$base"
  printf 'base\0binary\n' > asset
  git add asset && git commit -qm 'Add asset'
  common=$(git rev-parse HEAD)
  case "$kind" in
    rename) git mv fruit apple;;
    delete) git rm -q fruit;;
    binary) printf 'new\0binary' > asset;;
  esac
  git commit -qam "$kind incoming"
  incoming=$(git rev-parse HEAD)
  "$g" stash "$incoming"
  test "$(git rev-parse HEAD)" = "$common"
  case "$kind" in
    rename) git mv fruit pear;;
    delete) echo local > fruit;;
    binary) printf 'local\0binary' > asset;;
  esac
  git commit -qam "$kind local"
  before=$(git rev-parse HEAD)
  if "$g" unstash "$incoming"; then exit 1; fi
  test -n "$(git ls-files -u)"
  git cherry-pick --abort
  test "$(git rev-parse HEAD)" = "$before"
  git diff --quiet && git diff --cached --quiet
  test "$(cat scratch)" = 'keep this'
done
# A tracked file deleted by HEAD can reappear locally as ignored. Saving must
# never overwrite that file when moving the branch back to its parent.
git switch -q -c protect "$base"
git rm -q fruit && git commit -qm 'Remove fruit'
removed=$(git rev-parse HEAD)
echo fruit >> .git/info/exclude
echo mine > fruit
if "$g" stash; then exit 1; fi
test "$(git rev-parse HEAD)" = "$removed"
test "$(cat fruit)" = mine
