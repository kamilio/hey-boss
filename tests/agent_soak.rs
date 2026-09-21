//! Manual sustained-connection check. Uses six small model requests total;
//! intermediate inspection exercises the actual transports without model work.
use hey_boss::agent_runtime::{AgentSession, Event, Launch, Provider, TurnStatus};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    process::Command,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

fn reply(agent: &mut AgentSession, token: &str) {
    agent
        .prompt(&format!("Reply with only {token}. Do not use tools."), None)
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        assert!(
            Instant::now() < deadline,
            "Reply timed out: {:?}",
            agent.state()
        );
        if let Some(event) = agent.receive(Duration::from_millis(100)).unwrap() {
            match event {
                Event::TurnCompleted { status, output, .. } => {
                    assert_eq!(status, TurnStatus::Completed);
                    assert!(output.contains(token), "{output}");
                    return;
                }
                Event::ToolStarted { .. } | Event::Approval { .. } | Event::Input { .. } => {
                    panic!("Unexpected tool/input in text-only soak: {event:?}")
                }
                _ => {}
            }
        }
    }
}

#[test]
#[ignore = "one-hour real authenticated Codex/Claude/Pi connection soak; six model requests"]
fn real_idle_sessions_remain_responsive_for_one_hour() {
    let root: PathBuf = std::env::temp_dir().join(format!(
        "hey-boss-agent-soak-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let mut agents = Vec::new();
    for provider in [Provider::Codex, Provider::Claude, Provider::Pi] {
        let cwd = root.join(provider.name());
        std::fs::create_dir_all(&cwd).unwrap();
        let mut agent = AgentSession::launch(Launch {
            provider,
            binary: None,
            cwd,
            resume: None,
            env: BTreeMap::new(),
            output_schema: None,
        })
        .unwrap();
        reply(&mut agent, "SOAK_READY");
        let reference = agent.inspect().unwrap().session.unwrap();
        agents.push((agent, reference));
    }
    let pids = agents
        .iter()
        .map(|(agent, _)| agent.pid().to_string())
        .collect::<Vec<_>>()
        .join(",");
    let started = Instant::now();
    let mut report = started;
    let mut inspections = 0_u64;
    while started.elapsed() < Duration::from_secs(60 * 60) {
        for (agent, reference) in &mut agents {
            let state = agent.inspect().unwrap();
            assert_eq!(state.session.as_ref(), Some(&*reference));
            assert!(state.turn.is_none() && !state.stopped && !state.outcome_uncertain);
            while let Some(event) = agent.receive(Duration::ZERO).unwrap() {
                assert!(
                    !matches!(
                        event,
                        Event::TurnStarted { .. }
                            | Event::TurnCompleted { .. }
                            | Event::ToolStarted { .. }
                            | Event::Approval { .. }
                            | Event::Input { .. }
                    ),
                    "Unexpected idle work: {event:?}"
                );
            }
            inspections += 1;
        }
        if Instant::now() >= report {
            let usage = Command::new("ps")
                .args(["-o", "pid=,rss=,pcpu=", "-p", &pids])
                .output()
                .unwrap();
            assert!(usage.status.success());
            eprintln!(
                "SOAK elapsed={}s inspections={inspections}; PID / RSS KiB / CPU %\n{}",
                started.elapsed().as_secs(),
                String::from_utf8_lossy(&usage.stdout).trim()
            );
            report = Instant::now() + Duration::from_secs(60);
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    for (mut agent, reference) in agents {
        reply(&mut agent, "SOAK_RECOVERED");
        assert_eq!(agent.inspect().unwrap().session.as_ref(), Some(&reference));
        agent.stop().unwrap();
        eprintln!(
            "PASS {} responsive after one hour",
            reference.provider.name()
        );
    }
    eprintln!(
        "PASS one-hour soak; {inspections} inspections; scratch {}",
        root.display()
    );
}
