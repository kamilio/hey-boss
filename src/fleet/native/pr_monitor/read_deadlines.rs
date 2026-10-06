use super::*;
use serde_json::json;

#[test]
fn metadata_backfill_fallback_shares_the_remaining_read_budget() {
    for inherited in [false, true] {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let mut client =
            ApiClient::new(format!("http://{}/", server.server_addr()).parse().unwrap()).unwrap();
        if inherited {
            client =
                client.with_read_deadline(tokio::time::Instant::now() + Duration::from_secs(5));
        }
        let serving = std::thread::spawn(move || {
            let cached = server
                .recv_timeout(Duration::from_secs(3))
                .unwrap()
                .unwrap();
            assert!(cached.url().ends_with("metadata?cached_only=true"));
            let first = tests::read_budget(&cached, if inherited { 5_000 } else { 20_000 });
            // A cache miss must not reset either the caller or metadata budget.
            std::thread::sleep(Duration::from_millis(30));
            cached
                .respond(
                    tiny_http::Response::from_string(
                        json!({"code":"cache_miss","error":"missing cache"}).to_string(),
                    )
                    .with_status_code(404),
                )
                .unwrap();
            let fallback = server
                .recv_timeout(Duration::from_secs(3))
                .unwrap()
                .unwrap();
            assert!(fallback.url().ends_with("metadata?max_age_seconds=300"));
            let second = tests::read_budget(&fallback, first);
            assert!(second < first, "Fallback reset its total read budget");
            fallback.respond(tiny_http::Response::from_string(
                json!({"data":{"number":1},"validated_at_ms":123,"fetched_at_ms":123,"source":"network"}).to_string(),
            )).unwrap();
        });
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let response = runtime
            .block_on(tokio_read(&client, "o/r", 1, true))
            .unwrap();
        assert_eq!(response.data["number"], 1);
        serving.join().unwrap();
    }
}
