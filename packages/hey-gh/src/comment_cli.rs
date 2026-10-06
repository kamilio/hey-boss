use clap::Args;
use std::{io::Read, path::PathBuf, process::Stdio};
use tokio::io::AsyncWriteExt;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Args, Clone)]
#[command(after_help = hey_gh::comments::GUIDANCE)]
pub struct CommentArgs {
    /// NUMBER, URL, or PR branch; omit for the current PR branch.
    pub selector: Option<String>,
    /// Comment text: at most 2 lines and 300 characters.
    #[arg(
        short = 'b',
        long,
        required_unless_present = "body_file",
        conflicts_with = "body_file"
    )]
    body: Option<String>,
    /// Read comment text from a UTF-8 file (use - for stdin).
    #[arg(short = 'F', long)]
    body_file: Option<PathBuf>,
}

pub async fn post(kind: &str, args: &CommentArgs, repo: Option<&str>) -> Result<()> {
    if kind == "issue" && args.selector.is_none() {
        return Err("issue comment requires a number or URL".into());
    }
    if args
        .selector
        .as_deref()
        .is_some_and(|value| value.starts_with('-') || value.trim().is_empty())
    {
        return Err("comment requires a number, URL, or branch".into());
    }
    let body = match (&args.body, &args.body_file) {
        (Some(body), None) => body.clone(),
        (None, Some(path)) => {
            let reader: Box<dyn Read> = if path.as_os_str() == "-" {
                Box::new(std::io::stdin())
            } else {
                Box::new(std::fs::File::open(path)?)
            };
            // Bound file/stdin reads, including whitespace, before allocating.
            let mut body = String::new();
            reader.take(64 * 1024 + 1).read_to_string(&mut body)?;
            if body.len() > 64 * 1024 {
                return Err(rejection().into());
            }
            body
        }
        _ => return Err("provide --body or --body-file".into()),
    };
    let body = body.trim();
    if body.is_empty() || hey_gh::comments::too_long(body) {
        return Err(rejection().into());
    }
    // Legacy cached syntax retains its validation, but shares native auth.
    let mut argv: Vec<std::ffi::OsString> = vec![kind.into(), "comment".into()];
    if let Some(selector) = &args.selector {
        argv.push(selector.into());
    }
    if let Some(repo) = repo {
        argv.extend(["--repo".into(), repo.into()]);
    }
    argv.extend(["--body-file".into(), "-".into()]);
    let invocation = crate::cli_route::Invocation::parse(argv)?;
    let native = tokio::task::spawn_blocking(move || {
        crate::cli_auth::command(&invocation).map_err(|error| error.to_string())
    })
    .await??;
    let mut command = tokio::process::Command::from(native);
    let mut child = command.stdin(Stdio::piped()).kill_on_drop(true).spawn()?;
    let write = child
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(body.as_bytes())
        .await;
    let status = child.wait().await?;
    if !status.success() || write.is_err() {
        return Err("GitHub comment failed; not retried. Check GitHub before retrying because the comment may already have been posted.".into());
    }
    Ok(())
}

fn rejection() -> String {
    format!(
        "{}\nComments must contain text and be at most 2 lines and 300 characters.",
        hey_gh::comments::GUIDANCE.trim()
    )
}
