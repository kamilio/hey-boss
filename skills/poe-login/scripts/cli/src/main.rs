use base64::{
    Engine,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use clap::{Args, Parser, Subcommand};
use regex::Regex;
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

type Result<T> = std::result::Result<T, &'static str>;
const RECORD: &str = "poe-login/gmail";
const MAX_BYTES: u64 = 1_048_576;

#[derive(Parser)]
#[command(
    about = "Read Gmail for Poe login using locally granted OAuth credentials. Mail output is private; do not record it."
)]
struct Cli {
    #[command(subcommand)]
    command: Action,
}
#[derive(Subcommand)]
enum Action {
    /// Read filtered mail. Credentials come from the distributed poe-login/gmail record.
    Emails(Emails),
    /// Fill an existing named agent-browser session without printing the code. Submit normally afterward.
    Enter {
        #[arg(long)]
        email: String,
        /// UTC timestamp recorded immediately before requesting this code (at most ten minutes ago).
        #[arg(long)]
        after: String,
        #[arg(long)]
        origin: String,
        #[arg(long)]
        session: String,
        #[arg(
            long,
            default_value = "input[autocomplete=\"one-time-code\"], input[placeholder=\"Code\"]"
        )]
        code_selector: String,
    },
    /// Refresh OAuth and verify Gmail access, returning only a boolean verdict.
    Check,
}
#[derive(Args)]
struct Emails {
    #[arg(long)]
    to: String,
    #[arg(long = "from", required = true)]
    from: Vec<String>,
    #[arg(long, default_value_t = 2, value_parser = clap::value_parser!(u16).range(1..=500))]
    max_emails: u16,
    #[arg(long, value_parser = clap::value_parser!(u16).range(1..))]
    days: Option<u16>,
    #[arg(long)]
    body: bool,
    /// Prefer text extracted from HTML when both MIME alternatives exist.
    #[arg(long)]
    html: bool,
    #[arg(long, conflicts_with = "markdown")]
    json: bool,
    #[arg(long)]
    markdown: bool,
}
#[derive(Clone, Deserialize, Serialize)]
struct Credentials {
    client_id: String,
    client_secret: String,
    refresh_token: String,
}
#[derive(Clone, Serialize)]
struct Email {
    subject: String,
    from: String,
    to: String,
    date: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    body: Option<String>,
    #[serde(skip)]
    timestamp: i64,
}
struct Intent {
    email: String,
    requested_at: i64,
    now: i64,
}
impl Intent {
    fn new(email: &str, after: &str, now: i64) -> Result<Self> {
        let requested_at = after
            .parse::<jiff::Timestamp>()
            .map_err(|_| "Invalid login timestamp")?
            .as_millisecond();
        if !Regex::new(r"^kjopek\+[a-zA-Z0-9][a-zA-Z0-9._-]{0,63}@quora\.com$")
            .unwrap()
            .is_match(email)
            || requested_at > now
            || now - requested_at > 600_000
        {
            return Err("Invalid QA login intent");
        }
        Ok(Self {
            email: email.to_lowercase(),
            requested_at,
            now,
        })
    }
}

fn private_command(program: &str, args: &[&str], input: Vec<u8>) -> Result<Vec<u8>> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "Private command unavailable")?;
    let mut stdin = child.stdin.take().ok_or("Private command unavailable")?;
    let stdout = child.stdout.take().ok_or("Private command unavailable")?;
    let writer = std::thread::spawn(move || stdin.write_all(&input));
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout
            .take(MAX_BYTES + 1)
            .read_to_end(&mut bytes)
            .map(|_| bytes)
    });
    let deadline = Instant::now() + Duration::from_secs(30);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    let written = writer.join().map_err(|_| "Private command failed")?;
    let bytes = reader
        .join()
        .map_err(|_| "Private command failed")?
        .map_err(|_| "Private command failed")?;
    if !status.is_some_and(|s| s.success()) || written.is_err() || bytes.len() > MAX_BYTES as usize
    {
        return Err("Private command failed; output withheld");
    }
    Ok(bytes)
}
fn rpc(request: Value) -> Result<Value> {
    let bytes = private_command(
        "hey-boss",
        &["secrets", "rpc"],
        serde_json::to_vec(&request).map_err(|_| "Secret request failed")?,
    )?;
    let response: Value = serde_json::from_slice(&bytes).map_err(|_| "Secret store unavailable")?;
    if response["ok"] != true {
        return Err(
            "Gmail credentials unavailable; check this machine's grant and unlocked local store",
        );
    }
    Ok(response)
}
fn json_response(response: reqwest::Result<reqwest::blocking::Response>) -> Result<Value> {
    let response = response.map_err(|_| "Provider request failed; output withheld")?;
    if !response.status().is_success() {
        return Err("Provider rejected request; output withheld");
    }
    let mut bytes = Vec::new();
    response
        .take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "Provider response failed")?;
    if bytes.len() > MAX_BYTES as usize {
        return Err("Provider response too large");
    }
    serde_json::from_slice(&bytes).map_err(|_| "Invalid provider response")
}
fn access_token(client: &Client) -> Result<String> {
    let response = rpc(json!({"op":"get", "name":RECORD}))?;
    let bytes = STANDARD
        .decode(
            response["data"]
                .as_str()
                .ok_or("Invalid credential record")?,
        )
        .map_err(|_| "Invalid credential record")?;
    let mut credentials: Credentials =
        serde_json::from_slice(&bytes).map_err(|_| "Invalid credential record")?;
    if [
        &credentials.client_id,
        &credentials.client_secret,
        &credentials.refresh_token,
    ]
    .iter()
    .any(|s| s.is_empty())
    {
        return Err("Incomplete credential record");
    }
    let token = json_response(
        client
            .post("https://oauth2.googleapis.com/token")
            .form(&[
                ("grant_type", "refresh_token"),
                ("client_id", &credentials.client_id),
                ("client_secret", &credentials.client_secret),
                ("refresh_token", &credentials.refresh_token),
            ])
            .send(),
    )?;
    if let Some(rotated) = token["refresh_token"]
        .as_str()
        .filter(|s| !s.is_empty() && *s != credentials.refresh_token)
    {
        credentials.refresh_token = rotated.into();
        rpc(
            json!({"op":"put", "name":response["record"]["id"], "data":STANDARD.encode(serde_json::to_vec(&credentials).map_err(|_| "Credential update failed")?), "kind":"bundle", "expected":response["record"]["revision"]}),
        )?;
    }
    token["access_token"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(String::from)
        .ok_or("OAuth did not return an access token")
}
fn valid_address(s: &str) -> bool {
    Regex::new(r"^[a-zA-Z0-9.!#$%&'*+/=?^_`{|}~-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}$")
        .unwrap()
        .is_match(s)
}
fn mail_query(options: &Emails) -> Result<String> {
    if !valid_address(&options.to)
        || options.from.is_empty()
        || options.from.iter().any(|s| !valid_address(s))
    {
        return Err("Invalid recipient or sender filter");
    }
    let mut query = format!(
        "to:{} ({})",
        options.to,
        options
            .from
            .iter()
            .map(|s| format!("from:{s}"))
            .collect::<Vec<_>>()
            .join(" OR ")
    );
    if let Some(days) = options.days {
        query.push_str(&format!(" newer_than:{days}d"));
    }
    Ok(query)
}
fn html_text(html: &str) -> String {
    let stripped = Regex::new(r"(?is)<(script|style)\b[^>]*>.*?</(?:script|style)\s*>")
        .unwrap()
        .replace_all(html, " ");
    let text = Regex::new(r"(?s)<!--.*?-->|<[^>]*>")
        .unwrap()
        .replace_all(&stripped, " ");
    html_escape::decode_html_entities(&text)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
fn extract_body(payload: &Value, prefer_html: bool) -> Result<String> {
    fn visit(part: &Value, plain: &mut Vec<String>, html: &mut Vec<String>) -> Result<()> {
        if let Some(data) = part["body"]["data"]
            .as_str()
            .filter(|_| part["filename"].as_str().unwrap_or("").is_empty())
        {
            let bytes = URL_SAFE_NO_PAD
                .decode(data.trim_end_matches('='))
                .map_err(|_| "Invalid message encoding")?;
            let text = String::from_utf8_lossy(&bytes).into_owned();
            match part["mimeType"].as_str() {
                Some("text/plain") => plain.push(text),
                Some("text/html") => html.push(text),
                _ => {}
            }
        }
        if let Some(parts) = part["parts"].as_array() {
            for child in parts {
                visit(child, plain, html)?;
            }
        }
        Ok(())
    }
    let (mut plain, mut html) = (vec![], vec![]);
    visit(payload, &mut plain, &mut html)?;
    if !html.is_empty() && (prefer_html || plain.is_empty()) {
        Ok(html_text(&html.join(" ")))
    } else {
        Ok(plain.join("\n"))
    }
}
fn read_emails(client: &Client, token: &str, options: &Emails) -> Result<Vec<Email>> {
    let query = mail_query(options)?;
    let mut ids = vec![];
    let mut next = String::new();
    loop {
        let remaining = usize::from(options.max_emails).saturating_sub(ids.len());
        if remaining == 0 {
            break;
        }
        let mut request = client
            .get("https://gmail.googleapis.com/gmail/v1/users/me/messages")
            .bearer_auth(token)
            .query(&[
                ("q", query.as_str()),
                ("maxResults", &remaining.to_string()),
            ]);
        if !next.is_empty() {
            request = request.query(&[("pageToken", &next)]);
        }
        let response = json_response(request.send())?;
        if let Some(messages) = response["messages"].as_array() {
            for m in messages.iter().take(remaining) {
                ids.push(m["id"].as_str().ok_or("Invalid message ID")?.to_owned());
            }
        }
        let Some(page) = response["nextPageToken"].as_str() else {
            break;
        };
        if page == next || page.is_empty() {
            return Err("Invalid Gmail pagination");
        }
        next = page.into();
    }
    let mut emails = vec![];
    for id in ids {
        if !id.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err("Invalid message ID");
        }
        let mut request = client
            .get(format!(
                "https://gmail.googleapis.com/gmail/v1/users/me/messages/{id}"
            ))
            .bearer_auth(token)
            .query(&[("format", if options.body { "full" } else { "metadata" })]);
        for name in ["From", "To", "Subject", "Date"] {
            request = request.query(&[("metadataHeaders", name)]);
        }
        let data = json_response(request.send())?;
        let payload = &data["payload"];
        let headers = payload["headers"]
            .as_array()
            .ok_or("Invalid message headers")?;
        let header = |name: &str| {
            headers
                .iter()
                .find(|h| {
                    h["name"]
                        .as_str()
                        .is_some_and(|s| s.eq_ignore_ascii_case(name))
                })
                .and_then(|h| h["value"].as_str())
                .unwrap_or("")
                .to_owned()
        };
        let timestamp: i64 = data["internalDate"]
            .as_str()
            .ok_or("Invalid message timestamp")?
            .parse()
            .map_err(|_| "Invalid message timestamp")?;
        let date = jiff::Timestamp::from_millisecond(timestamp)
            .map_err(|_| "Invalid message timestamp")?
            .to_string();
        emails.push(Email {
            from: header("From"),
            to: header("To"),
            subject: header("Subject"),
            date,
            body: if options.body {
                Some(extract_body(payload, options.html)?)
            } else {
                None
            },
            timestamp,
        });
    }
    emails.sort_by_key(|e| std::cmp::Reverse(e.timestamp));
    Ok(emails)
}
fn addresses(header: &str) -> Vec<String> {
    let pattern = Regex::new(r#"^(?:[^<>]*<([^<>]+)>|([^<>\s"]+))$"#).unwrap();
    header
        .split(',')
        .filter_map(|part| {
            pattern
                .captures(part.trim())
                .and_then(|c| c.get(1).or_else(|| c.get(2)))
                .map(|m| m.as_str().to_lowercase())
        })
        .collect()
}
fn verification_code(messages: &[Email], intent: &Intent) -> Result<String> {
    if messages.len() > 2 {
        return Err("Invalid mailbox response");
    }
    let pattern = Regex::new(r"Your Poe verification code is:\s*([0-9]{6})\b").unwrap();
    let mut codes = vec![];
    for mail in messages {
        if addresses(&mail.from) != ["noreply@poe.com"]
            || !addresses(&mail.to).contains(&intent.email)
            || mail.subject != "Your verification code"
        {
            continue;
        }
        let Ok(timestamp) = mail.date.parse::<jiff::Timestamp>() else {
            continue;
        };
        let timestamp = timestamp.as_millisecond();
        if timestamp < intent.requested_at.div_euclid(1000) * 1000 || timestamp > intent.now + 5000
        {
            continue;
        }
        let matches: Vec<_> = pattern
            .captures_iter(mail.body.as_deref().unwrap_or(""))
            .collect();
        if matches.len() == 1 {
            codes.push(matches[0][1].to_owned());
        }
    }
    if codes.len() != 1 {
        return Err("No unique fresh verification mail");
    }
    Ok(codes.remove(0))
}
fn valid_origin(origin: &str) -> bool {
    matches!(
        origin,
        "https://poe.com"
            | "https://poe-tiles-poe--legacy.poe-dev.workers.dev"
            | "https://poe-tiles-poeqa--recovery-oct17.poe-dev.workers.dev"
    )
}
fn browser_script(origin: &str, email: &str, selector: &str, code: &str) -> String {
    format!(
        "({})({})",
        include_str!("../../browser-entry.js"),
        json!({"origin":origin,"email":email,"selector":selector,"code":code})
    )
}
fn run(action: Action) -> Result<()> {
    let client = Client::builder()
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| "HTTP client unavailable")?;
    match action {
        Action::Emails(options) => {
            mail_query(&options)?;
            let token = access_token(&client)?;
            let emails = read_emails(&client, &token, &options)?;
            if options.json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&emails)
                        .map_err(|_| "Message formatting failed")?
                );
            } else {
                for email in emails {
                    println!(
                        "## {}\nFrom: {}\nTo: {}\nDate: {}\n",
                        email.subject, email.from, email.to, email.date
                    );
                    if let Some(body) = email.body {
                        println!("{body}\n");
                    }
                }
            }
        }
        Action::Check => {
            let token = access_token(&client)?;
            json_response(
                client
                    .get("https://gmail.googleapis.com/gmail/v1/users/me/profile")
                    .bearer_auth(token)
                    .send(),
            )?;
            println!("{{\"gmailAccess\":true}}");
        }
        Action::Enter {
            email,
            after,
            origin,
            session,
            code_selector,
        } => {
            let intent = Intent::new(&email, &after, jiff::Timestamp::now().as_millisecond())?;
            if !valid_origin(&origin)
                || !Regex::new(r"^[a-zA-Z0-9_-]{1,80}$")
                    .unwrap()
                    .is_match(&session)
            {
                return Err("Invalid login origin or browser session");
            }
            let token = access_token(&client)?;
            let emails = read_emails(
                &client,
                &token,
                &Emails {
                    to: intent.email.clone(),
                    from: vec!["noreply@poe.com".into()],
                    max_emails: 2,
                    days: None,
                    body: true,
                    html: true,
                    json: true,
                    markdown: false,
                },
            )?;
            let intent = Intent::new(
                &intent.email,
                &after,
                jiff::Timestamp::now().as_millisecond(),
            )?;
            let code = verification_code(&emails, &intent)?;
            let script = browser_script(&origin, &intent.email, &code_selector, &code);
            let bytes = private_command(
                "agent-browser",
                &["--session", &session, "--json", "eval", "--stdin"],
                script.into_bytes(),
            )?;
            let envelope: Value = serde_json::from_slice(&bytes)
                .map_err(|_| "Invalid browser response; output withheld")?;
            if envelope["success"] != true || envelope["data"]["result"]["codeEntered"] != true {
                return Err("Browser code entry failed; output withheld");
            }
            println!("{{\"codeEntered\":true}}");
        }
    }
    Ok(())
}
fn main() {
    if let Err(error) = run(Cli::parse().command) {
        eprintln!("poe-login: {error}");
        std::process::exit(1);
    }
}
#[cfg(test)]
mod tests;
