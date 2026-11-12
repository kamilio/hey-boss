// Synthetic CLI for the terminal tests. Only operates on its current fixture directory.
use serde_json::{Value, json};
use std::{env, fs, path::Path, process, thread, time::Duration};

fn main() {
    let args: Vec<_> = env::args().skip(1).collect();
    if args == ["--print-path"] {
        println!("{}", env::current_exe().unwrap().display());
        return;
    }
    let mut data: Value = serde_json::from_slice(&fs::read("snapshot.json").unwrap()).unwrap();
    let args: Vec<_> = args.iter().map(String::as_str).collect();
    let save = |data: &Value| {
        fs::write("snapshot.new", data.to_string()).unwrap();
        fs::rename("snapshot.new", "snapshot.json").unwrap();
        println!("{}", json!({"ok":true}));
    };
    match args.as_slice() {
        [_, "--json", .., "status"] => {
            if Path::new("offline").exists() {
                eprintln!("Synthetic queue offline");
                process::exit(1);
            }
            if Path::new("slow").exists() {
                fs::write("reading", "").unwrap();
                thread::sleep(Duration::from_secs(4));
            }
            println!("{data}");
        }
        ["worker", "--json", action @ ("pause" | "stop"), id] if data["controls"].is_object() => {
            let worker = data["workers"]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .find(|w| w["id"] == *id)
                .unwrap();
            worker["config"]["enabled"] = json!(false);
            if *action == "stop" {
                worker["pid"] = Value::Null;
            }
            data["controls"][*id] = json!(action);
            save(&data);
        }
        ["workers", "--json", "add", flags @ ..] if data["mutations"].is_array() => {
            let value = |key| flags.windows(2).find(|p| p[0] == key).unwrap()[1];
            let name = value("--name");
            let slots = value("--concurrency").parse::<usize>().unwrap();
            let dirs: Vec<_> = flags
                .windows(2)
                .filter(|p| p[0] == "--directory")
                .map(|p| p[1])
                .collect();
            data["mutations"]
                .as_array_mut()
                .unwrap()
                .push(json!({"action":"add","name":name,"slots":slots,"directories":dirs}));
            data["workers"].as_array_mut().unwrap().push(json!({"id":value("--id"),"pid":42,"active":0,"config":{"name":name,"enabled":true,"concurrency":slots,"projects":["named:Alpha"],"directory":dirs.join(" · ")},"runs":[]}));
            save(&data);
        }
        ["workers", "--json", "remove", id] if data["mutations"].is_array() => {
            data["mutations"]
                .as_array_mut()
                .unwrap()
                .push(json!({"action":"remove","id":id}));
            let worker = data["workers"]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .find(|w| w["id"] == *id)
                .unwrap();
            worker["intent"] = json!("drain");
            worker["config"]["enabled"] = json!(false);
            save(&data);
        }
        _ => {
            fs::write("unexpected-command", format!("{args:?}")).unwrap();
            panic!("unexpected fixture invocation: {args:?}")
        }
    }
}
