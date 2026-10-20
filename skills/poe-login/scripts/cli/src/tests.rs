use super::*;
use serde_json::json;

fn mail(to: &str, date: &str, body: &str) -> Email {
    Email {
        subject: "Your verification code".into(),
        from: "Poe <noreply@poe.com>".into(),
        to: to.into(),
        date: date.into(),
        body: Some(body.into()),
        timestamp: 0,
    }
}

#[test]
fn picks_only_one_fresh_exact_recipient_and_sender() {
    let intent = Intent::new(
        "kjopek+qa-test@quora.com",
        "2026-10-20T12:00:00Z",
        1792497605000,
    )
    .unwrap();
    let good = mail(
        &intent.email,
        "2026-10-20T12:00:01Z",
        "Your Poe verification code is: 123456",
    );
    assert_eq!(
        verification_code(
            &[
                good.clone(),
                mail(
                    "kjopek+other@quora.com",
                    &good.date,
                    good.body.as_ref().unwrap()
                )
            ],
            &intent
        )
        .unwrap(),
        "123456"
    );
    assert!(verification_code(&[good.clone(), good.clone()], &intent).is_err());
    let mut bad = good.clone();
    bad.to = format!("\"{}\" <someone@example.invalid>", intent.email);
    assert!(verification_code(&[bad], &intent).is_err());
    for date in [
        "2026-10-20T11:59:59Z",
        "2026-10-20T12:00:11Z",
        "2026-10-20T12:00:01",
    ] {
        assert!(
            verification_code(
                &[mail(&intent.email, date, good.body.as_ref().unwrap())],
                &intent
            )
            .is_err()
        );
    }
    for body in [
        "Your Poe verification code is: 1234567",
        "Unrelated 123456",
        "Your Poe verification code is: 123456 Your Poe verification code is: 654321",
    ] {
        assert!(verification_code(&[mail(&intent.email, &good.date, body)], &intent).is_err());
    }
    let mut bad = good.clone();
    bad.from = "noreply@poe.com.attacker.invalid".into();
    assert!(verification_code(&[bad], &intent).is_err());
    assert!(Intent::new("other@quora.com", "2026-10-20T12:00:00Z", intent.now).is_err());
    assert!(Intent::new(&intent.email, "2026-10-20T12:00:00Z", intent.now + 600_000).is_err());
}

#[test]
fn decodes_nested_gmail_mime_and_honors_html_preference() {
    let encode = |s: &str| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(s);
    let payload = json!({"mimeType":"multipart/mixed","parts":[{"mimeType":"multipart/alternative","parts":[{"mimeType":"text/plain","body":{"data":encode("plain")}}, {"mimeType":"text/html","body":{"data":encode("<html><style>hidden</style><p>Your Poe verification code is: <b>123456</b></p></html>")}}]}]});
    assert_eq!(extract_body(&payload, false).unwrap(), "plain");
    assert_eq!(
        extract_body(&payload, true).unwrap(),
        "Your Poe verification code is: 123456"
    );
    assert!(extract_body(&json!({"mimeType":"text/plain","body":{"data":"!"}}), false).is_err());
}

#[test]
fn query_filters_cannot_inject_other_mailboxes() {
    assert!(
        mail_query(&Emails {
            to: "a@example.com OR in:anywhere".into(),
            from: vec!["noreply@poe.com".into()],
            max_emails: 2,
            days: None,
            body: true,
            html: true,
            json: true,
            markdown: false
        })
        .is_err()
    );
}

#[test]
fn login_origin_allowlist_is_exact() {
    assert!(valid_origin("https://poe.com"));
    assert!(valid_origin(
        "https://poe-tiles-poe--legacy.poe-dev.workers.dev"
    ));
    for origin in [
        "http://poe.com",
        "https://poe.com.attacker.invalid",
        "https://poe.com/login",
        "https://poe.com@attacker.invalid",
    ] {
        assert!(!valid_origin(origin));
    }
}

#[test]
fn subprocess_errors_never_return_private_output() {
    let result = private_command(
        "/bin/sh",
        &["-c", "printf synthetic-private-content; exit 1"],
        Vec::new(),
    );
    assert_eq!(
        result.unwrap_err(),
        "Private command failed; output withheld"
    );
}
