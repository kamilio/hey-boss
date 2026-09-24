//! Worker dashboard components. Rendering never performs IO or controls workers.
pub mod backend;
pub mod diagnostics;
pub mod runtime;
mod terminal_name;
pub mod ui;

use serde_json::Value;
use std::collections::BTreeSet;

/// Dedicated projects precede the lower-priority pool of shared workers.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum DashboardTab {
    Project(String),
    Shared,
}

fn worker_projects(worker: &Value) -> BTreeSet<&str> {
    worker["config"]["projects"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect()
}

fn worker_tab(worker: &Value) -> DashboardTab {
    let projects = worker_projects(worker);
    if projects.len() == 1 {
        DashboardTab::Project(projects.first().unwrap().to_string())
    } else {
        DashboardTab::Shared
    }
}

/// Strip terminal controls, including ANSI and bidi overrides, from queue text.
pub fn text(value: &Value) -> String {
    value
        .as_str()
        .unwrap_or_default()
        .chars()
        .filter(|c| {
            !c.is_control() && !matches!(*c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        })
        .take(8192)
        .collect()
}

/// User-facing project identity; storage keys remain available in JSON.
pub fn project_name(id: &str, snapshot: &Value) -> String {
    if let Some(project) = snapshot["projects"]
        .as_array()
        .and_then(|projects| projects.iter().find(|project| project["id"] == id))
    {
        return text(&project["name"]);
    }
    let name = id
        .strip_prefix("named:")
        .unwrap_or_else(|| id.rsplit('/').next().unwrap_or(id));
    text(&Value::String(name.into()))
}

#[derive(Default)]
pub struct Dashboard {
    pub snapshot: Value,
    pub owned_worker: bool,
    pub worker_id: Option<String>,
    pub selected_tab: Option<DashboardTab>,
    pub manage_workers: bool,
    pub managed_worker_id: Option<String>,
    pub add_worker: Option<AddWorker>,
    pub run_id: Option<String>,
    pub history: bool,
    pub help: bool,
    pub pending: bool,
    pub error: Option<String>,
    pub diagnostic: Option<String>,
    pub confirmation: Option<Confirmation>,
    pub detail_scroll: u16,
    /// Current wall-clock milliseconds, supplied by the application for live timers.
    pub now_ms: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Confirmation {
    pub worker_id: String,
    pub name: String,
    pub stop: bool,
    pub graceful: bool,
}

pub struct AddWorker {
    pub id: String,
    pub fields: Vec<String>,
    pub selected: usize,
    pub error: Option<String>,
}
impl Default for AddWorker {
    fn default() -> Self {
        Self {
            id: format!(
                "auto-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            ),
            fields: vec![String::new(), "1".into(), String::new()],
            selected: 0,
            error: None,
        }
    }
}
impl AddWorker {
    pub fn request(&self) -> Result<backend::Request, String> {
        let concurrency = self.fields[1]
            .parse::<u32>()
            .ok()
            .filter(|n| (1..=1024).contains(n))
            .ok_or("Slots must be between 1 and 1024")?;
        if self.fields[0].trim().is_empty() {
            return Err("Enter a worker name".into());
        }
        if self.fields[2..].iter().any(|s| s.trim().is_empty()) {
            return Err("Enter a directory for each checkout".into());
        }
        Ok(backend::Request::AddWorker {
            id: self.id.clone(),
            name: self.fields[0].clone(),
            concurrency,
            directories: self.fields[2..].to_vec(),
        })
    }
}

impl Dashboard {
    pub fn project_tabs(&self) -> bool {
        self.snapshot["project_tabs"] == true
    }

    pub fn tabs(&self) -> Vec<DashboardTab> {
        self.workers()
            .into_iter()
            .map(worker_tab)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    pub fn tab_label(&self, tab: &DashboardTab) -> String {
        match tab {
            DashboardTab::Project(id) => project_name(id, &self.snapshot),
            DashboardTab::Shared => {
                let workers: Vec<_> = self
                    .workers()
                    .into_iter()
                    .filter(|w| worker_tab(w) == DashboardTab::Shared)
                    .collect();
                let projects: BTreeSet<_> =
                    workers.iter().flat_map(|w| worker_projects(w)).collect();
                let scope = if workers.iter().any(|w| worker_projects(w).is_empty()) {
                    "all projects".to_owned()
                } else {
                    format!("{} projects", projects.len())
                };
                format!(
                    "Shared · {} worker{} · {scope}",
                    workers.len(),
                    if workers.len() == 1 { "" } else { "s" }
                )
            }
        }
    }

    pub fn switch_project(&mut self, delta: isize) {
        let ids = self.tabs();
        if ids.is_empty() {
            return;
        }
        let at = ids
            .iter()
            .position(|id| Some(id) == self.selected_tab.as_ref())
            .unwrap_or(0);
        self.selected_tab =
            Some(ids[(at as isize + delta).rem_euclid(ids.len() as isize) as usize].clone());
        self.run_id = None;
        self.detail_scroll = 0;
        self.normalize_run();
        self.navigate_worker(0);
    }

    pub fn workers(&self) -> Vec<&Value> {
        self.snapshot["workers"]
            .as_array()
            .map(|a| a.iter().collect())
            .unwrap_or_default()
    }

    pub fn project_workers(&self) -> Vec<&Value> {
        self.workers()
            .into_iter()
            .filter(|w| Some(&worker_tab(w)) == self.selected_tab.as_ref())
            .collect()
    }

    pub fn navigate_worker(&mut self, delta: isize) {
        let ids: Vec<_> = self
            .project_workers()
            .iter()
            .filter_map(|w| w["id"].as_str().map(str::to_owned))
            .collect();
        if ids.is_empty() {
            self.managed_worker_id = None;
            return;
        }
        let at = ids
            .iter()
            .position(|id| Some(id) == self.managed_worker_id.as_ref())
            .unwrap_or(0);
        self.managed_worker_id =
            Some(ids[at.saturating_add_signed(delta).min(ids.len() - 1)].clone());
    }

    pub fn confirm_remove(&mut self) {
        if self.pending || self.error.is_some() {
            return;
        }
        if let Some(worker) = self
            .project_workers()
            .into_iter()
            .find(|w| w["id"].as_str() == self.managed_worker_id.as_deref())
        {
            self.confirmation = Some(Confirmation {
                worker_id: text(&worker["id"]),
                name: text(&worker["config"]["name"]),
                stop: false,
                graceful: true,
            });
        }
    }

    pub fn runs(&self) -> Vec<&Value> {
        if self.project_tabs() {
            return self
                .project_workers()
                .into_iter()
                .flat_map(|worker| {
                    ["runs", "chiefs"]
                        .into_iter()
                        .flat_map(move |key| worker[key].as_array().into_iter().flatten())
                })
                .filter(|run| {
                    (run["finished_at"].is_null() || run["retry_at"].is_i64()) != self.history
                })
                .collect();
        }
        // Never display another worker's sessions while its refresh is in flight.
        if self.snapshot["worker_id"].as_str() != self.worker_id.as_deref() {
            return vec![];
        }
        ["runs", "chiefs"]
            .into_iter()
            .filter_map(|key| self.snapshot[key].as_array())
            .flatten()
            .filter(|r| (r["finished_at"].is_null() || r["retry_at"].is_i64()) != self.history)
            .collect()
    }

    pub fn apply(&mut self, snapshot: Value) {
        self.snapshot = snapshot;
        if self.project_tabs() {
            let tabs = self.tabs();
            if !tabs
                .iter()
                .any(|tab| Some(tab) == self.selected_tab.as_ref())
            {
                self.selected_tab = tabs.into_iter().next();
            }
        }
        let workers = self.workers();
        if !self.owned_worker
            && !workers
                .iter()
                .any(|w| w["id"].as_str() == self.worker_id.as_deref())
        {
            self.worker_id = self.snapshot["worker_id"]
                .as_str()
                .or_else(|| workers.first().and_then(|w| w["id"].as_str()))
                .map(str::to_owned);
        }
        self.normalize_run();
        self.navigate_worker(0);
        self.error = None;
    }

    pub fn normalize_run(&mut self) {
        let runs = self.runs();
        if !runs
            .iter()
            .any(|r| r["id"].as_str() == self.run_id.as_deref())
        {
            self.run_id = runs
                .first()
                .and_then(|r| r["id"].as_str())
                .map(str::to_owned);
            self.detail_scroll = 0;
        }
        self.select_run_worker();
    }

    fn select_run_worker(&mut self) {
        if !self.project_tabs() {
            return;
        }
        self.worker_id = self
            .project_workers()
            .into_iter()
            .find(|w| {
                ["runs", "chiefs"]
                    .iter()
                    .flat_map(|key| w[*key].as_array().into_iter().flatten())
                    .any(|r| r["id"].as_str() == self.run_id.as_deref())
            })
            .or_else(|| self.project_workers().into_iter().next())
            .and_then(|w| w["id"].as_str())
            .map(str::to_owned);
    }

    pub fn navigate(&mut self, delta: isize) {
        let ids: Vec<String> = self
            .runs()
            .iter()
            .filter_map(|v| v["id"].as_str().map(str::to_owned))
            .collect();
        let selected = &mut self.run_id;
        if ids.is_empty() {
            return;
        }
        let at = ids
            .iter()
            .position(|id| Some(id) == selected.as_ref())
            .unwrap_or(0);
        let next = at.saturating_add_signed(delta).min(ids.len() - 1);
        *selected = Some(ids[next].clone());
        self.detail_scroll = 0;
        self.select_run_worker();
    }

    pub fn confirm(&mut self, stop: bool) {
        if self.pending || self.error.is_some() {
            return;
        }
        if let Some(w) = self
            .workers()
            .into_iter()
            .find(|w| w["id"].as_str() == self.worker_id.as_deref())
        {
            self.confirmation = Some(Confirmation {
                worker_id: w["id"].as_str().unwrap_or_default().into(),
                name: text(&w["config"]["name"]),
                stop,
                graceful: false,
            });
        }
    }
}
