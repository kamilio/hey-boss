use super::*;
use serde_json::json;

#[test]
fn identity_revalidation_respects_the_inherited_read_budget() {
    for inherited in [false, true] {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let mut client =
            ApiClient::new(format!("http://{}/", server.server_addr()).parse().unwrap()).unwrap();
        if inherited {
            client =
                client.with_read_deadline(tokio::time::Instant::now() + Duration::from_secs(5));
        }
        let serving = std::thread::spawn(move || {
            let repair = server
                .recv_timeout(Duration::from_secs(3))
                .unwrap()
                .unwrap();
            assert!(repair.url().ends_with("metadata?refresh=true"));
            tests::read_budget(&repair, if inherited { 5_000 } else { 20_000 });
            repair.respond(tiny_http::Response::from_string(
                json!({"data":{"number":1},"validated_at_ms":123,"fetched_at_ms":123,"source":"network"}).to_string(),
            )).unwrap();
        });
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let response = runtime
            .block_on(repair_identity(&client, "o/r", 1))
            .unwrap();
        assert_eq!(response.data["number"], 1);
        serving.join().unwrap();
    }
}
