//! Exercise the installed review wrapper with native gh, without contacting GitHub.
//! cargo run --example code-reviews-gh -- /path/to/hey-boss /path/to/hey-gh /path/to/native/gh
use serde_json::{Value, json};
use std::{
    env, fs,
    io::Write,
    os::unix::fs::symlink,
    path::Path,
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    thread,
};
use tiny_http::{Header, Method, Response, Server};

const SHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const REPO: &str = "example/time";

#[derive(Default)]
struct Traffic {
    writes: Vec<(String, String)>,
    reads: Vec<(String, String)>,
}

fn serve(server: &Server, traffic: &Mutex<Traffic>) {
    for mut req in server.incoming_requests() {
        let path = req.url().split('?').next().unwrap().to_owned();
        let auth = req
            .headers()
            .iter()
            .find(|h| h.field.equiv("Authorization"))
            .map(|h| h.value.as_str())
            .unwrap_or("")
            .to_owned();
        let mut body = Vec::new();
        req.as_reader().read_to_end(&mut body).unwrap();
        let value: Value = if body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&body).unwrap()
        };
        let query = value["query"].as_str().unwrap_or("");
        let mutation = *req.method() != Method::Get
            && (!path.ends_with("/graphql")
                || query
                    .split(|c: char| !(c.is_alphanumeric() || c == '_'))
                    .any(|w| w == "mutation"));
        if mutation {
            traffic.lock().unwrap().writes.push((path.clone(), auth));
        } else if path == "/graphql" {
            traffic.lock().unwrap().reads.push((query.to_owned(), auth));
        }
        let response = match path.as_str() {
            "/graphql" => {
                let mut pr = json!({"id":"PR_fixture","number":7,"title":"Timezone fix","headRefOid":SHA,"reviews":{"nodes":[],"pageInfo":{"hasNextPage":false}}});
                let repo = json!({"id":"R_fixture","name":"time","owner":{"login":"example"},"pullRequest":pr});
                pr["repository"] = json!({"nameWithOwner":REPO});
                json!({"data":{"repository":repo,"node":pr}})
            }
            "/user" => json!({"login":"fixture-human"}),
            p if p.ends_with("/files") => {
                json!([{"filename":"time.rs","patch":"@@ -8,2 +8,3 @@\n before\n+midnight();\n after"}])
            }
            p if p.ends_with("/reviews") || p.ends_with("/comments") => {
                if mutation {
                    json!({"id":903})
                } else {
                    json!([])
                }
            }
            p if p.ends_with("/pulls/7") => {
                json!({"state":"closed","merged":true,"title":"Timezone fix","head":{"sha":SHA}})
            }
            _ => json!({}),
        };
        req.respond(
            Response::from_string(response.to_string())
                .with_header(Header::from_bytes("Content-Type", "application/json").unwrap()),
        )
        .unwrap();
    }
}

fn main() {
    let [boss, hey, native]: [_; 3] = env::args_os()
        .skip(1)
        .map(|p| fs::canonicalize(p).unwrap())
        .collect::<Vec<_>>()
        .try_into()
        .expect("expected hey-boss, hey-gh and native gh paths");
    let dir = tempfile::tempdir_in(".").unwrap();
    let root = dir.path().canonicalize().unwrap();
    let db = root.join("issue.db");
    let _owner = hey_boss::database::Owner::start(&db)
        .unwrap()
        .expect("private fixture database");
    let bin = root.join(".local/bin");
    let upstream = root.join("native-bin");
    let config = root.join("config");
    for path in [&bin, &upstream, &config] {
        fs::create_dir_all(path).unwrap();
    }
    symlink(&native, upstream.join("gh")).unwrap();
    symlink(&boss, bin.join("hey-boss")).unwrap();
    symlink(&hey, bin.join("hey-gh")).unwrap();
    let gh = bin.join("gh");
    let socket = root.join("hub.sock");
    fs::write(
        config.join("config.yml"),
        format!(
            "http_unix_socket: {}\nprompt: disabled\naliases:\n  review-it: pr review\n",
            socket.display()
        ),
    )
    .unwrap();
    let server = Arc::new(Server::http_unix(&socket).unwrap());
    let traffic = Arc::new(Mutex::new(Traffic::default()));
    let listener = {
        let s = server.clone();
        let t = traffic.clone();
        thread::spawn(move || serve(&s, &t))
    };
    let run = |program: &Path, args: &[&str], input: &str, host: &str| {
        let mut child = Command::new(program)
            .args(args)
            .current_dir(&root)
            .env_clear()
            .env("HOME", &root)
            .env(
                "PATH",
                env::join_paths([
                    bin.as_path(),
                    upstream.as_path(),
                    Path::new("/usr/bin"),
                    Path::new("/bin"),
                ])
                .unwrap(),
            )
            .env("TMPDIR", &root)
            .env("GH_CONFIG_DIR", &config)
            .env("GH_HOST", host)
            .env("GH_TOKEN", "fixture-personal")
            .env("GH_ENTERPRISE_TOKEN", "fixture-enterprise")
            .env("HEY_GH_APP_TOKEN", "fixture-app")
            .env("GH_NO_UPDATE_NOTIFIER", "1")
            .env("GH_NO_EXTENSION_UPDATE_NOTIFIER", "1")
            .env("HEY_BOSS_AGENT_ID", "human:test")
            .env("HEY_BOSS_ISSUE_DB", &db)
            .env("GIT_CEILING_DIRECTORIES", root.parent().unwrap())
            .env("HEY_BOSS_FLEET_STATE", &root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    };
    let ok = |program: &Path, args: &[&str], input: &str| {
        let out = run(program, args, input, "github.com");
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        out.stdout
    };
    let api = |args: &[&str], input: &str| -> Value {
        serde_json::from_slice(&ok(&gh, args, input)).unwrap()
    };
    let installer =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("packages/hey-gh/install-review-command.sh");
    ok(&installer, &[], "");
    ok(&installer, &[], "");
    let project = "github.com/example/time";
    ok(
        &boss,
        &[
            "project",
            "init",
            "--project",
            project,
            "--prs",
            "true",
            "--worktree",
            "false",
            "--yes",
            "--json",
        ],
        "",
    );
    ok(
        &boss,
        &[
            "issue",
            "--project",
            project,
            "settings",
            "set",
            "--code-reviews-enabled",
            "--json",
        ],
        "",
    );
    for (host, args) in [
        ("github.com", &["--version"][..]),
        ("github.com", &["pr", "review", "--help"]),
        ("github.com", &["api", "--help"]),
        ("github.com", &["--auth", "nonsense"]),
        ("github.com", &["pr", "review", "--bogus"]),
        (
            "github.com",
            &[
                "pr",
                "review",
                "7",
                "-R",
                REPO,
                "--approve",
                "--request-changes",
            ],
        ),
        (
            "github.com",
            &[
                "pr",
                "review",
                "7",
                "-R",
                REPO,
                "--body-file",
                "missing-file",
            ],
        ),
        (
            "git.example.test",
            &["api", "graphql", "-f", "query={viewer{login}}"],
        ),
    ] {
        assert_eq!(
            run(&gh, args, "", host),
            run(&native, args, "", host),
            "native parity: {args:?}"
        );
    }
    let comments = "repos/example/time/pulls/7/comments";
    let inline = api(
        &["api", comments, "--input", "-"],
        &json!({"path":"time.rs","line":9,"side":"RIGHT","commit_id":SHA,"body":"Test midnight."})
            .to_string(),
    );
    assert_eq!(inline["line"], 9);
    assert!(inline["diff_hunk"].as_str().unwrap().contains("midnight"));
    ok(
        &gh,
        &[
            "review-it",
            "https://github.com/example/time/pull/7",
            "--request-changes",
            "--body-file",
            "-",
        ],
        "Please cover midnight.",
    );
    let reviews = api(&["api", "repos/example/time/pulls/7/reviews"], "");
    assert_eq!(reviews.as_array().unwrap().len(), 1);
    assert_eq!(reviews[0]["state"], "CHANGES_REQUESTED");
    let graphql = |query: &str, input: Value| {
        api(
            &["api", "graphql", "--input", "-"],
            &json!({"query":query,"variables":{"input":input}}).to_string(),
        )
    };
    let draft = graphql(
        "mutation($input:AddPullRequestReviewInput!){draft:addPullRequestReview(input:$input){pullRequestReview{id state}}}",
        json!({"pullRequestId":"PR_fixture","body":"Next pass"}),
    );
    let draft = &draft["data"]["draft"]["pullRequestReview"];
    assert_eq!(draft["state"], "PENDING");
    let thread = graphql(
        "mutation($input:AddPullRequestReviewThreadInput!){addPullRequestReviewThread(input:$input){thread{path line comments{totalCount}}}}",
        json!({"pullRequestReviewId":draft["id"],"path":"time.rs","line":9,"side":"RIGHT","body":"Looks good."}),
    );
    assert_eq!(
        thread["data"]["addPullRequestReviewThread"]["thread"]["line"],
        9
    );
    graphql(
        "mutation($input:SubmitPullRequestReviewInput!){submitPullRequestReview(input:$input){pullRequestReview{state}}}",
        json!({"pullRequestReviewId":draft["id"],"event":"APPROVE","body":"The boundary is covered."}),
    );
    let rejected = run(
        &gh,
        &[
            "api",
            comments,
            "-f",
            "path=time.rs",
            "-F",
            "line=300",
            "-f",
            &format!("commit_id={SHA}"),
            "-f",
            "body=Bad anchor",
        ],
        "",
        "github.com",
    );
    assert!(!rejected.status.success());
    let error = String::from_utf8_lossy(&rejected.stderr);
    assert!(
        error.contains("Code Reviews: Line is outside the PR diff") && error.contains("422"),
        "{error}"
    );
    let stale = run(
        &gh,
        &["api", "graphql", "--input", "-"],
        &json!({"query":"mutation($input:AddPullRequestReviewInput!){addPullRequestReview(input:$input){pullRequestReview{state}}}", "variables":{"input":{"pullRequestId":"PR_fixture","commitOID":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","event":"APPROVE"}}}).to_string(),
        "github.com",
    );
    assert!(!stale.status.success());
    let error = String::from_utf8_lossy(&stale.stderr);
    assert!(error.contains("Code Reviews: PR head changed"), "{error}");
    let view = api(
        &["pr", "view", "7", "-R", REPO, "--json", "number,reviews"],
        "",
    );
    assert!(
        view["reviews"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["state"] == "APPROVED")
    );
    assert!(
        traffic.lock().unwrap().writes.is_empty(),
        "review mutations must stay local"
    );
    ok(
        &hey,
        &[
            "api",
            "repos/elsewhere/other/issues/8/comments",
            "-f",
            "body=Normal comment",
        ],
        "",
    );
    ok(
        &gh,
        &[
            "api",
            "repos/elsewhere/other/pulls/1/reviews",
            "-f",
            "event=APPROVE",
        ],
        "",
    );
    ok(
        &hey,
        &[
            "api",
            "graphql",
            "-f",
            r#"query=mutation { addComment(input:{subjectId:"outside",body:"hello"}) {clientMutationId} }"#,
        ],
        "",
    );
    ok(
        &hey,
        &[
            "--auth",
            "app",
            "api",
            "graphql",
            "-f",
            "query={ viewer { login } }",
        ],
        "",
    );
    server.unblock();
    listener.join().unwrap();
    let traffic = traffic.lock().unwrap();
    assert_eq!(traffic.writes.len(), 3);
    for ((path, auth), (expected, token)) in traffic.writes.iter().zip([
        ("/repos/elsewhere/other/issues/8/comments", "fixture-app"),
        ("/repos/elsewhere/other/pulls/1/reviews", "fixture-personal"),
        ("/graphql", "fixture-app"),
    ]) {
        assert_eq!(path, expected);
        assert!(auth.ends_with(token), "{path}: unexpected identity");
    }
    let (query, auth) = traffic.reads.last().unwrap();
    assert!(auth.ends_with("fixture-app"));
    assert!(query.contains(r#"user(login: "fixture-human")"#));
    println!("Native gh review routing and identity checks passed.");
}
