use super::{Project, identity};
use crate::agents::Snapshot;
use std::collections::BTreeMap;
use std::path::Path;

pub(super) fn projects(snapshot: &Snapshot, machine: &str) -> Vec<(Project, i64)> {
    let mut projects = BTreeMap::<String, (Project, i64)>::new();
    for agent in &snapshot.agents {
        if agent
            .cwd
            .as_deref()
            .is_some_and(|cwd| identity::is_git_metadata_path(Path::new(cwd)))
        {
            continue;
        }
        let project = if let Some(git) = &agent.git {
            identity::project_from_git(git, machine)
        } else if let Some(cwd) = &agent.cwd {
            let Ok(project) = identity::project(Path::new(cwd), machine) else {
                continue;
            };
            project
        } else {
            continue;
        };
        if identity::is_home_project(&project)
            || identity::is_temporary_project(&project.id)
            || identity::is_git_metadata_project(&project.id)
        {
            continue;
        }
        let at = agent
            .activity_at
            .into_iter()
            .chain(agent.updated_at)
            .max()
            .unwrap_or(0)
            .min(snapshot.observed_at)
            .saturating_mul(1000)
            .min(i64::MAX as u64) as i64;
        projects
            .entry(project.id.clone())
            .and_modify(|entry| entry.1 = entry.1.max(at))
            .or_insert((project, at));
    }
    projects.into_values().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::{Agent, GitInfo};
    #[test]
    fn metadata_cwds_are_ignored_even_with_cached_git_information() {
        for git in [
            None,
            Some(GitInfo {
                repository_root: "/workspace/hey-gh".into(),
                common_dir: "/workspace/hey-gh/.git".into(),
                worktree: "/workspace/hey-gh".into(),
                branch: None,
                repository_id: "github.com/example/hey-gh".into(),
                origin: Some("github.com/example/hey-gh".into()),
            }),
        ] {
            for cwd in ["/workspace/hey-gh/.git", "/workspace/hey-gh/.git/objects"] {
                let agent = Agent {
                    id: "metadata".into(),
                    pid: 42,
                    kind: "Codex".into(),
                    cwd: Some(cwd.into()),
                    session_id: None,
                    task: None,
                    title: None,
                    activity: None,
                    state: "Working".into(),
                    updated_at: Some(10),
                    evidence: String::new(),
                    update: None,
                    activity_at: Some(20),
                    git: git.clone(),
                };
                let snapshot = Snapshot {
                    host: "local".into(),
                    observed_at: 50,
                    agents: vec![agent],
                    warnings: vec![],
                };
                assert!(projects(&snapshot, "machine").is_empty());
            }
        }
    }
    #[test]
    fn worktrees_share_identity_and_polling_does_not_manufacture_activity() {
        let git = GitInfo {
            repository_root: "/repo".into(),
            common_dir: "/repo/.git".into(),
            worktree: "/repo".into(),
            branch: None,
            repository_id: "github.com/example/repo".into(),
            origin: Some("github.com/example/repo".into()),
        };
        let agent = Agent {
            id: "session".into(),
            pid: 42,
            kind: "Codex".into(),
            cwd: Some("/repo".into()),
            session_id: None,
            task: None,
            title: None,
            activity: None,
            state: "Working".into(),
            updated_at: Some(10),
            evidence: String::new(),
            update: None,
            activity_at: Some(20),
            git: Some(git.clone()),
        };
        let mut second = agent.clone();
        second.git.as_mut().unwrap().worktree = "/worktree".into();
        second.activity_at = Some(30);
        let mut snapshot = Snapshot {
            host: "local".into(),
            observed_at: 50,
            agents: vec![agent, second],
            warnings: vec![],
        };
        let found = projects(&snapshot, "machine");
        assert_eq!(
            found,
            vec![(
                Project {
                    id: "github.com/example/repo".into(),
                    name: "repo".into()
                },
                30000
            )]
        );
        snapshot.observed_at = 500;
        assert_eq!(projects(&snapshot, "machine"), found);
        for agent in &mut snapshot.agents {
            agent.git.as_mut().unwrap().origin = None;
        }
        let local = projects(&snapshot, "machine");
        assert_eq!(local.len(), 1);
        assert_eq!(local[0].0.id, "local:machine:/repo");
        assert!(!identity::is_git_metadata_project(&local[0].0.id));
    }
}
