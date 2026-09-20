//! Profile a private backup; never modify or print production issue contents.
//! cargo run --example profile_issues -- /absolute/path/to/issues.db
use hey_boss::issues::{self, Operation, Project, Request, Store};
use rusqlite::{Connection, OpenFlags};
use serde_json::{Value, json};
use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf, time::Instant};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn measure(mut operation: impl FnMut() -> Result<usize>) -> Result<Value> {
    operation()?;
    let mut times = Vec::new();
    let mut rows = 0;
    for _ in 0..25 {
        let start = Instant::now();
        rows = operation()?;
        times.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    times.sort_by(f64::total_cmp);
    Ok(json!({"median_ms":times[12],"p95_ms":times[23],"rows":rows}))
}

fn sql_case(db: &Connection, sql: &str, project: &str) -> Result<Value> {
    let timing = measure(|| {
        let mut statement = db.prepare(sql)?;
        let mut rows = statement.query([project])?;
        let mut count = 0;
        while rows.next()?.is_some() {
            count += 1;
        }
        Ok(count)
    })?;
    let plan = db
        .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))?
        .query_map([project], |row| row.get::<_, String>(3))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut statement = db.prepare(sql)?;
    {
        let mut rows = statement.query([project])?;
        while rows.next()?.is_some() {}
    }
    let steps = statement.get_status(rusqlite::StatementStatus::VmStep);
    Ok(json!({"timing":timing,"plan":plan,"vm_steps":steps}))
}

fn values(db: &Connection, sql: &str, parameter: &str) -> Result<Vec<Option<String>>> {
    Ok(db
        .prepare(sql)?
        .query_map([parameter], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?)
}

fn main() -> Result<()> {
    let source = match std::env::args_os().nth(1) {
        Some(path) => PathBuf::from(path),
        None => issues::database_path()?,
    };
    let root = std::env::temp_dir().join(format!("hey-boss-profile-{}", std::process::id()));
    fs::create_dir(&root)?;
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;
    let path = root.join("issues.db");
    let production = Connection::open_with_flags(&source, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    production.backup("main", &path, None)?;
    drop(production);
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
    let mut store = Store::open(&path).map_err(|error| error.message)?;
    let db = Connection::open(&path)?;
    let project = db.query_row("SELECT p.id,p.name FROM projects p JOIN issues i ON i.project_id=p.id GROUP BY p.id ORDER BY count(*) DESC LIMIT 1", [], |row| Ok(Project {id:row.get(0)?,name:row.get(1)?}))?;
    let number: i64 = db.query_row("SELECT number FROM issues WHERE project_id=?1 AND deleted_at IS NULL ORDER BY number LIMIT 1", [&project.id], |row| row.get(0))?;
    let mut report =
        json!({"sqlite_version":rusqlite::version(),"tables":{},"operations":{},"sql":{}});
    for table in [
        "issues",
        "projects",
        "agents",
        "worker_runs",
        "worker_events",
        "fleet_outbox",
    ] {
        let count: i64 = db.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
            row.get(0)
        })?;
        report["tables"][table] = json!(count);
    }
    report["operations"]["open_store"] = measure(|| {
        Store::open(&path).map_err(|error| error.message)?;
        Ok(1)
    })?;
    for (name, operation) in [
        (
            "projects",
            Operation::Projects {
                include_hidden: true,
            },
        ),
        (
            "issue_list",
            Operation::List {
                state: "all".into(),
                mine: false,
                unassigned: false,
                assignee: None,
                labels: vec![],
                search: None,
                limit: 50,
                offset: 0,
                all: false,
            },
        ),
        ("issue_view", Operation::View { number }),
        ("worker_status", Operation::Workers { worker_id: None }),
    ] {
        let request = Request {
            version: 1,
            project: project.clone(),
            project_override: None,
            actor: None,
            operation,
            request_id: None,
        };
        report["operations"][name] = measure(|| {
            let value = store.execute(&request).map_err(|error| error.message)?;
            Ok(value["issues"]
                .as_array()
                .or_else(|| value["projects"].as_array())
                .or_else(|| value["runs"].as_array())
                .map_or(1, Vec::len))
        })?;
    }
    for (name, sql) in [
        (
            "directory_discovery",
            "SELECT json_extract(metadata,'$.cwd') FROM agents WHERE EXISTS(SELECT 1 FROM issues i WHERE i.project_id=?1 AND (i.created_by=agents.id OR i.assignee=agents.id)) ORDER BY last_seen DESC LIMIT 50",
        ),
        (
            "directory_project_first",
            "SELECT json_extract(metadata,'$.cwd') FROM agents WHERE id IN(SELECT created_by FROM issues WHERE project_id=?1 UNION SELECT assignee FROM issues WHERE project_id=?1 AND assignee IS NOT NULL) ORDER BY last_seen DESC LIMIT 50",
        ),
        (
            "worker_history",
            "SELECT id FROM worker_runs WHERE worker_id=?1 AND finished_at IS NOT NULL ORDER BY started_at DESC,id DESC LIMIT 20",
        ),
    ] {
        let parameter = if name == "worker_history" {
            db.query_row("SELECT worker_id FROM worker_runs WHERE worker_id IS NOT NULL GROUP BY worker_id ORDER BY count(*) DESC LIMIT 1", [], |row| row.get::<_, String>(0))?
        } else {
            project.id.clone()
        };
        report["sql"][name] = sql_case(&db, sql, &parameter)?;
    }
    let history = Connection::open_in_memory()?;
    history.execute_batch("CREATE TABLE worker_runs(id TEXT PRIMARY KEY,worker_id TEXT,started_at INTEGER,finished_at INTEGER);
        CREATE INDEX worker_runs_worker ON worker_runs(worker_id,finished_at,started_at DESC);
        WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<50000)
        INSERT INTO worker_runs SELECT printf('run-%06d',x),'worker',x,(x*7919)%50000 FROM n;
        INSERT INTO worker_runs VALUES('active','worker',50001,NULL);")?;
    let old = "SELECT id FROM worker_runs r WHERE worker_id=?1 AND (finished_at IS NULL OR id IN(SELECT id FROM worker_runs WHERE worker_id=?1 AND finished_at IS NOT NULL ORDER BY started_at DESC,id DESC LIMIT 20)) ORDER BY finished_at IS NOT NULL,started_at DESC,id DESC";
    let new = "SELECT id FROM worker_runs r WHERE id IN(SELECT id FROM worker_runs WHERE worker_id=?1 AND finished_at IS NULL UNION ALL SELECT id FROM(SELECT id FROM worker_runs WHERE worker_id=?1 AND finished_at IS NOT NULL ORDER BY started_at DESC,id DESC LIMIT 20)) ORDER BY finished_at IS NOT NULL,started_at DESC,id DESC";
    let before = sql_case(&history, old, "worker")?;
    history.execute_batch("CREATE INDEX worker_finished_history ON worker_runs(worker_id,started_at DESC,id DESC) WHERE finished_at IS NOT NULL;")?;
    let after = sql_case(&history, new, "worker")?;
    if values(&history, old, "worker")? != values(&history, new, "worker")? {
        return Err("History query changed the selected attempts or their ordering".into());
    }
    report["synthetic_history_50000"] = json!({"before":before,"after":after});
    println!("{}", serde_json::to_string_pretty(&report)?);
    // Retain the private backup for comparative replay; only its path is shown.
    eprintln!("Private profiling backup: {}", path.display());
    Ok(())
}
