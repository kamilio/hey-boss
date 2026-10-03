//! Service-manager ownership is independent of runtime name, age and activity.
use super::*;

#[cfg(any(target_os = "macos", test))]
fn unavailable() -> io::Error {
    // Never include launchctl output: domain output can contain environment values.
    io::Error::other("Service ownership inspection unavailable or incomplete; processes preserved")
}

#[cfg(any(target_os = "macos", test))]
fn parse_domain(text: &str) -> io::Result<BTreeSet<u32>> {
    let mut count = None;
    let mut rows = 0;
    let mut inside = false;
    let mut complete = false;
    let mut roots = BTreeSet::new();
    for line in text.lines() {
        if let Some(value) = line.strip_prefix("\tservice count = ") {
            if count.is_some() {
                return Err(unavailable());
            }
            count = Some(value.parse::<usize>().map_err(|_| unavailable())?);
        } else if line == "\tservices = {" {
            if inside || complete {
                return Err(unavailable());
            }
            inside = true;
        } else if inside && line == "\t}" {
            inside = false;
            complete = true;
        } else if inside {
            let mut fields = line.split_whitespace();
            let pid: u32 = fields
                .next()
                .ok_or_else(unavailable)?
                .parse()
                .map_err(|_| unavailable())?;
            // launchd uses numeric exit codes and symbolic states such as (pe),
            // (cs) and (jt). Labels may contain spaces; neither field authorizes
            // cleanup, so keep them opaque and require only that they exist.
            fields.next().ok_or_else(unavailable)?;
            fields.next().ok_or_else(unavailable)?;
            rows += 1;
            if pid > 0 {
                roots.insert(pid);
            }
        }
    }
    if !complete || inside || count != Some(rows) {
        return Err(unavailable());
    }
    Ok(roots)
}

fn family(table: &Table, roots: &BTreeSet<u32>) -> BTreeSet<u32> {
    if roots.is_empty() {
        return BTreeSet::new();
    }
    // Include children even when the root exited between ps and launchctl.
    let mut children: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    for p in table.values() {
        children.entry(p.parent).or_default().push(p.pid);
    }
    let mut protected = roots.clone();
    let mut pending: Vec<_> = roots.iter().copied().collect();
    while let Some(pid) = pending.pop() {
        for child in children.get(&pid).into_iter().flatten() {
            if protected.insert(*child) {
                pending.push(*child);
            }
        }
    }
    protected
}

#[cfg(any(target_os = "macos", test))]
fn inventory_with(
    uid: u32,
    mut query: impl FnMut(&str) -> io::Result<String>,
) -> io::Result<BTreeSet<u32>> {
    let mut roots = BTreeSet::new();
    for domain in [format!("gui/{uid}"), format!("user/{uid}"), "system".into()] {
        let text = query(&domain).map_err(|_| unavailable())?;
        roots.extend(parse_domain(&text)?);
    }
    Ok(roots)
}

pub(super) struct Guard {
    table: Table,
    protected: BTreeSet<u32>,
}

pub(super) fn protection(
    guard: &io::Result<Guard>,
    p: &Process,
) -> Option<super::super::workload_ownership::Protection> {
    match guard {
        Ok(guard) => {
            guard
                .protection(p)
                .map(|reason| super::super::workload_ownership::Protection {
                    reason: reason.into(),
                    uncertain: false,
                })
        }
        Err(_) => Some(super::super::workload_ownership::Protection {
            reason: "Service ownership inspection unavailable or incomplete; processes preserved"
                .into(),
            uncertain: true,
        }),
    }
}

impl Guard {
    fn new(table: Table, roots: BTreeSet<u32>) -> Self {
        let protected = family(&table, &roots);
        Self { table, protected }
    }

    pub(super) fn read(table: &Table) -> io::Result<Self> {
        #[cfg(target_os = "macos")]
        let roots = inventory_with(unsafe { libc::geteuid() }, |domain| {
            let result = super::super::output_with_limit(
                Command::new("/bin/launchctl").args(["print", domain]),
                Duration::from_secs(5),
                8 * 1024 * 1024,
            )
            .map_err(|_| unavailable())?;
            if !result.status.success() || !result.stderr.is_empty() {
                return Err(unavailable());
            }
            String::from_utf8(result.stdout).map_err(|_| unavailable())
        })?;
        #[cfg(not(target_os = "macos"))]
        let roots = BTreeSet::new();
        Ok(Self::new(table.clone(), roots))
    }

    pub(super) fn refresh(table: &Table) -> io::Result<Self> {
        #[cfg(target_os = "macos")]
        {
            let _ = table;
            Self::read(&inventory().map_err(|_| unavailable())?)
        }
        #[cfg(not(target_os = "macos"))]
        Self::read(table)
    }

    pub(super) fn protection(&self, process: &Process) -> Option<&'static str> {
        if self.protected.contains(&process.pid) {
            return Some("Managed service or descendant; preserved");
        }
        if !self.table.get(&process.pid).is_some_and(|fresh| {
            fresh.identity == process.identity
                && fresh.executable == process.executable
                && fresh.uid == process.uid
                && fresh.parent == process.parent
        }) {
            return Some("Process identity or ancestry changed; preserved");
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn process(pid: u32, parent: u32, name: &str) -> Process {
        Process {
            pid,
            parent,
            uid: unsafe { libc::geteuid() },
            age_seconds: 90000,
            cpu_seconds: 0.0,
            executable: format!("/fixture/{name}"),
            identity: format!("start-{pid}"),
            arguments: String::new(),
        }
    }

    #[test]
    fn domain_inventory_reads_only_complete_service_rows() {
        let text = "gui/501 = {\n\tservice count = 3\n\tenvironment = {\n\t\tSECRET => never expose this\n\t}\n\tservices = {\n\t\t101 - adapter\n\t\t102 (pe) worker\n\t\t0 0 inactive\n\t}\n}\n";
        assert_eq!(parse_domain(text).unwrap(), BTreeSet::from([101, 102]));
        for status in ["(pe)", "(cs)", "(jt)", "-15", "0", "-"] {
            let text = format!(
                "\tservice count = 1\n\tservices = {{\n\t\t101 {status} service with spaces\n\t}}\n"
            );
            assert_eq!(parse_domain(&text).unwrap(), BTreeSet::from([101]));
        }
        for invalid in [
            "",
            "\tservices = {\n",
            "\tservice count = 1\n\tservices = {\n\t}\n",
            "\tservice count = 1\n\tservices = {\n\t\tunknown - adapter\n\t}\n",
        ] {
            assert!(parse_domain(invalid).is_err(), "{invalid:?}");
        }
        assert_eq!(
            parse_domain("\tservice count = 0\n\tservices = {\n\t}\n").unwrap(),
            BTreeSet::new()
        );
    }

    #[test]
    fn managed_runtime_families_survive_every_age_and_pressure() {
        for name in ["python", "python3.14", "node", "bun"] {
            for age in [3601, 90000] {
                let mut root = process(101, 1, name);
                root.age_seconds = age;
                let table = BTreeMap::from([
                    (101, root),
                    (102, process(102, 101, "node")),
                    (103, process(103, 102, "bun")),
                    (104, process(104, 1, name)),
                ]);
                for pressure in ["Normal", "Warning", "Critical"] {
                    let managed = Table::from_iter(
                        table
                            .iter()
                            .filter(|(pid, _)| **pid != 104)
                            .map(|(pid, p)| (*pid, p.clone())),
                    );
                    let (items, count) = aggressive_expiration(
                        &managed,
                        pressure,
                        true,
                        |table, _| Ok(Guard::new(table.clone(), BTreeSet::from([101]))),
                        |_, _| panic!("managed service signaled: {name} {age} {pressure}"),
                    )
                    .unwrap();
                    assert_eq!(count, 0);
                    assert_eq!(items.len(), 3);
                    assert!(
                        items
                            .iter()
                            .all(|item| !item.eligible && item.detail.contains("Managed service"))
                    );
                }
            }
        }
    }

    #[test]
    fn new_ownership_at_either_signal_boundary_blocks_the_signal() {
        // Nonexistent PIDs avoid consulting unrelated live workloads. The injected
        // sender records the real expiration path's decisions without OS signals.
        let table = BTreeMap::from([
            (4_000_001, process(4_000_001, 1, "python")),
            (4_000_002, process(4_000_002, 4_000_001, "bun")),
        ]);
        for owned_at in 1..=2 {
            let mut sent = Vec::new();
            let mut inspections = 0;
            aggressive_expiration(
                &table,
                "Critical",
                true,
                |table, refresh| {
                    let stage = inspections;
                    inspections += 1;
                    assert_eq!(refresh, stage > 0);
                    Ok(Guard::new(
                        table.clone(),
                        if stage >= owned_at {
                            BTreeSet::from([4_000_001])
                        } else {
                            BTreeSet::new()
                        },
                    ))
                },
                |p, kind| {
                    sent.push((p.pid, kind));
                    Ok(true)
                },
            )
            .unwrap();
            assert_eq!(inspections, owned_at + 1);
            assert_eq!(sent.len(), if owned_at == 1 { 0 } else { 2 });
            assert!(sent.iter().all(|(_, kind)| *kind == libc::SIGTERM));
        }
    }

    #[test]
    fn failed_service_inspection_at_each_stage_never_authorizes_a_signal() {
        let table = BTreeMap::from([(4_000_003, process(4_000_003, 1, "python"))]);
        for failed_at in 0..=2 {
            let mut inspections = 0;
            let mut sent = Vec::new();
            let result = aggressive_expiration(
                &table,
                "Normal",
                true,
                |table, _| {
                    let stage = inspections;
                    inspections += 1;
                    if stage == failed_at {
                        Err(unavailable())
                    } else {
                        Ok(Guard::new(table.clone(), BTreeSet::new()))
                    }
                },
                |_, kind| {
                    sent.push(kind);
                    Ok(true)
                },
            );
            assert_eq!(result.is_err(), failed_at == 0);
            assert_eq!(
                sent,
                if failed_at == 2 {
                    vec![libc::SIGTERM]
                } else {
                    vec![]
                }
            );
        }
    }

    #[test]
    fn unmanaged_orphan_still_reaches_term_and_kill_with_bounded_inspections() {
        let table = BTreeMap::from([(4_000_004, process(4_000_004, 1, "node"))]);
        let mut inspections = 0;
        let mut sent = Vec::new();
        aggressive_expiration(
            &table,
            "Normal",
            true,
            |table, _| {
                inspections += 1;
                Ok(Guard::new(table.clone(), BTreeSet::new()))
            },
            |_, kind| {
                sent.push(kind);
                Ok(true)
            },
        )
        .unwrap();
        assert_eq!(inspections, 3);
        assert_eq!(sent, [libc::SIGTERM, libc::SIGKILL]);
    }

    #[test]
    fn identity_or_parent_changes_cannot_authorize_signals() {
        let old = process(101, 1, "node");
        for change in 0..4 {
            let mut fresh = old.clone();
            match change {
                0 => fresh.identity = "reused".into(),
                1 => fresh.parent = 999,
                2 => fresh.executable = "/new/node".into(),
                _ => fresh.uid += 1,
            }
            let guard = Guard::new(BTreeMap::from([(101, fresh)]), BTreeSet::new());
            assert!(guard.protection(&old).is_some());
        }
        assert!(
            Guard::new(Table::new(), BTreeSet::new())
                .protection(&old)
                .is_some()
        );
        assert!(
            Guard::new(BTreeMap::from([(101, old.clone())]), BTreeSet::new())
                .protection(&old)
                .is_none()
        );
    }

    #[test]
    fn three_bounded_queries_cover_agents_and_daemons_and_fail_closed() {
        let mut domains = Vec::new();
        let roots = inventory_with(501, |domain| {
            domains.push(domain.to_owned());
            Ok(format!(
                "\tservice count = 1\n\tservices = {{\n\t\t{} - fixture\n\t}}\n",
                domains.len() + 100
            ))
        })
        .unwrap();
        assert_eq!(domains, ["gui/501", "user/501", "system"]);
        assert_eq!(roots, BTreeSet::from([101, 102, 103]));
        for failed in ["gui/501", "user/501", "system"] {
            let error = inventory_with(501, |domain| {
                if domain == failed {
                    Err(io::Error::other("SECRET"))
                } else {
                    Ok("\tservice count = 0\n\tservices = {\n\t}\n".into())
                }
            })
            .unwrap_err();
            assert!(!error.to_string().contains("SECRET"));
        }
    }
}
