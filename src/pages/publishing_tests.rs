use super::*;
use crate::pages::tests::Fixture;
use std::os::unix::fs::PermissionsExt;

fn provider(f: &Fixture) -> Provider {
    let script = f.0.join("provider.sh");
    fs::write(&script, r#"#!/bin/sh
set -eu
root=$(dirname "$0")
printf '%s\n' "$1" >> "$root/calls"
case "$1" in
  read) printf '{"artifact_id":"remote-id","artifact_url":"https://example.com/report","version":"v1"}'; exit ;;
  create|update) test -f "$2/index.html" ;;
esac
if test -f "$root/fail"; then echo 'provider unavailable' >&2; exit 1; fi
if test -f "$root/json-fail"; then echo '{"level":"error","message":"JSON provider failure"}'; exit 1; fi
if test "$1" = update; then test "$3" = remote-id; test "$4" = v1; fi
printf '{"artifact_id":"remote-id","artifact_url":"https://example.com/report","version":"v1"}'
"#).unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
    serde_json::from_value(serde_json::json!({
        "command":script,"audience":"public",
        "create":["create","{source}"],"update":["update","{source}","{id}","{version}"],"read":["read","{id}"],
        "fields":{"id":"/artifact_id","url":"/artifact_url","version":"/version"}
    })).unwrap()
}

#[test]
fn external_publishing_remembers_link_and_tracks_changed_failed_and_retried_content() {
    let f = Fixture::new();
    let database = f.0.join("issues.db");
    let source = f.0.join("index.html");
    fs::write(&source, "first").unwrap();
    super::super::publish(&database, &source, "report").unwrap();
    let p = provider(&f);
    share_with(&database, "report", "fixture", &p, None, false).unwrap();
    let page = super::super::view(&database, "report").unwrap();
    assert!(page.publications[0].synced);
    assert_eq!(page.publications[0].url, "https://example.com/report");
    fs::write(&source, "second").unwrap();
    let page = super::super::publish(&database, &source, "report").unwrap();
    assert!(!page.publications[0].synced);
    fs::write(f.0.join("fail"), "").unwrap();
    assert!(share_with(&database, "report", "fixture", &p, None, false).is_err());
    let page = super::super::view(&database, "report").unwrap();
    assert!(
        page.publications[0]
            .error
            .as_deref()
            .unwrap()
            .contains("provider unavailable")
    );
    assert_eq!(page.publications[0].url, "https://example.com/report");
    fs::remove_file(f.0.join("fail")).unwrap();
    share_with(&database, "report", "fixture", &p, None, false).unwrap();
    assert!(
        super::super::view(&database, "report")
            .unwrap()
            .publications[0]
            .synced
    );
    assert_eq!(
        fs::read_to_string(f.0.join("calls")).unwrap(),
        "create\nupdate\nupdate\n"
    );
    super::super::delete(&database, "report").unwrap();
    assert!(
        super::super::publish(&database, &source, "report")
            .unwrap()
            .publications
            .is_empty()
    );
}

#[test]
fn uncertain_creation_requires_linking_before_retry_and_never_creates_twice() {
    let f = Fixture::new();
    let database = f.0.join("issues.db");
    let source = f.0.join("index.html");
    fs::write(&source, "first").unwrap();
    super::super::publish(&database, &source, "report").unwrap();
    let p = provider(&f);
    fs::write(f.0.join("json-fail"), "").unwrap();
    let error = share_with(&database, "report", "fixture", &p, None, true).unwrap_err();
    assert!(error.message.contains("JSON provider failure"));
    fs::remove_file(f.0.join("json-fail")).unwrap();
    assert!(share_with(&database, "report", "fixture", &p, None, true).is_err());
    share_with(
        &database,
        "report",
        "fixture",
        &p,
        Some("https://example.com/report"),
        true,
    )
    .unwrap();
    let page = super::super::view(&database, "report").unwrap();
    assert!(page.publications[0].synced);
    assert!(page.publications[0].auto_sync);
    assert_eq!(
        fs::read_to_string(f.0.join("calls")).unwrap(),
        "create\nread\nupdate\n"
    );
}
