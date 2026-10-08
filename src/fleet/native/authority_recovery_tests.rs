use super::{tests::test_context, *};

fn bounded(
    ctx: &Context,
    budget: Duration,
    cancel: &dyn Fn() -> bool,
) -> crate::issues::Result<Value> {
    client::exchange(
        &ctx.state,
        &ctx.path,
        json!({"kind":"capabilities"}),
        Instant::now() + budget,
        cancel,
    )
}

#[test]
fn missing_refused_and_unconfigured_relays_have_one_bounded_deadline() {
    let (root, ctx, store) = test_context();
    let output = Arc::new(Mutex::new(Vec::<u8>::new()));
    for phase in ["missing", "refused", "handshake"] {
        let relay = if phase == "handshake" {
            Some(Relay::start(&ctx, output.clone()).unwrap())
        } else {
            if phase == "refused" {
                drop(UnixListener::bind(ctx.state.join(SOCKET)).unwrap());
            }
            None
        };
        let start = Instant::now();
        let error = bounded(&ctx, Duration::from_millis(200), &|| false).unwrap_err();
        assert!(start.elapsed() >= Duration::from_millis(200), "{phase}");
        assert!(start.elapsed() < Duration::from_secs(1), "{phase}");
        assert_eq!(error.code, "fleet_unavailable");
        if phase != "handshake" {
            assert_eq!(error.details.as_ref().unwrap()["sent"], false);
        }
        assert!(!error.message.contains("upgrade"));
        if phase == "handshake"
            && error
                .details
                .as_ref()
                .is_some_and(|details| details["sent"] == false)
        {
            assert!(error.message.contains("handshake"));
        }
        drop(relay);
    }
    assert!(output.lock().unwrap().is_empty());
    drop(store);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn cancellation_stops_socket_and_handshake_waits_without_forwarding() {
    let (root, ctx, store) = test_context();
    let output = Arc::new(Mutex::new(Vec::<u8>::new()));
    for handshake in [false, true] {
        let relay = handshake.then(|| Relay::start(&ctx, output.clone()).unwrap());
        let start = Instant::now();
        let error = bounded(&ctx, Duration::from_secs(5), &|| {
            start.elapsed() >= Duration::from_millis(120)
        })
        .unwrap_err();
        assert!(start.elapsed() < Duration::from_millis(500));
        assert!(error.message.contains("cancelled"));
        // Cancellation can race the pending-handshake acknowledgment; an
        // unacknowledged envelope is conservatively an unknown outcome.
        if !handshake {
            assert_eq!(error.details.unwrap()["sent"], false);
        }
        drop(relay);
    }
    assert!(output.lock().unwrap().is_empty());
    drop(store);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn old_handshake_and_wrong_database_fail_promptly_without_forwarding() {
    let (root, ctx, store) = test_context();
    let output = Arc::new(Mutex::new(Vec::<u8>::new()));
    let relay = Relay::start(&ctx, output.clone()).unwrap();
    let start = Instant::now();
    let error = call(&ctx.state, &root.join("wrong.db"), json!({"kind":"status"})).unwrap_err();
    assert!(error.message.contains("different issue database"));
    assert!(start.elapsed() < Duration::from_millis(500));
    relay.configure(&json!({"capabilities":{}}));
    let start = Instant::now();
    let error = call(&ctx.state, &ctx.path, json!({"kind":"status"})).unwrap_err();
    assert_eq!(error.code, "fleet_capability_unsupported");
    assert_eq!(error.details.unwrap()["sent"], false);
    assert!(start.elapsed() < Duration::from_millis(500));
    assert_eq!(
        call(&ctx.state, &ctx.path, json!({"kind":"capabilities"})).unwrap()["capabilities"]["authority_rpc"],
        false
    );
    assert!(output.lock().unwrap().is_empty());
    drop((relay, store));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn permission_errors_are_not_retried() {
    let (root, ctx, store) = test_context();
    let private = root.join("private");
    fs::create_dir(&private).unwrap();
    let listener = UnixListener::bind(private.join(SOCKET)).unwrap();
    fs::set_permissions(&private, fs::Permissions::from_mode(0o000)).unwrap();
    let start = Instant::now();
    let result = call(&private, &ctx.path, json!({"kind":"capabilities"}));
    fs::set_permissions(&private, fs::Permissions::from_mode(0o700)).unwrap();
    let error = result.unwrap_err();
    assert!(start.elapsed() < Duration::from_millis(500));
    assert_eq!(error.details.unwrap()["sent"], false);
    drop((listener, store));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn lost_mutation_response_is_never_replayed_and_retains_its_request_id() {
    let (root, ctx, store) = test_context();
    let listener = UnixListener::bind(ctx.state.join(SOCKET)).unwrap();
    let server = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let received = read_frame(&mut BufReader::new(stream)).unwrap().unwrap();
        listener.set_nonblocking(true).unwrap();
        thread::sleep(Duration::from_millis(200));
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
        received
    });
    let request = json!({"kind":"resource","request":{"request_id":"lost-write","operation":{"action":"artifact_update","expected_version":7}}});
    let error = call(&ctx.state, &ctx.path, request.clone()).unwrap_err();
    let details = error.details.unwrap();
    assert_eq!(details["outcome"], "unknown");
    assert_eq!(details["request_id"], "lost-write");
    assert_eq!(server.join().unwrap()["request"], request);
    drop(store);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn acknowledged_unavailability_preserves_receipt_context_without_replaying() {
    let (root, ctx, store) = test_context();
    let listener = UnixListener::bind(ctx.state.join(SOCKET)).unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        read_frame(&mut BufReader::new(stream.try_clone().unwrap())).unwrap();
        let mut error = unavailable("supervisor did not acknowledge the request");
        error.details = Some(json!({"receipt":"retained"}));
        send(&mut stream, failure(error)).unwrap();
    });
    let error = call(
        &ctx.state,
        &ctx.path,
        json!({"kind":"resource","request":{"request_id":"retained-id"}}),
    )
    .unwrap_err();
    let details = error.details.unwrap();
    assert_eq!(details["receipt"], "retained");
    assert_eq!(details["request_id"], "retained-id");
    assert_eq!(details["outcome"], "unknown");
    server.join().unwrap();
    drop(store);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn fragmented_reply_survives_poll_timeouts_without_resending() {
    let (root, ctx, store) = test_context();
    let listener = UnixListener::bind(ctx.state.join(SOCKET)).unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        read_frame(&mut BufReader::new(stream.try_clone().unwrap())).unwrap();
        stream.write_all(b"{\"ok\":").unwrap();
        thread::sleep(Duration::from_millis(150));
        stream.write_all(b"true,\"version\":19}\n").unwrap();
    });
    let result = bounded(&ctx, Duration::from_secs(2), &|| false).unwrap();
    assert_eq!(result, json!({"ok":true,"version":19}));
    server.join().unwrap();
    drop(store);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn partial_responses_obey_cancellation_and_the_total_deadline() {
    let (root, ctx, store) = test_context();
    for cancel in [false, true] {
        let listener = UnixListener::bind(ctx.state.join(SOCKET)).unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            read_frame(&mut BufReader::new(stream.try_clone().unwrap())).unwrap();
            for _ in 0..40 {
                if stream.write_all(b" ").is_err() {
                    break;
                }
                thread::sleep(Duration::from_millis(20));
            }
        });
        let start = Instant::now();
        let error = bounded(&ctx, Duration::from_millis(200), &|| {
            cancel && start.elapsed() >= Duration::from_millis(100)
        })
        .unwrap_err();
        assert!(
            error
                .message
                .contains(if cancel { "cancelled" } else { "deadline" })
        );
        assert!(start.elapsed() < Duration::from_millis(500));
        assert_eq!(error.details.unwrap()["outcome"], "unknown");
        server.join().unwrap();
        fs::remove_file(ctx.state.join(SOCKET)).unwrap();
    }
    drop(store);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn delayed_socket_and_configure_recover_without_resubmission() {
    let (root, ctx, store) = test_context();
    let output = Arc::new(Mutex::new(Vec::<u8>::new()));
    let client_ctx = ctx.clone();
    let client = thread::spawn(move || {
        call(
            &client_ctx.state,
            &client_ctx.path,
            json!({"kind":"capabilities"}),
        )
    });
    thread::sleep(Duration::from_millis(150));
    let relay = Relay::start(&ctx, output.clone()).unwrap();
    thread::sleep(Duration::from_millis(150));
    relay.configure(&json!({"build":"current", "capabilities":{"authority_rpc":true}}));
    let result = client.join().unwrap().unwrap();
    assert_eq!(result["supervisor_build"], "current");
    assert_eq!(result["capabilities"]["authority_rpc"], true);
    assert!(output.lock().unwrap().is_empty());
    drop((relay, store));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn handshake_gap_does_not_report_unsupported_or_forward_mutations() {
    let (root, ctx, store) = test_context();
    let output = Arc::new(Mutex::new(Vec::<u8>::new()));
    let relay = Relay::start(&ctx, output.clone()).unwrap();
    let client_ctx = ctx.clone();
    let request = json!({"kind":"resource","request":{"request_id":"stable-id","operation":{"action":"artifact_update","expected_version":7}}});
    let expected = request.clone();
    let client = thread::spawn(move || call(&client_ctx.state, &client_ctx.path, request));
    thread::sleep(Duration::from_millis(150));
    assert!(output.lock().unwrap().is_empty());
    relay.configure(&json!({"build":"current", "capabilities":{"authority_rpc":true}}));
    let deadline = Instant::now() + Duration::from_secs(2);
    let forwarded = loop {
        if let Ok(frame) = serde_json::from_slice::<Value>(&output.lock().unwrap()) {
            break frame;
        }
        assert!(
            Instant::now() < deadline,
            "request was not forwarded after configure"
        );
        thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(forwarded["request"], expected);
    relay.receive(json!({"id":forwarded["id"],"result":failure(Error::new("revision_conflict", "stale guard"))}));
    assert_eq!(
        client.join().unwrap().unwrap_err().code,
        "revision_conflict"
    );
    assert_eq!(
        output
            .lock()
            .unwrap()
            .iter()
            .filter(|&&b| b == b'\n')
            .count(),
        1
    );
    drop((relay, store));
    fs::remove_dir_all(root).unwrap();
}
