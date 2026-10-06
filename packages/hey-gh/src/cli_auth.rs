//! CLI authentication policy, independent of command execution and cached reads.
use crate::cli_route::{Auth, Invocation, flag_enabled, option_values, positional_indices};
use serde_json::Value;
use std::{
    ffi::OsString,
    process::{Command, Stdio},
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

pub fn command(invocation: &Invocation) -> Result<Command> {
    let original_args = &invocation.args;
    let resolved = crate::cli_alias::resolve(invocation)?;
    let invocation = resolved.as_ref().unwrap_or(invocation);
    let options = option_values(&invocation.args);
    let comment = is_comment(invocation, &options)?;
    let app = !invocation.help()
        && match invocation.auth {
            Auth::Auto => comment,
            Auth::User => false,
            Auth::App => true,
        };
    let mut command = Command::new("gh");
    command.args(original_args);
    if !app {
        return Ok(command);
    }
    if comment && flag_enabled(&options, &["--web", "-w"]) {
        return Err("GitHub App comments cannot use a browser's personal login; omit --web or explicitly select --auth user".into());
    }
    let host = hostname(invocation, &options)?;
    let token = match std::env::var("HEY_GH_APP_TOKEN") {
        Ok(token) if hey_gh::app_auth::valid_cli_token(&token) => token,
        Ok(_) | Err(std::env::VarError::NotUnicode(_)) => {
            return Err(
                "HEY_GH_APP_TOKEN must contain a valid GitHub App installation token".into(),
            );
        }
        Err(std::env::VarError::NotPresent) => {
            let installation = hey_gh::app_auth::load(&host)?
                .ok_or("GitHub App is not configured for this host; configure it with hey-gh app (personal authentication is never used as a fallback)")?;
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?
                .block_on(installation.cli_token(&host))?
        }
    };
    set_token(&mut command, &host, &token);
    Ok(command)
}

fn set_token(command: &mut Command, host: &str, token: &str) {
    command
        .env_remove("GH_TOKEN")
        .env_remove("GITHUB_TOKEN")
        .env_remove("GH_ENTERPRISE_TOKEN")
        .env_remove("GITHUB_ENTERPRISE_TOKEN")
        .env_remove("HEY_GH_APP_PRIVATE_KEY")
        .env_remove("HEY_GH_APP_TOKEN");
    command.env(
        if host == "github.com" || host.ends_with(".ghe.com") {
            "GH_TOKEN"
        } else {
            "GH_ENTERPRISE_TOKEN"
        },
        token,
    );
    command.env("GH_HOST", host);
}

fn is_comment(invocation: &Invocation, options: &[(String, Option<OsString>)]) -> Result<bool> {
    if invocation.auth != Auth::Auto || invocation.help() {
        return Ok(
            matches!(invocation.root(), "pr" | "issue") && invocation.subcommand() == "comment"
        );
    }
    if matches!(invocation.root(), "pr" | "issue") && invocation.subcommand() == "comment" {
        return Ok(true);
    }
    if invocation.root() == "pr" && invocation.subcommand() == "review" {
        return Ok(!flag_enabled(
            options,
            &["--approve", "-a", "--request-changes", "-r"],
        ));
    }
    if invocation.root() != "api" {
        return Ok(false);
    }
    let endpoint = api_endpoint(invocation);
    let parsed = url::Url::parse(&endpoint).ok();
    let path = parsed
        .as_ref()
        .map(|url| url.path())
        .unwrap_or_else(|| endpoint.split('?').next().unwrap_or(&endpoint));
    let path = path.strip_prefix("/api/v3").unwrap_or(path);
    if matches!(path.trim_matches('/'), "graphql" | "api/graphql") {
        let query_string = parsed
            .as_ref()
            .and_then(|url| url.query())
            .or_else(|| endpoint.split_once('?').map(|(_, query)| query));
        let mut queries: Vec<String> = query_string
            .into_iter()
            .flat_map(|query| url::form_urlencoded::parse(query.as_bytes()))
            .filter(|(key, _)| key == "query")
            .map(|(_, value)| value.into_owned())
            .collect();
        for (name, value) in options {
            if matches!(name.as_str(), "-f" | "-F" | "--field" | "--raw-field")
                && let Some(value) = value
                    .as_ref()
                    .and_then(|v| v.to_str())
                    .and_then(|v| v.strip_prefix("query="))
            {
                if matches!(name.as_str(), "-F" | "--field") && value.starts_with('@') {
                    if value == "@-" {
                        return Err(
                            "GraphQL query from stdin requires explicit --auth app or --auth user"
                                .into(),
                        );
                    }
                    queries.push(std::fs::read_to_string(&value[1..])?);
                } else {
                    queries.push(value.to_owned());
                }
            }
            if name == "--input" {
                let path = value.as_ref().ok_or("--input requires a path")?;
                if path == "-" {
                    return Err(
                        "GraphQL input from stdin requires explicit --auth app or --auth user"
                            .into(),
                    );
                }
                let input: Value = serde_json::from_slice(&std::fs::read(path)?)?;
                if let Some(query) = input.get("query").and_then(Value::as_str) {
                    queries.push(query.to_owned());
                }
            }
        }
        return Ok(queries.iter().any(|query| graphql_comments(query)));
    }
    let method = options
        .iter()
        .rev()
        .find(|(n, _)| matches!(n.as_str(), "-X" | "--method"))
        .and_then(|(_, v)| v.as_ref())
        .and_then(|v| v.to_str())
        .unwrap_or_else(|| {
            if options.iter().any(|(n, _)| {
                matches!(
                    n.as_str(),
                    "-f" | "-F" | "--field" | "--raw-field" | "--input"
                )
            }) {
                "POST"
            } else {
                "GET"
            }
        });
    if matches!(
        method.to_ascii_uppercase().as_str(),
        "GET" | "HEAD" | "OPTIONS"
    ) {
        return Ok(false);
    }
    let parts: Vec<_> = path.trim_matches('/').split('/').collect();
    Ok((parts.first() == Some(&"repos")
        && parts.len() >= 5
        && matches!(parts[3], "issues" | "pulls" | "commits")
        && parts[4..]
            .iter()
            .any(|part| matches!(*part, "comments" | "reviews")))
        || (parts.first() == Some(&"gists") && parts.get(2) == Some(&"comments")))
}

fn graphql_comments(query: &str) -> bool {
    use graphql_parser::query::{Definition, OperationDefinition, Selection, SelectionSet};
    fn contains(set: &SelectionSet<'_, String>) -> bool {
        set.items.iter().any(|selection| match selection {
            Selection::Field(field) => {
                matches!(
                    field.name.as_str(),
                    "addComment"
                        | "updateIssueComment"
                        | "deleteIssueComment"
                        | "addPullRequestReview"
                        | "addPullRequestReviewComment"
                        | "addPullRequestReviewThread"
                        | "addPullRequestReviewThreadReply"
                        | "updatePullRequestReview"
                        | "updatePullRequestReviewComment"
                        | "deletePullRequestReviewComment"
                        | "submitPullRequestReview"
                        | "addDiscussionComment"
                        | "updateDiscussionComment"
                        | "deleteDiscussionComment"
                ) || contains(&field.selection_set)
            }
            Selection::InlineFragment(fragment) => contains(&fragment.selection_set),
            Selection::FragmentSpread(_) => false,
        })
    }
    graphql_parser::parse_query::<String>(query).is_ok_and(|document| {
        document
            .definitions
            .iter()
            .any(|definition| match definition {
                Definition::Operation(operation) => match operation {
                    OperationDefinition::Mutation(mutation) => contains(&mutation.selection_set),
                    _ => false,
                },
                Definition::Fragment(fragment) => contains(&fragment.selection_set),
            })
    })
}

fn api_endpoint(invocation: &Invocation) -> String {
    positional_indices(&invocation.args)
        .get(1)
        .map(|i| invocation.args[*i].to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn hostname(invocation: &Invocation, options: &[(String, Option<OsString>)]) -> Result<String> {
    if invocation.root() == "api" && api_endpoint(invocation).starts_with("https://") {
        let host = url_host(&api_endpoint(invocation))?;
        return Ok(
            if host == "api.github.com" || (host.starts_with("api.") && host.ends_with(".ghe.com"))
            {
                host[4..].to_owned()
            } else {
                host
            },
        );
    }
    // Comment selectors override -R in gh, including their hostname. Skip
    // option values so a URL in a comment body cannot change authentication.
    if matches!(invocation.root(), "pr" | "issue") {
        for index in positional_indices(&invocation.args).into_iter().skip(2) {
            let value = invocation.args[index].to_string_lossy();
            if value.starts_with("https://") {
                return url_host(&value);
            }
        }
    }
    if let Some(host) = options
        .iter()
        .rev()
        .find(|(n, _)| n == "--hostname")
        .and_then(|(_, v)| v.as_ref())
        .and_then(|v| v.to_str())
    {
        return Ok(host.to_ascii_lowercase());
    }
    let repository = options
        .iter()
        .rev()
        .find(|(n, _)| matches!(n.as_str(), "-R" | "--repo"))
        .and_then(|(_, v)| v.as_ref())
        .cloned()
        .or_else(|| std::env::var_os("GH_REPO"));
    if let Some(repository) = repository {
        let repository = repository.to_str().ok_or("invalid repository")?;
        if repository.starts_with("https://") {
            return url_host(repository);
        }
        if repository.split('/').count() == 3 {
            return Ok(repository.split('/').next().unwrap().to_ascii_lowercase());
        }
        return Ok(std::env::var("GH_HOST")
            .unwrap_or_else(|_| "github.com".into())
            .to_ascii_lowercase());
    }
    if let Ok(host) = std::env::var("GH_HOST") {
        return Ok(host.to_ascii_lowercase());
    }
    if matches!(invocation.root(), "pr" | "issue") {
        let output = Command::new("gh")
            .args(["repo", "view", "--json", "url"])
            .stdin(Stdio::null())
            .output()?;
        if !output.status.success() {
            return Err(
                "cannot resolve the GitHub App target host; provide --repo [HOST/]OWNER/REPO"
                    .into(),
            );
        }
        let value: Value = serde_json::from_slice(&output.stdout)
            .map_err(|_| "cannot resolve GitHub App target host")?;
        return url_host(
            value["url"]
                .as_str()
                .ok_or("cannot resolve GitHub App target host")?,
        );
    }
    Ok("github.com".into())
}

fn url_host(value: &str) -> Result<String> {
    Ok(url::Url::parse(value)?
        .host_str()
        .ok_or("invalid GitHub URL")?
        .to_ascii_lowercase())
}
