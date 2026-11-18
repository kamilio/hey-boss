#!/bin/bash
# Usage: open.sh /abs/path/to/g
set -euo pipefail
g=$1
fixture=$(mktemp -d "$PWD/.shelf-open.XXXXXX")
cleanup() {
    if test -f "$fixture/viewer.pid"; then kill "$(cat "$fixture/viewer.pid")" 2>/dev/null || true; fi
    rm -rf "$fixture"
}
trap cleanup EXIT
export fixture
mkdir "$fixture/bin" "$fixture/repo"
cat > "$fixture/bin/hey-boss" <<'SH'
#!/bin/bash
set -eu
if [[ "$1 $2" == 'issue shelf' ]]; then
    cat >/dev/null
    printf '%s\n' '{"ok":true,"entries":[]}'
elif [[ "$1 $2" == 'browser open' ]]; then
    printf '%s' "$3" > "$fixture/url"
    pgrep -P "$PPID" -f serve-review > "$fixture/viewer.pid"
    case ${OPENER_RESULT:-ok} in
      fail) echo 'Mac is disconnected' >&2; exit 1 ;;
      missing) echo '{}' ;;
      *) printf '%s\n' '{"version":1,"result":{"session":"test","url":"http://localhost/"}}' ;;
    esac
else
    exit 2
fi
SH
chmod +x "$fixture/bin/hey-boss"
export PATH="$fixture/bin:$PATH" GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1
cd "$fixture/repo"
git init -q
git config user.name Test
git config user.email test@example.invalid
printf '%s\n' '<script>never execute</script>' > example.rs
git add example.rs
git commit -qm 'Review <script> safely'
before=$(git rev-parse HEAD)
"$g" open HEAD > "$fixture/output"
grep -q 'Opened commit HEAD on the connected Mac' "$fixture/output"
url=$(cat "$fixture/url")
curl -fsS "${url}commit.json" > "$fixture/commit.json"
node -e 'const fs=require("fs"); const {raw}=JSON.parse(fs.readFileSync(process.argv[1])); if (!raw.includes("Review <script> safely") || !raw.includes("+<script>never execute</script>")) process.exit(1)' "$fixture/commit.json"
for asset in '' review.js components.css commit-review.css diff.js git-page.js git-rows.js git-files.js git-activity.js git-layout.js; do
    curl -fsS "${url}${asset}" > /dev/null
done
test "$(curl -s -o /dev/null -w '%{http_code}' "${url%/*/}/commit.json")" = 404
test "$(curl -s -X POST -o /dev/null -w '%{http_code}' "${url}commit.json")" = 405
test "$(git rev-parse HEAD)" = "$before"
test -z "$(git status --porcelain)"
# Editing preserves dirty files and the index and requires the private review token.
token=/${url#http://*/}
printf '%s\n' staged >> example.rs
git add example.rs
printf '%s\n' unstaged >> example.rs
printf '%s\n' keep > untracked
git diff > "$fixture/unstaged"
git diff --cached > "$fixture/staged"
node -e 'console.log(JSON.stringify({sha:process.argv[1],message:"Edited subject\n\nBody <script> stays text"}))' "$before" > "$fixture/edit.json"
test "$(curl -s -X POST --data-binary @"$fixture/edit.json" -o /dev/null -w '%{http_code}' "${url}message")" = 409
test "$(git rev-parse HEAD)" = "$before"
curl -fsS -H "X-Git-Shelf-Review: $token" --data-binary @"$fixture/edit.json" "${url}message" > "$fixture/saved.json"
edited=$(git rev-parse HEAD)
test "$edited" != "$before"
test "$(git rev-parse HEAD^{tree})" = "$(git rev-parse "$before^{tree}")"
test "$(git log -1 --format=%s)" = 'Edited subject'
git diff > "$fixture/unstaged-after"
git diff --cached > "$fixture/staged-after"
cmp "$fixture/unstaged" "$fixture/unstaged-after"
cmp "$fixture/staged" "$fixture/staged-after"
test "$(cat untracked)" = keep
test "$(curl -s -H "X-Git-Shelf-Review: $token" --data-binary @"$fixture/edit.json" -o /dev/null -w '%{http_code}' "${url}message")" = 409
kill "$(cat "$fixture/viewer.pid")"
# An older commit can be edited without losing its descendants or their messages.
git add example.rs
git commit -qm 'Newer commit' -m 'Preserve this body'
git show -s --format=%B HEAD > "$fixture/newer-message"
tree=$(git rev-parse HEAD^{tree})
"$g" open "$edited" > /dev/null
url=$(cat "$fixture/url")
token=/${url#http://*/}
node -e 'console.log(JSON.stringify({sha:process.argv[1],message:"Edited ancestor"}))' "$edited" > "$fixture/edit.json"
curl -fsS -H "X-Git-Shelf-Review: $token" --data-binary @"$fixture/edit.json" "${url}message" > "$fixture/saved.json"
test "$(git rev-parse HEAD^{tree})" = "$tree"
test "$(git log -1 --format=%s HEAD^)" = 'Edited ancestor'
git show -s --format=%B HEAD > "$fixture/newer-message-after"
cmp "$fixture/newer-message" "$fixture/newer-message-after"
git commit --allow-empty -qm 'Concurrent change'
node -e 'const fs=require("fs"); console.log(JSON.stringify({sha:JSON.parse(fs.readFileSync(process.argv[1])).raw.split("\0")[0],message:"Stale edit"}))' "$fixture/saved.json" > "$fixture/edit.json"
test "$(curl -s -H "X-Git-Shelf-Review: $token" --data-binary @"$fixture/edit.json" -o "$fixture/error" -w '%{http_code}' "${url}message")" = 409
grep -q 'Branch changed' "$fixture/error"
kill "$(cat "$fixture/viewer.pid")"
# A commit opened from another checkout edits only its owning worktree's branch.
current=$(git rev-parse HEAD)
git worktree add -qb owning "$fixture/other"
git -C "$fixture/other" commit --allow-empty -qm 'Other worktree'
other=$(git -C "$fixture/other" rev-parse HEAD)
"$g" open "$other" > /dev/null
url=$(cat "$fixture/url")
token=/${url#http://*/}
node -e 'console.log(JSON.stringify({sha:process.argv[1],message:"Edited in owning worktree"}))' "$other" > "$fixture/edit.json"
curl -fsS -H "X-Git-Shelf-Review: $token" --data-binary @"$fixture/edit.json" "${url}message" > /dev/null
test "$(git rev-parse HEAD)" = "$current"
test "$(git -C "$fixture/other" log -1 --format=%s)" = 'Edited in owning worktree'
kill "$(cat "$fixture/viewer.pid")"
# Signing uses Git's configured signer; a failed signer cannot move the branch.
git config commit.gpgsign true
git config gpg.program false
"$g" open HEAD > /dev/null
url=$(cat "$fixture/url")
token=/${url#http://*/}
node -e 'console.log(JSON.stringify({sha:process.argv[1],message:"Signed edit"}))' "$current" > "$fixture/edit.json"
test "$(curl -s -H "X-Git-Shelf-Review: $token" --data-binary @"$fixture/edit.json" -o /dev/null -w '%{http_code}' "${url}message")" = 409
test "$(git rev-parse HEAD)" = "$current"
ssh-keygen -q -t ed25519 -N '' -f "$fixture/signing"
git config gpg.format ssh
git config user.signingkey "$fixture/signing"
printf 'test@example.invalid %s\n' "$(cat "$fixture/signing.pub")" > "$fixture/signers"
git config gpg.ssh.allowedSignersFile "$fixture/signers"
curl -fsS -H "X-Git-Shelf-Review: $token" --data-binary @"$fixture/edit.json" "${url}message" > /dev/null
git verify-commit HEAD > "$fixture/signature" 2>&1
kill "$(cat "$fixture/viewer.pid")"
for result in fail missing; do
    if OPENER_RESULT=$result "$g" open HEAD > "$fixture/output" 2>&1; then exit 1; fi
    grep -q 'Could not open the commit' "$fixture/output"
    if kill -0 "$(cat "$fixture/viewer.pid")" 2>/dev/null; then exit 1; fi
done
if "$g" open deadbeef > "$fixture/output" 2>&1; then exit 1; fi
echo 'PASS: browser handoff, commit editing, dirty files, descendants, owning worktree, stale saves, and failed-open cleanup'
