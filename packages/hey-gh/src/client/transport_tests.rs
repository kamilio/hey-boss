use super::{Client, Config, Freshness};
use axum::body::Body;
use hyper::{Response, Version, service::service_fn};
use hyper_util::{
    rt::{TokioExecutor, TokioIo},
    server::conn::auto::Builder,
};
use std::{
    convert::Infallible,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::{net::TcpListener, sync::Notify};
use tokio_rustls::{
    TlsAcceptor,
    rustls::{ServerConfig, pki_types::PrivatePkcs8KeyDer},
};

#[tokio::test]
async fn github_transport_negotiates_http2_and_multiplexes_scheduled_reads() {
    exercise_transport(true, Version::HTTP_2, 1).await;
}

#[tokio::test]
async fn github_transport_keeps_http1_fallback_for_other_servers() {
    exercise_transport(false, Version::HTTP_11, 2).await;
}

async fn exercise_transport(http2: bool, expected: Version, connections: usize) {
    let cert = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
    let trusted = reqwest::Certificate::from_der(cert.cert.der()).unwrap();
    let mut tls = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(
            vec![cert.cert.der().clone()],
            PrivatePkcs8KeyDer::from(cert.key_pair.serialize_der()).into(),
        )
        .unwrap();
    tls.alpn_protocols = if http2 {
        vec![b"h2".to_vec(), b"http/1.1".to_vec()]
    } else {
        vec![b"http/1.1".to_vec()]
    };
    let acceptor = TlsAcceptor::from(Arc::new(tls));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!(
        "https://localhost:{}/",
        listener.local_addr().unwrap().port()
    );
    let accepted = Arc::new(AtomicUsize::new(0));
    let versions = Arc::new(Mutex::new(Vec::new()));
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let server = {
        let (accepted, versions, started, release) = (
            accepted.clone(),
            versions.clone(),
            started.clone(),
            release.clone(),
        );
        tokio::spawn(async move {
            let mut tasks = tokio::task::JoinSet::new();
            loop {
                let (socket, _) = listener.accept().await.unwrap();
                accepted.fetch_add(1, Ordering::SeqCst);
                let (acceptor, versions, started, release) = (
                    acceptor.clone(),
                    versions.clone(),
                    started.clone(),
                    release.clone(),
                );
                tasks.spawn(async move {
                    let stream = acceptor.accept(socket).await.unwrap();
                    let service =
                        service_fn(move |request: hyper::Request<hyper::body::Incoming>| {
                            versions.lock().unwrap().push(request.version());
                            let (started, release) = (started.clone(), release.clone());
                            async move {
                                if request.uri().path().ends_with("/comments") {
                                    started.notify_one();
                                    release.notified().await;
                                }
                                Ok::<_, Infallible>(Response::new(Body::from(r#"{"ok":true}"#)))
                            }
                        });
                    let _ = Builder::new(TokioExecutor::new())
                        .serve_connection(TokioIo::new(stream), service)
                        .await;
                });
            }
        })
    };
    let dir = tempfile::tempdir().unwrap();
    let client = Client::with_http(
        Config {
            rest_url: url.parse().unwrap(),
            graphql_url: format!("{url}graphql").parse().unwrap(),
            cache_path: dir.path().join("cache.sqlite"),
            min_spacing: Duration::ZERO,
            max_attempts: 1,
            request_timeout: Duration::from_secs(5),
            ..Config::default()
        },
        "synthetic-token".into(),
        reqwest::Client::builder().add_root_certificate(trusted),
    )
    .unwrap();
    client.get("user", Freshness::Revalidate).await.unwrap();
    assert_eq!(versions.lock().unwrap().as_slice(), &[expected]);
    let held = {
        let client = client.clone();
        tokio::spawn(async move {
            client
                .get("repos/acme/demo/issues/1/comments", Freshness::Revalidate)
                .await
        })
    };
    tokio::time::timeout(Duration::from_secs(5), started.notified())
        .await
        .unwrap();
    // The existing detail/core lanes allow this read while comments are held.
    // HTTP/2 must reuse the warm TLS connection, rather than open another one.
    let fast = tokio::time::timeout(
        Duration::from_secs(3),
        client.get("repos/acme/demo/pulls/1", Freshness::Revalidate),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(fast.data["ok"], true);
    assert_eq!(accepted.load(Ordering::SeqCst), connections);
    release.notify_one();
    held.await.unwrap().unwrap();
    assert!(
        versions
            .lock()
            .unwrap()
            .iter()
            .all(|version| *version == expected)
    );
    server.abort();
}
