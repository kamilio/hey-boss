use super::*;
use std::net::{SocketAddr, TcpListener};

#[test]
fn optional_port_failure_preserves_the_primary_listener_and_reports_the_cause() {
    for kind in [
        std::io::ErrorKind::PermissionDenied,
        std::io::ErrorKind::AddrInUse,
    ] {
        let mut servers = vec![Server::http(("127.0.0.1", 0)).unwrap()];
        let primary = servers[0].server_addr().to_ip().unwrap();
        let (activated, error) = add_local_http(&mut servers, Err(std::io::Error::from(kind)));
        assert!(!activated);
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].server_addr().to_ip().unwrap(), primary);
        let error = error.unwrap();
        assert!(error.contains("Port 4781 remains available"));
        assert!(error.contains(&std::io::Error::from(kind).to_string()));
        assert!(std::net::TcpStream::connect(primary).is_ok());
    }
}

#[test]
fn default_port_origins_are_equivalent_but_other_local_addresses_are_cross_origin() {
    let mobile = Some("https://mac.example.ts.net");
    for host in ["hey-boss.test", "hey-boss.test:80"] {
        for origin in ["http://hey-boss.test", "http://hey-boss.test:80"] {
            assert!(same_origin(
                Some(host),
                Some(origin),
                Some("same-origin"),
                &[80, 4781],
                mobile
            ));
        }
        for origin in [
            "http://localhost",
            "http://127.0.0.1",
            "http://hey-boss.test:4781",
            "https://hey-boss.test",
            "https://mac.example.ts.net",
            "null",
            "http://hey-boss.test/",
            "http://hey-boss.test:80?x",
        ] {
            assert!(
                !same_origin(Some(host), Some(origin), None, &[80, 4781], mobile),
                "{host} / {origin}"
            );
        }
        assert!(!same_origin(
            Some(host),
            None,
            Some("cross-site"),
            &[80, 4781],
            mobile
        ));
        assert!(!same_origin(
            Some(host),
            None,
            Some("same-site"),
            &[80, 4781],
            mobile
        ));
    }
    assert!(same_origin(
        Some("mac.example.ts.net"),
        Some("https://mac.example.ts.net"),
        None,
        &[80, 4781],
        mobile
    ));
    assert!(!same_origin(
        Some("mac.example.ts.net"),
        Some("http://hey-boss.test"),
        None,
        &[80, 4781],
        mobile
    ));
    assert!(!same_origin(None, None, None, &[80, 4781], mobile));
}

struct Fixture {
    root: std::path::PathBuf,
    app: Arc<App>,
    stop: Arc<AtomicBool>,
    server: Option<std::thread::JoinHandle<()>>,
    addresses: Vec<SocketAddr>,
}

impl Fixture {
    fn start() -> Self {
        let root =
            std::env::temp_dir().join(format!("hey-boss-71-listeners-{}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        let machine = identity::machine().unwrap();
        let project = identity::project(&root, &machine).unwrap();
        let servers = (0..2)
            .map(|_| Server::http(("127.0.0.1", 0)).unwrap())
            .collect::<Vec<_>>();
        let addresses = servers
            .iter()
            .map(|server| server.server_addr().to_ip().unwrap())
            .collect::<Vec<_>>();
        let app = Arc::new(App {
            backend: Backend::Local(root.join("issues.db")),
            project,
            actor: Actor {
                id: "human:boss".into(),
                kind: "human".into(),
                session_id: None,
                machine,
                host: "test".into(),
                pid: None,
                process_start: None,
                cwd: root.clone(),
                source: "listener test".into(),
                invocation: None,
                creation_run: None,
                model: None,
            },
            token: "one-shared-csrf-token".into(),
            authority: addresses[0].to_string(),
            local_ports: addresses.iter().map(|a| a.port()).collect(),
            mobile_origin: Some("https://mac.example.ts.net".into()),
        });
        let stop = Arc::new(AtomicBool::new(false));
        let server = {
            let app = Arc::clone(&app);
            let stop = Arc::clone(&stop);
            std::thread::spawn(move || serve_requests(&servers, &app, &stop))
        };
        Self {
            root,
            app,
            stop,
            server: Some(server),
            addresses,
        }
    }

    fn request(
        &self,
        listener: usize,
        path: &str,
        host: &str,
        origin: Option<&str>,
        token: Option<&str>,
        operation: Option<Value>,
    ) -> reqwest::blocking::Response {
        let client = reqwest::blocking::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(10))
            .build()
            .unwrap();
        let url = format!("http://{}{path}", self.addresses[listener]);
        let mut request = if let Some(operation) = operation {
            client
                .post(url)
                .json(&json!({"project":self.app.project.id,"operation":operation}))
        } else {
            client.get(url)
        }
        .header("Host", host);
        if let Some(origin) = origin {
            request = request.header("Origin", origin);
        }
        if let Some(token) = token {
            request = request.header("X-Hey-Boss-CSRF", token);
        }
        request.send().unwrap()
    }

    fn shutdown(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(server) = self.server.take() {
            server.join().unwrap();
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.shutdown();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn simultaneous_listeners_share_writes_csrf_routes_and_release_both_sockets() {
    let mut fixture = Fixture::start();
    let hosts = [
        fixture.addresses[0].to_string(),
        format!("hey-boss.test:{}", fixture.addresses[1].port()),
    ];
    let origins = hosts.each_ref().map(|host| format!("http://{host}"));
    for listener in 0..2 {
        let boot: Value = fixture
            .request(
                listener,
                "/api/bootstrap",
                &hosts[listener],
                None,
                None,
                None,
            )
            .json()
            .unwrap();
        assert_eq!(boot["csrf"], fixture.app.token);
        assert_eq!(
            fixture
                .request(listener, "/mm?focus=1", &hosts[listener], None, None, None)
                .status(),
            200
        );
        let create = json!({"action":"create","title":format!("Listener {listener}"),"body":"Shared database","labels":[]});
        assert_eq!(
            fixture
                .request(
                    listener,
                    "/api/action",
                    &hosts[listener],
                    Some(&origins[listener]),
                    None,
                    Some(create.clone())
                )
                .status(),
            403
        );
        assert_eq!(
            fixture
                .request(
                    listener,
                    "/api/action",
                    &hosts[listener],
                    Some(&origins[1 - listener]),
                    Some(&fixture.app.token),
                    Some(create.clone())
                )
                .status(),
            403
        );
        assert_eq!(
            fixture
                .request(
                    listener,
                    "/api/action",
                    &hosts[listener],
                    Some(&origins[listener]),
                    Some(&fixture.app.token),
                    Some(create)
                )
                .status(),
            200
        );
        let view: Value = fixture
            .request(
                1 - listener,
                "/api/action",
                &hosts[1 - listener],
                Some(&origins[1 - listener]),
                Some(&fixture.app.token),
                Some(json!({"action":"view","number":listener+1})),
            )
            .json()
            .unwrap();
        assert_eq!(view["issue"]["title"], format!("Listener {listener}"));
        // Event streams use the same guard before they contact the fleet.
        assert_eq!(
            fixture
                .request(
                    listener,
                    "/api/fleet/events?cursor=1",
                    "evil.example",
                    None,
                    None,
                    None
                )
                .status(),
            403
        );
        let mobile: Value = fixture
            .request(
                listener,
                "/api/bootstrap",
                "mac.example.ts.net",
                Some("https://mac.example.ts.net"),
                None,
                None,
            )
            .json()
            .unwrap();
        assert_eq!(mobile["csrf"], fixture.app.token);
    }
    fixture.shutdown();
    // tiny_http wakes its accept threads on drop. Allow them to finish closing.
    for address in &fixture.addresses {
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            match TcpListener::bind(address) {
                Ok(_) => break,
                Err(error) if std::time::Instant::now() < deadline => {
                    assert_eq!(error.kind(), std::io::ErrorKind::AddrInUse);
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("Listener {address} was not released: {error}"),
            }
        }
    }
}
