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
node -e 'const fs=require("fs"); const raw=JSON.parse(fs.readFileSync(process.argv[1])); if (!raw.includes("Review <script> safely") || !raw.includes("+<script>never execute</script>")) process.exit(1)' "$fixture/commit.json"
for asset in '' review.js components.css commit-review.css diff.js git-page.js git-rows.js git-files.js git-activity.js git-layout.js; do
    curl -fsS "${url}${asset}" > /dev/null
done
test "$(curl -s -o /dev/null -w '%{http_code}' "${url%/*/}/commit.json")" = 404
test "$(curl -s -X POST -o /dev/null -w '%{http_code}' "${url}commit.json")" = 405
test "$(git rev-parse HEAD)" = "$before"
test -z "$(git status --porcelain)"
kill "$(cat "$fixture/viewer.pid")"
for result in fail missing; do
    if OPENER_RESULT=$result "$g" open HEAD > "$fixture/output" 2>&1; then exit 1; fi
    grep -q 'Could not open the commit' "$fixture/output"
    if kill -0 "$(cat "$fixture/viewer.pid")" 2>/dev/null; then exit 1; fi
done
if "$g" open deadbeef > "$fixture/output" 2>&1; then exit 1; fi
echo 'PASS: local commit browser handoff, private assets, Git preservation, and failed-open cleanup'
