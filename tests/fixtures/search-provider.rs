//! Disposable stand-in for `hey-proxy embed` in the search workload.
use serde_json::{Value, json};
use std::{fs, io};
fn main() {
    assert_eq!(std::env::args().nth(1).as_deref(), Some("embed"));
    let request: Value = serde_json::from_reader(io::stdin().lock()).unwrap();
    let input = request["input"].as_array().unwrap();
    assert_eq!(request["dimensions"], 512);
    fs::write("provider-called", "").unwrap();
    match fs::read_to_string("provider-mode").unwrap().as_str() {
        "down" => std::process::exit(1),
        "slow" => {
            if input == &[json!("Network issue slow")] {
                fs::write("slow-query-started", "").unwrap();
            }
            std::thread::sleep(std::time::Duration::from_secs(30));
            std::process::exit(1)
        }
        "ok" => (),
        mode => panic!("unknown provider mode {mode}"),
    }
    let mut vector = [0; 512];
    vector[0] = 1;
    let data = input
        .iter()
        .enumerate()
        .map(|(index, _)| json!({"index":index,"embedding":&vector[..]}))
        .collect::<Vec<_>>();
    serde_json::to_writer(io::stdout().lock(), &json!({"data":data})).unwrap();
}
