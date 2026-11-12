//! Measure foreground work against a full synthetic corpus and a private embedding provider.
use super::*;
use rusqlite::Connection;
use std::time::Instant;

fn distribution(mut ms: Vec<f64>) -> Value {
    ms.sort_by(f64::total_cmp);
    json!({"median_ms":ms[ms.len()/2],"p95_ms":ms[ms.len()*95/100]})
}

#[test]
#[ignore = "build --example search-provider first; full 108,001-passage workload"]
fn full_index_search_with_foreground_writes_and_provider_outages() {
    let mut f = installed_fixture();
    f.init("SearchBench").unwrap();
    let root = f.root.path();
    fs::copy(
        std::env::current_exe()
            .unwrap()
            .with_file_name("search-provider"),
        root.join("hey-proxy"),
    )
    .expect("cargo build --example search-provider");
    let mode = |name: &str| fs::write(root.join("provider-mode"), name).unwrap();
    mode("ok");
    let db = Connection::open(&f.db).unwrap();
    db.busy_timeout(Duration::from_secs(5)).unwrap();
    db.execute_batch(&format!("BEGIN;
        INSERT OR IGNORE INTO agents VALUES('human:boss','{{\"id\":\"human:boss\",\"kind\":\"human\"}}',1);
        UPDATE projects SET next_number=9001 WHERE id='named:SearchBench';
        WITH RECURSIVE n(x) AS(VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<9000)
        INSERT INTO issues(project_id,number,title,body,state,created_by,created_at,updated_at,version,labels,sort_order)
        SELECT 'named:SearchBench',x,'Network issue '||x,printf('Reconnect after sleep %d. {}',x),'open','human:boss',1,1,1,'[]',x FROM n;
        WITH RECURSIVE n(x) AS(VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<90000)
        INSERT INTO comments(project_id,issue_number,author,body,created_at)
        SELECT 'named:SearchBench',((x-1)%9000)+1,'human:boss',printf('Comment %d. {}',x),1 FROM n; COMMIT;",
        "Network changes can interrupt a background connection. ".repeat(12),"The connection should recover after waking and retry the queued update. ".repeat(8))).unwrap();
    // Indexing runs in the installed database owner; library fixtures skip provider calls.
    f.owner.stop();
    let _owner = Service(
        f.command(&["fleet", "companion"])
            .stdout(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let (web, base) = web(&f, "SearchBench");
    let owner = hey_boss::database::Connection::connect(&f.db).unwrap();
    let http = reqwest::blocking::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(30))
        .build()
        .unwrap();
    let boot: Value = http
        .get(format!("{base}/api/bootstrap"))
        .send()
        .unwrap()
        .json()
        .unwrap();
    let api = |operation: Value| {
        let reply: Value = http
            .post(format!("{base}/api/action"))
            .header("Origin", &base)
            .header("X-Hey-Boss-CSRF", boot["csrf"].as_str().unwrap())
            .json(&json!({"project":"named:SearchBench","operation":operation}))
            .send()
            .unwrap()
            .json()
            .unwrap();
        assert_eq!(reply["ok"], true, "{reply}");
        reply
    };
    let list = || {
        let r = api(
            json!({"action":"list","state":"active","mine":false,"unassigned":false,"labels":[],"search":null,"limit":50,"offset":0}),
        );
        assert_eq!(r["issues"].as_array().unwrap().len(), 50);
    };
    let index = |enabled| api(json!({"action":"search_index","enabled":enabled}));
    let ms = |t: Instant| t.elapsed().as_secs_f64() * 1000.;
    for phase in ["disabled", "backfill", "provider_down"] {
        if phase == "backfill" {
            index(true);
            let deadline = Instant::now() + Duration::from_secs(30);
            while !root.join("provider-called").exists() {
                assert!(
                    Instant::now() < deadline,
                    "background provider never ran: {}",
                    api(json!({"action":"search_index"}))
                );
                thread::sleep(Duration::from_millis(100));
            }
        }
        if phase == "provider_down" {
            mode("down");
        }
        let (mut reads, mut writes, mut sync) = (Vec::new(), Vec::new(), Vec::new());
        for i in 0..30 {
            let t = Instant::now();
            list();
            reads.push(ms(t));
            let t = Instant::now();
            api(
                json!({"action":"edit","number":9000,"title":"Network issue 9000","body":format!("Fresh foreground edit {phase} {i}"),"add_labels":[],"remove_labels":[],"if_version":null}),
            );
            writes.push(ms(t));
            let t = Instant::now();
            assert_eq!(
                owner
                    .execute(
                        "UPDATE issues SET body=?1 WHERE number BETWEEN 8900 AND 8924",
                        [format!("Replica batch {phase} {i}")]
                    )
                    .unwrap(),
                25
            );
            sync.push(ms(t));
            thread::sleep(Duration::from_millis(100));
        }
        println!(
            "{}",
            json!({"phase":phase,"list":distribution(reads),"save":distribution(writes),"replica_25_updates":distribution(sync)})
        );
        assert_eq!(root.join("provider-called").exists(), phase != "disabled");
    }
    let t = Instant::now();
    let fallback = api(json!({"action":"search","query":"Network issue outage"}));
    assert_eq!(fallback["search"]["mode"], "keywords");
    assert!(!fallback["issues"].as_array().unwrap().is_empty());
    println!("{}", json!({"outage_search_ms":ms(t)}));
    db.execute_batch("PRAGMA wal_checkpoint(PASSIVE)").unwrap();
    let derived = Connection::open(f.db.with_extension("db.search.db")).unwrap();
    derived.busy_timeout(Duration::from_secs(5)).unwrap();
    derived
        .execute("DELETE FROM meta WHERE key='query_retry_after'", [])
        .unwrap();
    mode("slow");
    thread::scope(|s| {
        let pending = s.spawn(|| api(json!({"action":"search","query":"Network issue slow"})));
        let start = Instant::now();
        while !root.join("slow-query-started").exists() {
            assert!(
                !pending.is_finished() && start.elapsed() < Duration::from_secs(10),
                "provider never saw the slow query"
            );
            thread::sleep(Duration::from_millis(5));
        }
        let t = Instant::now();
        list();
        let read = ms(t);
        let t = Instant::now();
        api(json!({"action":"comment","number":1,"body":"Comment while embeddings wait"}));
        println!(
            "{}",
            json!({"during_slow_query":{"list_ms":read,"save_ms":ms(t)}})
        );
        assert!(
            !pending.is_finished(),
            "Slow embeddings blocked foreground operations"
        );
        assert_eq!(pending.join().unwrap()["search"]["mode"], "keywords");
    });
    index(false);
    let rss = Command::new("ps")
        .args(["-o", "rss=", "-p", &web.0.id().to_string()])
        .output()
        .unwrap();
    assert!(rss.status.success());
    println!(
        "{}",
        json!({"index":api(json!({"action":"search_index"})),"rss_kib":String::from_utf8(rss.stdout).unwrap().trim().parse::<u64>().unwrap()})
    );
    derived
        .execute("ATTACH ? AS source", [f.db.to_str().unwrap()])
        .unwrap();
    derived.execute_batch("BEGIN; DELETE FROM passages; DELETE FROM embeddings; DELETE FROM documents;
        INSERT INTO passages(project,number,field,source,part,text,hash) SELECT project_id,number,'title',0,0,title,'title-'||number FROM source.issues;
        INSERT INTO passages(project,number,field,source,part,text,hash) SELECT project_id,number,'description',0,0,body,'body-'||number FROM source.issues;
        INSERT INTO passages(project,number,field,source,part,text,hash) SELECT project_id,issue_number,'comment',id,0,body,'comment-'||id FROM source.comments;
        INSERT INTO documents(project,number,generation,complete) SELECT project,number,CAST(generation AS TEXT),1 FROM source.issue_search_dirty;").unwrap();
    let mut compact = vec![0u8; 512];
    compact[0] = 127;
    compact.extend((1f32 / 127.).to_le_bytes());
    derived
        .execute(
            "INSERT INTO embeddings SELECT DISTINCT hash,? FROM passages",
            [compact],
        )
        .unwrap();
    let mut vector = vec![0u8; 512 * 4];
    vector[..4].copy_from_slice(&1f32.to_le_bytes());
    derived
        .execute(
            "INSERT OR REPLACE INTO queries VALUES('connection resume',?,0)",
            [vector],
        )
        .unwrap();
    derived
        .execute_batch("INSERT OR REPLACE INTO meta VALUES('backfill_done','1'); COMMIT;")
        .unwrap();
    owner
        .execute("UPDATE issue_search_dirty SET dirty=0", [])
        .unwrap();
    let passages: i64 = derived
        .query_row("SELECT count(*) FROM passages", [], |r| r.get(0))
        .unwrap();
    assert_eq!(passages, 108001);
    mode("ok");
    index(true);
    for semantic in [false, true] {
        let mut times = Vec::new();
        for _ in 0..5 {
            let t = Instant::now();
            let result =
                api(json!({"action":"search","query":"connection resume","semantic":semantic}));
            assert!(!result["issues"].as_array().unwrap().is_empty());
            assert_eq!(
                result["search"]["mode"],
                if semantic { "hybrid" } else { "keywords" }
            );
            times.push(ms(t));
        }
        let mut result = distribution(times);
        result["full_index"] = json!(passages);
        result["semantic"] = json!(semantic);
        println!("{result}");
    }
    index(false);
}
