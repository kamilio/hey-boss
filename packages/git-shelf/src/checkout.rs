use super::*;
use std::{
    fs,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
};

fn git(args: &[&str]) -> Result<String> {
    let o = Command::new("git").args(args).output()?;
    if !o.status.success() {
        return Err(String::from_utf8_lossy(&o.stderr).trim().to_string().into());
    }
    Ok(String::from_utf8(o.stdout)?.trim_end_matches('\n').into())
}
pub fn to_root() -> Result<()> {
    std::env::set_current_dir(git(&["rev-parse", "--show-toplevel"])?)?;
    Ok(())
}
fn file(name: &str) -> Result<PathBuf> {
    Ok(PathBuf::from(git(&["rev-parse", "--git-path", name])?))
}
fn ready() -> Result<String> {
    for op in [
        "MERGE_HEAD",
        "CHERRY_PICK_HEAD",
        "REVERT_HEAD",
        "REBASE_HEAD",
        "rebase-apply",
        "rebase-merge",
        "sequencer",
        "BISECT_START",
    ] {
        if file(op)?.exists() {
            return Err("Finish or abort the Git operation in this worktree first".into());
        }
    }
    let branch = git(&["symbolic-ref", "--quiet", "HEAD"])
        .map_err(|_| "Switch to a local branch before changing this checkout")?;
    if !git(&[
        "status",
        "--porcelain",
        "--untracked-files=no",
        "--ignore-submodules=none",
    ])?
    .is_empty()
    {
        return Err("Commit or put aside staged and unstaged changes first".into());
    }
    Ok(branch)
}

fn input(args: &[&str], bytes: &[u8], index: Option<&Path>) -> Result<String> {
    let mut cmd = Command::new("git");
    if let Some(path) = index {
        cmd.env("GIT_INDEX_FILE", path);
    }
    let mut child = cmd
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;
    let sent = child.stdin.take().unwrap().write_all(bytes);
    let out = child.wait_with_output()?;
    if !out.status.success() {
        return Err(format!("git {} failed; the shelf is unchanged", args[0]).into());
    }
    sent?;
    Ok(String::from_utf8(out.stdout)?.trim_end().into())
}

// Recreate the exact original Git object in another clone, including signed
// headers. A private index keeps this entirely outside the user's checkout.
fn materialize(v: &Value, verify_tree: bool) -> Result<String> {
    let sha = field(v, "sha")?;
    let raw = STANDARD.decode(field(v, "raw_commit")?)?;
    let r = &v["review"];
    if input(&["hash-object", "-t", "commit", "--stdin"], &raw, None)? != sha {
        return Err("Stored commit failed its object ID check".into());
    }
    if !verify_tree
        && Command::new("git")
            .args(["cat-file", "-e", &format!("{sha}^{{commit}}")])
            .stderr(Stdio::null())
            .status()?
            .success()
    {
        return Ok(sha.into());
    }
    let parent = field(r, "parent")?;
    if !matches!(parent.len(), 40 | 64) || !parent.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err("Invalid stored parent".into());
    }
    let dir = file(&format!("shelf-index-{}", std::process::id()))?;
    fs::create_dir(&dir)?;
    let result = (|| -> Result<()> {
        let index = dir.join("index");
        input(&["read-tree", parent], &[], Some(&index))
            .map_err(|_| format!("Fetch parent {parent} into this clone before restoring"))?;
        let patch = field(r, "patch")?;
        if !patch.is_empty() {
            input(
                &[
                    "-c",
                    "apply.ignoreWhitespace=no",
                    "apply",
                    "--cached",
                    "--binary",
                    "--whitespace=nowarn",
                    "-",
                ],
                patch.as_bytes(),
                Some(&index),
            )?;
        }
        let tree = input(&["write-tree"], &[], Some(&index))?;
        if !raw.starts_with(format!("tree {tree}\nparent {parent}\n").as_bytes()) {
            return Err(
                "Stored patch does not reproduce the original commit tree; branch unchanged".into(),
            );
        }
        input(
            &["hash-object", "-w", "-t", "commit", "--stdin"],
            &raw,
            None,
        )?;
        Ok(())
    })();
    let _ = fs::remove_dir_all(dir);
    result?;
    Ok(sha.into())
}

pub fn stash(expected: Option<&str>, task: Option<i64>) -> Result<String> {
    let sha = head()?;
    if expected.is_some_and(|s| s.len() < 4 || !sha.starts_with(&s.to_ascii_lowercase())) {
        return Err("Selected commit is not the current branch tip; nothing changed".into());
    }
    stash_selected(&sha, &sha, task)
}

pub fn stash_selected(sha: &str, expected_head: &str, task: Option<i64>) -> Result<String> {
    let branch = ready()?;
    if head()? != expected_head {
        return Err("Branch tip changed; reopen the shelf and select the commit again".into());
    }
    let parents = git(&["show", "-s", "--format=%P", sha])?;
    let mut ps = parents.split_whitespace();
    let parent = ps.next().ok_or("Cannot shelve the root commit")?;
    if ps.next().is_some() {
        return Err("Shelve an individual commit, not a merge".into());
    }
    let next = if sha == expected_head {
        parent.to_owned()
    } else {
        if !active(expected_head)?.iter().any(|e| e["sha"] == sha) {
            return Err("Selected commit is no longer ahead of the remote base".into());
        }
        let range = format!("{sha}..{branch}");
        if !git(&["rev-list", "--merges", &range])?.is_empty() {
            return Err("Cannot shelve through a merge; branch unchanged".into());
        }
        // Replay in the object database. Older Git only prints ref updates;
        // newer Git must be told not to apply them.
        let updates = git(&[
            "-c",
            "replay.refAction=print",
            "replay",
            "--onto",
            parent,
            &range,
        ])
        .map_err(|e| format!("Could not replay newer commits; branch unchanged. {e}"))?;
        let next = updates
            .lines()
            .find_map(|l| {
                let w: Vec<_> = l.split_whitespace().collect();
                match w.as_slice() {
                    ["update", r, new, old] if *r == branch && *old == expected_head => {
                        Some(new.to_string())
                    }
                    _ => None,
                }
            })
            .ok_or("Branch changed or Git could not replay this stack; nothing changed")?;
        if git(&["rev-list", "--count", &format!("{sha}..{expected_head}")])?
            != git(&["rev-list", "--count", &format!("{parent}..{next}")])?
        {
            return Err("Replay would omit a newer commit; branch unchanged".into());
        }
        next
    };
    let raw = Command::new("git")
        .args(["cat-file", "commit", sha])
        .output()?;
    if !raw.status.success() {
        return Err("Could not read HEAD".into());
    }
    rpc(json!({"command":"save","sha":sha,"raw_commit":STANDARD.encode(raw.stdout),"task":task}))?;
    let saved = open(sha)?;
    if materialize(&saved, true)? != sha || task.is_some_and(|n| saved["number"] != n) {
        return Err("Could not verify the shelved commit and task; branch unchanged".into());
    }
    git(&["update-ref", &format!("refs/hey-boss/shelf/{sha}"), sha])?;
    if sha != expected_head {
        git(&[
            "update-ref",
            &format!("refs/hey-boss/shelf-recovery/{expected_head}"),
            expected_head,
        ])?;
    }
    // Lock HEAD at the expected SHA before touching the index. Two concurrent
    // shelves cannot remove two tips. read-tree refuses obstructing local files.
    let mut txn = Command::new("git")
        .args(["update-ref", "-m", "g stash-commit", "--stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;
    let mut pipe = txn.stdin.take().unwrap();
    let mut replies = BufReader::new(txn.stdout.take().unwrap());
    let step = (|| -> Result<()> {
        writeln!(pipe, "start\nupdate HEAD {next} {expected_head}\nprepare")?;
        for expect in ["start: ok", "prepare: ok"] {
            let mut line = String::new();
            replies.read_line(&mut line)?;
            if line.trim() != expect {
                return Err("Branch changed while shelving; the saved commit is safe".into());
            }
        }
        if ready()? != branch {
            return Err("Worktree changed while shelving; the saved commit is safe".into());
        }
        protect_loose(expected_head, &next)?;
        git(&["read-tree", "-m", "-u", expected_head, &next])?;
        writeln!(pipe, "commit")?;
        Ok(())
    })();
    drop(pipe);
    let status = txn.wait()?;
    step?;
    if !status.success() {
        return Err(
            "Branch update failed; inspect git status. The shelf and recovery ref are intact"
                .into(),
        );
    }
    Ok(format!(
        "Shelved {} from {}",
        &sha[..10],
        branch.trim_start_matches("refs/heads/")
    ))
}

pub fn unstash(prefix: &str) -> Result<String> {
    ready()?;
    let sha = materialize(&open(prefix)?, false)?;
    protect_loose(&format!("{sha}^"), &sha)?;
    let out = Command::new("git").args(["cherry-pick", &sha]).output()?;
    if !out.status.success() {
        if file("CHERRY_PICK_HEAD")?.exists() {
            let paths = git(&["diff", "--name-only", "--diff-filter=U"])?;
            let detail = if paths.is_empty() {
                String::from_utf8_lossy(&out.stderr).trim().to_owned()
            } else {
                format!(
                    "Conflicts: {}",
                    paths.lines().collect::<Vec<_>>().join(", ")
                )
            };
            return Err(format!("{detail}. Shelf retained. Resolve and git add, then git cherry-pick --continue; or git cherry-pick --abort.").into());
        }
        return Err(format!(
            "{}; shelf retained",
            String::from_utf8_lossy(&out.stderr).trim()
        )
        .into());
    }
    Ok(format!(
        "Restored {}. The shelf and review are still available.",
        &sha[..10]
    ))
}

// Git's merge machinery may overwrite ignored files. Limit this check to paths
// touched by the commit, not node_modules or every untracked file in the repo.
fn protect_loose(from: &str, to: &str) -> Result<()> {
    use std::{
        collections::BTreeSet,
        ffi::OsStr,
        os::unix::ffi::{OsStrExt, OsStringExt},
    };
    let touched = Command::new("git")
        .args(["diff", "--no-renames", "--name-only", "-z", from, to, "--"])
        .output()?;
    if !touched.status.success() {
        return Err("Could not inspect changed paths".into());
    }
    let mut candidates = BTreeSet::new();
    for path in touched.stdout.split(|b| *b == 0).filter(|p| !p.is_empty()) {
        let path = Path::new(OsStr::from_bytes(path));
        // A file or symlink can also obstruct a directory higher up the path.
        for p in path.ancestors().take_while(|p| !p.as_os_str().is_empty()) {
            if let Ok(m) = fs::symlink_metadata(p)
                && (p == path || !m.is_dir())
            {
                candidates.insert(p);
            }
        }
    }
    let paths: Vec<_> = candidates.into_iter().collect();
    for chunk in paths.chunks(32) {
        let loose = Command::new("git")
            .args([
                "--literal-pathspecs",
                "ls-files",
                "--others",
                "--directory",
                "-z",
                "--",
            ])
            .args(chunk)
            .output()?;
        if !loose.status.success() {
            return Err("Could not check untracked paths".into());
        }
        if let Some(path) = loose.stdout.split(|b| *b == 0).find(|p| !p.is_empty()) {
            return Err(format!(
                "Untracked path could be overwritten: {}",
                PathBuf::from(std::ffi::OsString::from_vec(path.to_vec())).display()
            )
            .into());
        }
    }
    Ok(())
}

pub fn branch() -> Result<String> {
    git(&["branch", "--show-current"])
}

pub fn tip() -> Result<String> {
    let branch = branch()?;
    let subject = git(&["log", "-1", "--format=%h · %s"]).unwrap_or_else(|_| "No commits".into());
    Ok(format!(
        "{}  /  {}",
        if branch.is_empty() {
            "detached HEAD"
        } else {
            &branch
        },
        clean(&subject)
    ))
}

// Match the original shelf: compare with the tracking remote's default branch,
// even when this branch has an upstream of its own.
pub fn active(head: &str) -> Result<Vec<Value>> {
    let upstream = git(&["rev-parse", "--symbolic-full-name", "@{upstream}"]).ok();
    let remotes = git(&["remote"])?;
    let remote = upstream
        .as_deref()
        .and_then(|r| r.strip_prefix("refs/remotes/"))
        .and_then(|r| {
            remotes
                .lines()
                .find(|name| r.starts_with(&format!("{name}/")))
        })
        .or_else(|| remotes.lines().find(|r| *r == "origin"))
        .or_else(|| remotes.lines().next());
    if head.is_empty() {
        return Ok(vec![]);
    }
    let base = remote.and_then(|r| {
        [
            Some(format!("refs/remotes/{r}/HEAD")),
            upstream.clone(),
            Some(format!("refs/remotes/{r}/master")),
            Some(format!("refs/remotes/{r}/main")),
        ]
        .into_iter()
        .flatten()
        .find_map(|name| git(&["rev-parse", "--verify", &format!("{name}^{{commit}}")]).ok())
    });
    // Until the first fetch there is no known range. Keep the current tip
    // available, without treating the entire repository history as local.
    let range = base.as_ref().map(|b| format!("{b}..{head}"));
    let mut args = vec!["log", "--format=%H%x00%s", "--reverse"];
    if range.is_none() {
        args.push("-1");
    }
    args.extend([range.as_deref().unwrap_or(head), "--"]);
    Ok(git(&args)?
        .lines()
        .filter_map(|l| l.split_once('\0'))
        .map(|(sha, title)| json!({"sha":sha,"title":title,"active":true}))
        .collect())
}

pub fn head() -> Result<String> {
    git(&["rev-parse", "--verify", "HEAD"])
}

pub fn preview(sha: &str) -> Result<Value> {
    let output = git(&[
        "show",
        "--no-color",
        "--no-ext-diff",
        "--no-textconv",
        "--format=%an%x00%B%x00",
        "--patch",
        sha,
        "--",
    ])?;
    let mut fields = output.splitn(3, '\0');
    Ok(
        json!({"sha":sha,"comments":[],"review":{"author":fields.next(),"message":fields.next(),
        "patch":fields.next().unwrap_or("").trim_start_matches('\n')}}),
    )
}
