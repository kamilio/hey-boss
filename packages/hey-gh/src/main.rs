use clap::{Args as ClapArgs, Parser, Subcommand};
use hey_gh::{ApiClient, Client, Config, Freshness};
use serde_json::Value;
use std::{net::SocketAddr, path::PathBuf, time::Duration};

mod log_summary;
mod logging;
mod read_deadline;
mod skill_install;

#[derive(Parser)]
#[command(
    version,
    about = "Cached GitHub PR reports and a durable incremental local API"
)]
struct Args {
    #[arg(long, global = true, default_value = "http://127.0.0.1:8787")]
    server: String,
    #[command(subcommand)]
    command: Option<Command>,
    #[arg(short = 'R', long = "repo", global = true)]
    repo: Option<String>,
    #[arg(long, global = true)]
    cursor: Option<String>,
    /// Total single-PR read budget in seconds, including selector resolution.
    /// Returns cached evidence or unavailable JSON with exit 1 at the deadline.
    #[arg(long, global = true, value_parser = clap::value_parser!(u64).range(1..=3600))]
    timeout: Option<u64>,
}

#[derive(Subcommand)]
enum Command {
    /// Read recent rotated daemon diagnostics without connecting to the daemon.
    Logs {
        #[arg(long, default_value_t = 100)]
        tail: usize,
        /// Print the log directory instead of its contents.
        #[arg(long)]
        path: bool,
        /// Print safe JSON aggregates across retained logs, without the daemon.
        #[arg(long, conflicts_with_all = ["path", "tail"])]
        summary: bool,
        /// Summary window in seconds (defaults to 900).
        #[arg(long, requires = "summary", value_parser = clap::value_parser!(u64).range(1..=86400))]
        since: Option<u64>,
        #[arg(long)]
        log_dir: Option<PathBuf>,
    },
    /// Install or update the global hey-gh command-card skill for coding agents.
    Install {
        /// Override global destinations with one or more skill root directories.
        #[arg(long = "skills-dir")]
        skills_dirs: Vec<PathBuf>,
    },
    /// Start the shared queue/cache/API. Uses your existing gh auth login.
    Serve {
        #[arg(long, default_value = "127.0.0.1:8787")]
        listen: SocketAddr,
        #[arg(long)]
        cache: Option<PathBuf>,
        /// Override the private, rotated daemon log directory.
        #[arg(long)]
        log_dir: Option<PathBuf>,
        #[arg(long, default_value = "github.com")]
        hostname: String,
        #[arg(long, default_value_t = 256)]
        queue_capacity: usize,
    },
    /// Status of all your open PRs, or gh-style list/view/checks commands.
    Pr(PrArgs),
    /// Fetch detailed CI for a PR without GraphQL or comment requests.
    Ci {
        repository: String,
        number: u64,
        #[arg(long, conflicts_with = "cached_only")]
        refresh: bool,
        #[arg(long)]
        cached_only: bool,
    },
    /// Read all last-known snapshots and an atomic starting cursor.
    Snapshot,
    /// List your open PRs in a repository.
    Mine {
        repository: String,
        #[arg(long)]
        refresh: bool,
    },
    /// Start durable polling. Omit NUMBER to discover and monitor your open PRs.
    Watch {
        repository: String,
        number: Option<u64>,
        #[arg(long, default_value_t = 60)]
        interval: u64,
    },
    /// Monitor default/selected branches and the local refs of watched PRs.
    WatchRepo {
        repository: String,
        #[arg(long = "branch")]
        branches: Vec<String>,
        #[arg(long)]
        all_branches: bool,
        #[arg(long, default_value_t = 60)]
        interval: u64,
    },
    /// Reconcile branch tips and new commits into the cursor feed.
    Repo {
        repository: String,
        #[arg(long = "branch")]
        branches: Vec<String>,
        #[arg(long)]
        all_branches: bool,
        #[arg(long, conflicts_with = "cached_only")]
        refresh: bool,
        #[arg(long)]
        cached_only: bool,
    },
    /// Evaluate required checks separately from overall observed CI.
    RequiredChecks {
        repository: String,
        number: u64,
        #[arg(long, conflicts_with = "cached_only")]
        refresh: bool,
        #[arg(long)]
        cached_only: bool,
    },
    /// List all authors' PRs, including closed/merged PRs with --state all.
    Prs {
        repository: String,
        #[arg(long, default_value="open", value_parser=["open","closed","all"])]
        state: String,
        #[arg(long, conflicts_with = "cached_only")]
        refresh: bool,
        #[arg(long)]
        cached_only: bool,
    },
    Watches,
    Unwatch {
        id: String,
    },
    /// Read changes since an opaque cursor. Omit the cursor to replay history.
    Changes {
        #[arg(long, default_value_t = 100)]
        limit: usize,
        #[arg(long, default_value_t = 0)]
        wait: u64,
    },
    Status,
}

#[derive(ClapArgs, Default, Clone)]
struct PrArgs {
    /// Compatibility: hey-gh pr OWNER/REPO NUMBER.
    legacy_repository: Option<String>,
    legacy_number: Option<u64>,
    #[command(subcommand)]
    action: Option<PrAction>,
    #[arg(long, global = true, conflicts_with = "cached_only")]
    refresh: bool,
    #[arg(long, global = true)]
    cached_only: bool,
    /// Comma-separated gh-style fields; the cursor envelope is always retained.
    #[arg(long, global = true)]
    json: Option<String>,
    /// Long-poll cursor reads for at most 30 seconds.
    #[arg(long, global = true, default_value_t = 0)]
    wait: u64,
}

#[derive(Subcommand, Clone)]
enum PrAction {
    /// List your open PRs across repositories (or restrict with -R).
    List {
        #[arg(short = 'L', long)]
        limit: Option<usize>,
        #[arg(short = 's', long, default_value = "open", value_parser = ["open"])]
        state: String,
        #[arg(short = 'A', long, default_value = "@me", value_parser = ["@me"])]
        author: String,
    },
    /// Status of your authored open PRs.
    Status,
    /// View NUMBER, URL, or BRANCH; omit to use the current git branch.
    View { selector: Option<String> },
    /// Detailed CI for NUMBER, URL, or BRANCH.
    Checks { selector: Option<String> },
    /// Read only changed PRs; requires --cursor.
    Changes {
        #[arg(short = 'L', long, default_value_t = 1000)]
        limit: usize,
    },
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    let log_directory = match &args.command {
        Some(Command::Serve {
            listen, log_dir, ..
        }) => Some(
            log_dir
                .clone()
                .unwrap_or_else(|| logging::directory(listen.port())),
        ),
        _ => None,
    };
    if let Err(error) = logging::init(log_directory.as_deref()) {
        eprintln!("hey-gh: cannot initialize daemon logging: {error}");
        std::process::exit(1);
    }
    if let Err(error) = run(args).await {
        tracing::error!("hey-gh command failed; details reported on stderr");
        eprintln!("hey-gh: {error}");
        std::process::exit(1);
    }
}

async fn run(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    let deadline = args
        .timeout
        .map(|seconds| tokio::time::Instant::now() + Duration::from_secs(seconds));
    if args.timeout.is_some() {
        read_deadline::validate(args.command.as_ref(), args.cursor.as_deref())?;
    }
    if let Some(Command::Logs {
        tail,
        path,
        log_dir,
        summary,
        since,
    }) = &args.command
    {
        let port = url::Url::parse(&args.server)?
            .port_or_known_default()
            .ok_or("server port missing")?;
        let directory = log_dir.clone().unwrap_or_else(|| logging::directory(port));
        if *path {
            println!("{}", directory.display());
        } else if *summary {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_millis() as u64;
            println!(
                "{}",
                serde_json::to_string_pretty(&log_summary::read(
                    &directory,
                    since.unwrap_or(900),
                    now
                )?)?
            );
        } else {
            logging::tail(&directory, *tail)?;
        }
        return Ok(());
    }
    if let Some(Command::Install { skills_dirs }) = &args.command {
        let installed = skill_install::install(skills_dirs.clone())?;
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "skill": "hey-gh",
                "installed": installed,
            }))?
        );
        return Ok(());
    }
    if let Some(Command::Serve {
        listen,
        cache,
        log_dir,
        hostname,
        queue_capacity,
    }) = args.command
    {
        if !listen.ip().is_loopback() {
            return Err("API server must bind to a loopback address".into());
        }
        tracing::info!(version=env!("CARGO_PKG_VERSION"),%listen,%hostname,log_directory=%log_dir.unwrap_or_else(|| logging::directory(listen.port())).display(),"daemon starting");
        let mut config = Config {
            hostname: hostname.clone(),
            queue_capacity,
            ..Config::default()
        };
        if hostname != "github.com" {
            config.rest_url = url::Url::parse(&format!("https://{hostname}/api/v3/"))?;
            config.graphql_url = url::Url::parse(&format!("https://{hostname}/api/graphql"))?;
        }
        if let Some(path) = cache {
            config.cache_path = path;
        }
        if let Some(parent) = config
            .cache_path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent)?;
        }
        let lock_path = config.cache_path.with_extension("daemon.lock");
        let mut options = std::fs::OpenOptions::new();
        options.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let lock = options.open(lock_path)?;
        lock.try_lock()
            .map_err(|_| "another hey-gh daemon already owns this cache database")?;
        let client = Client::from_gh(config).await?;
        let api = hey_gh::api::Api::new(client).await?;
        let listener = tokio::net::TcpListener::bind(listen).await?;
        let (api_token, _registration) = hey_gh::local_auth::register(listener.local_addr()?)?;
        tracing::info!(address=%listener.local_addr()?,"daemon listening");
        eprintln!(
            "hey-gh API listening on http://{} (using gh login for {hostname})",
            listener.local_addr()?
        );
        let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);
        let server = axum::serve(listener, api.router_with_auth(Some(api_token)))
            .with_graceful_shutdown(async move {
                shutdown_signal().await;
                tracing::info!("daemon shutdown requested");
                let _ = shutdown_tx.send(true);
            });
        tokio::select! {
            result = server => result?,
            _ = async {
                let _ = shutdown_rx.changed().await;
                tokio::time::sleep(Duration::from_secs(5)).await;
            } => {
                tracing::warn!("graceful shutdown budget elapsed; canceling remaining HTTP reads");
            }
        }
        api.stop().await;
        tracing::info!("daemon stopped");
        drop(lock);
        return Ok(());
    }
    let api = ApiClient::new(url::Url::parse(&args.server)?)?;
    if let Some(deadline) = deadline {
        let (value, complete) = read_deadline::read(
            &api,
            args.command.expect("validated read command"),
            args.repo,
            deadline,
        )
        .await?;
        println!("{}", serde_json::to_string_pretty(&value)?);
        if !complete {
            return Err("incomplete report; source errors are included in the JSON output".into());
        }
        return Ok(());
    }
    let mut complete = true;
    let value: Value = match args.command.unwrap_or(Command::Pr(PrArgs::default())) {
        Command::Pr(options) => {
            let (value, is_complete) = run_pr(&api, options, args.repo, args.cursor).await?;
            complete = is_complete;
            value
        }
        Command::Ci {
            repository,
            number,
            refresh,
            cached_only,
        } => {
            match api
                .ci_for_pr(&repository, number, policy(refresh, cached_only))
                .await
            {
                Ok(report) => {
                    complete = report.complete;
                    serde_json::to_value(report)?
                }
                Err(hey_gh::Error::CacheMiss) if cached_only => {
                    complete = false;
                    unavailable_cached_pr(&repository, number)
                }
                Err(error) => return Err(error.into()),
            }
        }
        Command::Mine {
            repository,
            refresh,
        } => serde_json::to_value(
            api.my_pull_requests(&repository, policy(refresh, false))
                .await?,
        )?,
        Command::Watch {
            repository,
            number,
            interval,
        } => serde_json::to_value(api.watch(&repository, number, interval).await?)?,
        Command::WatchRepo {
            repository,
            branches,
            all_branches,
            interval,
        } => serde_json::to_value(
            api.watch_repository(&repository, branches, all_branches, interval)
                .await?,
        )?,
        Command::Repo {
            repository,
            branches,
            all_branches,
            refresh,
            cached_only,
        } => {
            let report = api
                .repository_report(
                    &repository,
                    &branches,
                    all_branches,
                    policy(refresh, cached_only),
                )
                .await?;
            complete = report.errors.is_empty();
            serde_json::to_value(report)?
        }
        Command::RequiredChecks {
            repository,
            number,
            refresh,
            cached_only,
        } => {
            let report = api
                .required_checks_for_pr(&repository, number, policy(refresh, cached_only))
                .await?;
            complete = report.errors.is_empty();
            serde_json::to_value(report)?
        }
        Command::Prs {
            repository,
            state,
            refresh,
            cached_only,
        } => serde_json::to_value(
            api.list_pull_requests(&repository, &state, policy(refresh, cached_only))
                .await?,
        )?,
        Command::Watches => serde_json::to_value(api.watches().await?)?,
        Command::Unwatch { id } => {
            api.unwatch(&id).await?;
            return Ok(());
        }
        Command::Changes { limit, wait } => serde_json::to_value(
            api.changes(args.cursor.as_deref(), limit, Duration::from_secs(wait))
                .await?,
        )?,
        Command::Snapshot => serde_json::to_value(api.bootstrap().await?)?,
        Command::Status => serde_json::to_value(api.status().await?)?,
        Command::Serve { .. } | Command::Install { .. } | Command::Logs { .. } => unreachable!(),
    };
    println!("{}", serde_json::to_string_pretty(&value)?);
    if !complete {
        return Err("incomplete report; source errors are included in the JSON output".into());
    }
    Ok(())
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut terminate) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {},
                    _ = terminate.recv() => {},
                }
            }
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
fn policy(refresh: bool, cached_only: bool) -> Freshness {
    if cached_only {
        Freshness::CachedOnly
    } else if refresh {
        Freshness::Revalidate
    } else {
        Freshness::default()
    }
}

async fn run_pr(
    api: &ApiClient,
    options: PrArgs,
    repo: Option<String>,
    cursor: Option<String>,
) -> Result<(Value, bool), Box<dyn std::error::Error>> {
    let fields = options
        .json
        .as_deref()
        .map(|s| s.split(',').collect::<Vec<_>>());
    if let Some(fields) = &fields {
        for field in fields {
            if !hey_gh::PR_STATUS_FIELDS.contains(field) {
                return Err(format!(
                    "unknown PR JSON field: {field}; supported fields: {}",
                    hey_gh::PR_STATUS_FIELDS.join(",")
                )
                .into());
            }
        }
    }
    if options.wait > 30 {
        return Err("--wait must be at most 30 seconds".into());
    }
    let freshness = policy(options.refresh, options.cached_only);
    if let Some(repository) = options.legacy_repository {
        if options.action.is_some() || repo.is_some() || cursor.is_some() || fields.is_some() {
            return Err(
                "legacy PR syntax cannot be combined with subcommands, -R, --cursor, or --json"
                    .into(),
            );
        }
        let number = options
            .legacy_number
            .ok_or("expected NUMBER after OWNER/REPO; use -R to filter the dashboard")?;
        let report = match api.pr_report(&repository, number, freshness).await {
            Ok(report) => report,
            Err(hey_gh::Error::CacheMiss) if options.cached_only => {
                return Ok((unavailable_cached_pr(&repository, number), false));
            }
            Err(error) => return Err(error.into()),
        };
        return Ok((serde_json::to_value(&report)?, report.complete));
    }
    let mut limit = 1000;
    let mut initial_limit = None;
    let checks = matches!(options.action, Some(PrAction::Checks { .. }));
    match options.action {
        Some(PrAction::View { selector }) | Some(PrAction::Checks { selector }) => {
            if cursor.is_some() || options.wait != 0 {
                return Err(
                    "--cursor/--wait apply to PR lists and changes, not a single PR".into(),
                );
            }
            let (repository, number) = resolve_pr(api, repo, selector).await?;
            if checks {
                if fields.is_some() {
                    return Err("pr checks returns the complete CI report; use pr view --json ci,statusCheckRollup for field selection".into());
                }
                let report = match api.ci_for_pr(&repository, number, freshness).await {
                    Ok(report) => report,
                    Err(hey_gh::Error::CacheMiss) if options.cached_only => {
                        return Ok((unavailable_cached_pr(&repository, number), false));
                    }
                    Err(error) => return Err(error.into()),
                };
                return Ok((serde_json::to_value(&report)?, report.complete));
            }
            let report = match api.pr_report(&repository, number, freshness).await {
                Ok(report) => report,
                Err(hey_gh::Error::CacheMiss) if options.cached_only => {
                    return Ok((unavailable_cached_pr(&repository, number), false));
                }
                Err(error) => return Err(error.into()),
            };
            let mut value = view_json(&report);
            if let Some(fields) = fields {
                let mut projected: serde_json::Map<_, _> = fields
                    .into_iter()
                    .map(|f| (f.to_owned(), value[f].clone()))
                    .collect();
                projected.insert("complete".into(), serde_json::json!(report.complete));
                projected.insert("sourceErrors".into(), value["sourceErrors"].take());
                for field in ["observedAtMs", "oldestValidationAtMs", "validations"] {
                    projected.insert(field.into(), value[field].take());
                }
                value = Value::Object(projected);
            }
            return Ok((value, report.complete));
        }
        Some(PrAction::List {
            limit: Some(requested),
            ..
        }) => {
            if !(1..=1000).contains(&requested) {
                return Err("--limit must be 1..1000".into());
            }
            limit = requested;
            initial_limit = Some(requested);
        }
        Some(PrAction::Changes { limit: requested }) => {
            if cursor.is_none() {
                return Err("pr changes requires --cursor from an initial hey-gh pr read".into());
            }
            limit = requested;
        }
        _ => {}
    }
    let mut page = api
        .pr_status_selected(
            hey_gh::PrStatusSelection {
                repository: repo.as_deref(),
                cursor: cursor.as_deref(),
                fields: fields.as_deref(),
            },
            limit,
            Duration::from_secs(options.wait),
            freshness,
        )
        .await?;
    // A fast read may be hydrating. Actual source errors still fail visibly.
    let complete = page.errors.is_empty()
        && page
            .pull_requests
            .iter()
            .chain(page.changes.iter().map(|c| &c.pull_request))
            .all(|p| p["sourceErrors"].as_object().is_some_and(|e| e.is_empty()));
    let total = page.pull_requests.len();
    let truncated = initial_limit.is_some_and(|l| total > l);
    if let Some(limit) = initial_limit {
        page.pull_requests.truncate(limit);
    }
    if let Some(fields) = fields {
        for row in page
            .pull_requests
            .iter_mut()
            .chain(page.changes.iter_mut().map(|c| &mut c.pull_request))
        {
            let projected: serde_json::Map<_, _> = fields
                .iter()
                .map(|f| ((*f).to_owned(), row[*f].clone()))
                .collect();
            *row = Value::Object(projected);
        }
    }
    let mut value = serde_json::to_value(page)?;
    if cursor.is_none() {
        value["totalCount"] = serde_json::json!(total);
        value["truncated"] = serde_json::json!(truncated);
    }
    Ok((value, complete))
}

fn unavailable_cached_pr(repository: &str, number: u64) -> Value {
    serde_json::json!({
        "number": number, "repository": {"nameWithOwner": repository},
        "complete": false, "available": false, "code": "cache_miss",
        "sourceErrors": {"sources": [{"source": "metadata", "message": "no cached response available"}]},
        "oldestValidationAtMs": null, "validations": [], "cursor": null
    })
}

fn view_json(report: &hey_gh::Report) -> Value {
    let data = &report.data;
    let pr = &data.pull_request;
    let mut value = serde_json::json!({
        "number":data.number,"repository":{"nameWithOwner":data.repository},
        "author":{"login":pr["user"]["login"]},
        "state":if pr["merged"]==true || !pr["merged_at"].is_null() {"MERGED"} else if pr["state"]=="closed" {"CLOSED"} else {"OPEN"},
        "headRefName":pr["head"]["ref"],"headRefOid":pr["head"]["sha"],"baseRefName":pr["base"]["ref"],"baseRefOid":pr["base"]["sha"],
        "conflicts":data.conflicts,"mergeable":match data.conflicts.as_str() {"clean"=>"MERGEABLE","conflicting"=>"CONFLICTING",_=>"UNKNOWN"},
        "ci":data.ci,"comments":data.comments,"reviewComments":data.review_comments,"reviews":data.reviews,"reviewThreads":data.review_threads,"reviewStatus":data.review_status,
        "complete":report.complete,"sourceErrors":{"sources":data.errors,"ci":data.ci.errors},
        "observedAtMs":report.observed_at_ms,"oldestValidationAtMs":report.oldest_validation_at_ms,"validations":report.validations,
        "reviewDecision":null,"mergeStateStatus":null,"requiredChecks":null,
        "raw":data.pull_request
    });
    for (out, input) in [
        ("id", "node_id"),
        ("title", "title"),
        ("url", "html_url"),
        ("body", "body"),
        ("isDraft", "draft"),
        ("createdAt", "created_at"),
        ("updatedAt", "updated_at"),
        ("mergedAt", "merged_at"),
        ("closedAt", "closed_at"),
        ("additions", "additions"),
        ("deletions", "deletions"),
        ("labels", "labels"),
        ("assignees", "assignees"),
    ] {
        value[out] = pr[input].clone();
    }
    let mut checks = vec![];
    for c in &data.ci.check_runs {
        checks.push(serde_json::json!({"__typename":"CheckRun","name":c["name"],"status":c["status"].as_str().map(str::to_ascii_uppercase),"conclusion":c["conclusion"].as_str().map(str::to_ascii_uppercase),"detailsUrl":c["details_url"]}));
    }
    for c in &data.ci.commit_statuses {
        checks.push(serde_json::json!({"__typename":"StatusContext","context":c["context"],"state":c["state"].as_str().map(str::to_ascii_uppercase),"targetUrl":c["target_url"]}));
    }
    value["statusCheckRollup"] = serde_json::json!(checks);
    value
}

async fn local_repository(repo: Option<String>) -> Result<String, Box<dyn std::error::Error>> {
    if let Some(repo) = repo.or_else(|| std::env::var("GH_REPO").ok()) {
        return Ok(repo);
    }
    let output = tokio::process::Command::new("git")
        .args(["remote", "get-url", "origin"])
        .output()
        .await?;
    if !output.status.success() {
        return Err(
            "specify -R OWNER/REPO, a PR URL, or run inside a repository with an origin remote"
                .into(),
        );
    }
    let remote = String::from_utf8(output.stdout)?;
    let remote = remote.trim().trim_end_matches(".git");
    let path = if let Ok(url) = url::Url::parse(remote) {
        url.path().trim_start_matches('/').to_owned()
    } else {
        remote
            .split_once(':')
            .map(|(_, p)| p.to_owned())
            .ok_or("cannot infer repository from origin; specify -R OWNER/REPO")?
    };
    Ok(path)
}

async fn resolve_pr(
    api: &ApiClient,
    repo: Option<String>,
    selector: Option<String>,
) -> Result<(String, u64), Box<dyn std::error::Error>> {
    if let Some(selector) = &selector
        && let Ok(url) = url::Url::parse(selector)
    {
        if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some() {
            return Err("invalid PR URL".into());
        }
        let parts: Vec<_> = url.path().trim_matches('/').split('/').collect();
        if parts.len() != 4 || parts[2] != "pull" {
            return Err("expected a GitHub PR URL ending in /OWNER/REPO/pull/NUMBER".into());
        }
        let repository = format!("{}/{}", parts[0], parts[1]);
        if repo
            .as_ref()
            .is_some_and(|r| !r.eq_ignore_ascii_case(&repository))
        {
            return Err("PR URL and -R select different repositories".into());
        }
        return Ok((repository, parts[3].parse()?));
    }
    let repository = local_repository(repo).await?;
    let selector = if let Some(selector) = selector {
        selector
    } else {
        let output = tokio::process::Command::new("git")
            .args(["branch", "--show-current"])
            .output()
            .await?;
        let branch = String::from_utf8(output.stdout)?.trim().to_owned();
        if !output.status.success() || branch.is_empty() {
            return Err("specify a PR number or branch".into());
        }
        branch
    };
    if let Ok(number) = selector.parse::<u64>() {
        return Ok((repository, number));
    }
    let pulls = api
        .list_pull_requests(&repository, "open", Freshness::default())
        .await?;
    let matches: Vec<_> = pulls
        .iter()
        .filter(|p| p["head"]["ref"] == selector)
        .collect();
    if matches.len() != 1 {
        return Err("branch does not uniquely identify an open PR; specify NUMBER".into());
    }
    Ok((
        repository,
        matches[0]["number"].as_u64().ok_or("PR has no number")?,
    ))
}
