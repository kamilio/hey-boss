use crate::*;
use std::collections::HashMap;

fn git(args: &[&str]) -> Result<String> {
    let output = Command::new("git").args(args).output()?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr)
            .trim()
            .to_string()
            .into());
    }
    Ok(String::from_utf8(output.stdout)?
        .trim_end_matches('\n')
        .into())
}

pub fn target(sha: &str) -> Result<(String, String)> {
    let branches = git(&[
        "for-each-ref",
        "--format=%(refname)",
        "--contains",
        sha,
        "refs/heads",
    ])?;
    let current = git(&["symbolic-ref", "--quiet", "HEAD"]).unwrap_or_default();
    let branches: Vec<_> = branches.lines().collect();
    let branch = if branches.contains(&current.as_str()) {
        current.as_str()
    } else if branches.len() == 1 {
        branches[0]
    } else {
        return Err("Open this commit from its branch's worktree to edit its message".into());
    };
    Ok((branch.into(), git(&["rev-parse", branch])?))
}

pub fn body(sha: &str) -> Result<String> {
    let output = Command::new("git")
        .args(["cat-file", "commit", sha])
        .output()?;
    if !output.status.success() {
        return Err("Could not read the commit message".into());
    }
    let raw = String::from_utf8(output.stdout)?;
    Ok(raw
        .split_once("\n\n")
        .ok_or("Incomplete commit object")?
        .1
        .into())
}

pub fn message(sha: &str, branch: &str, expected: &str, message: &str) -> Result<(String, String)> {
    if message.trim().is_empty() || message.contains('\0') || message.len() > 65536 {
        return Err("Enter a nonempty commit message of at most 64 KiB".into());
    }
    if git(&["rev-parse", branch])? != expected {
        return Err(
            "Branch changed since this review opened. Run g open again before saving".into(),
        );
    }
    // A message-only rewrite leaves every tree, the index, and local files intact.
    // Refuse to interfere with an operation in the branch's owning worktree.
    let worktrees = git(&["worktree", "list", "--porcelain", "-z"])?;
    let mut path = "";
    for field in worktrees.split('\0') {
        if let Some(value) = field.strip_prefix("worktree ") {
            path = value;
        }
        if field == format!("branch {branch}") {
            for operation in [
                "MERGE_HEAD",
                "CHERRY_PICK_HEAD",
                "REVERT_HEAD",
                "rebase-apply",
                "rebase-merge",
                "sequencer",
                "BISECT_START",
            ] {
                let file = git(&[
                    "-C",
                    path,
                    "rev-parse",
                    "--path-format=absolute",
                    "--git-path",
                    operation,
                ])?;
                if std::path::Path::new(&file).exists() {
                    return Err(
                        "Finish or abort the Git operation in the commit's worktree before editing"
                            .into(),
                    );
                }
            }
        }
    }
    if !git(&[
        "for-each-ref",
        "--format=%(refname)",
        "--contains",
        sha,
        "refs/remotes",
    ])?
    .is_empty()
    {
        return Err("This commit is already on a remote branch; edit an unpublished commit".into());
    }
    let message = format!("{}\n", message.trim_end());
    if body(sha)? == message {
        return Ok((sha.into(), expected.into()));
    }
    let signing = Command::new("git")
        .args(["config", "--bool", "--get", "commit.gpgsign"])
        .output()?;
    if !signing.status.success() && signing.status.code() != Some(1) {
        return Err(String::from_utf8_lossy(&signing.stderr)
            .trim()
            .to_string()
            .into());
    }
    let sign = signing.stdout == b"true\n";
    let descendants = git(&[
        "rev-list",
        "--reverse",
        "--topo-order",
        "--ancestry-path",
        &format!("{sha}..{expected}"),
    ])?;
    let mut rewritten = HashMap::<String, String>::new();
    for old in std::iter::once(sha).chain(descendants.lines()) {
        let metadata = git(&["show", "-s", "--format=%T%x00%P%x00%an%x00%ae%x00%aI", old])?;
        let fields: Vec<_> = metadata.splitn(5, '\0').collect();
        if fields.len() != 5 {
            return Err("Incomplete commit metadata".into());
        }
        let mut command = Command::new("git");
        command.args(["commit-tree", fields[0], "-F", "-"]);
        if sign {
            command.arg("-S");
        }
        for parent in fields[1].split_whitespace() {
            command.args([
                "-p",
                rewritten.get(parent).map(String::as_str).unwrap_or(parent),
            ]);
        }
        let mut child = command
            .env("GIT_AUTHOR_NAME", fields[2])
            .env("GIT_AUTHOR_EMAIL", fields[3])
            .env("GIT_AUTHOR_DATE", fields[4])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let body = if old == sha {
            message.clone()
        } else {
            body(old)?
        };
        let sent = child.stdin.take().unwrap().write_all(body.as_bytes());
        let output = child.wait_with_output()?;
        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr)
                .trim()
                .to_string()
                .into());
        }
        sent?;
        rewritten.insert(old.into(), String::from_utf8(output.stdout)?.trim().into());
    }
    let next = rewritten
        .get(expected)
        .ok_or("Commit is no longer on this branch")?;
    git(&[
        "update-ref",
        "-m",
        "g edit commit message",
        branch,
        next,
        expected,
    ])?;
    Ok((rewritten[sha].clone(), next.clone()))
}
