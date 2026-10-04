//! Durable alternation between changed heads and the ordinary account rotation.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

type Key = (String, u64);

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Head {
    node: Option<String>,
    sha: Option<String>,
}

impl Head {
    fn from_node(node: &Value) -> Self {
        Self {
            node: node["id"].as_str().map(str::to_owned),
            sha: node["headRefOid"].as_str().map(str::to_owned),
        }
    }
}

#[derive(Serialize, Deserialize)]
pub(super) struct Schedule {
    // JSON object keys cannot represent repository/number tuples.
    seen: Vec<(Key, Head)>,
    urgent: VecDeque<Key>,
    #[serde(default)]
    resume: VecDeque<Key>,
    pub next: Option<Key>,
    prefer_urgent: bool,
}

pub(super) struct Work {
    pub key: Key,
    pub node: Value,
    urgent: bool,
    at_normal_front: bool,
    successor: Key,
}

impl Schedule {
    pub fn baseline(nodes: &BTreeMap<Key, Value>, next: Option<Key>) -> Self {
        Self {
            seen: nodes
                .iter()
                .map(|(key, node)| (key.clone(), Head::from_node(node)))
                .collect(),
            urgent: VecDeque::new(),
            resume: VecDeque::new(),
            next,
            prefer_urgent: true,
        }
    }

    pub fn reconcile(&mut self, nodes: &BTreeMap<Key, Value>) {
        let seen: BTreeMap<_, _> = self.seen.iter().cloned().collect();
        self.urgent.retain(|key| nodes.contains_key(key));
        self.resume.retain(|key| {
            nodes
                .get(key)
                .is_some_and(|node| seen.get(key) == Some(&Head::from_node(node)))
        });
        let queued: BTreeSet<_> = self.urgent.iter().cloned().collect();
        self.seen = nodes
            .iter()
            .map(|(key, node)| {
                let head = Head::from_node(node);
                if seen.get(key) != Some(&head) && !queued.contains(key) {
                    self.urgent.push_back(key.clone());
                }
                (key.clone(), head)
            })
            .collect();
    }

    pub fn order(&self, mut nodes: BTreeMap<Key, Value>) -> Vec<Work> {
        let mut normal: Vec<_> = nodes.keys().cloned().collect();
        let successors: BTreeMap<_, _> = normal
            .iter()
            .zip(normal.iter().cycle().skip(1))
            .take(normal.len())
            .map(|(key, next)| (key.clone(), next.clone()))
            .collect();
        if let Some(next) = &self.next {
            // A closed PR can disappear while it is the saved cursor.
            let index = normal.partition_point(|key| key < next);
            if index < normal.len() {
                normal.rotate_left(index);
            }
        }
        let mut normal: VecDeque<_> = normal.into();
        let mut urgent: VecDeque<_> = self.resume.iter().chain(&self.urgent).cloned().collect();
        let mut prefer_urgent = self.prefer_urgent;
        let mut work = Vec::with_capacity(nodes.len());
        while !nodes.is_empty() {
            while normal.front().is_some_and(|key| !nodes.contains_key(key)) {
                normal.pop_front();
            }
            while urgent.front().is_some_and(|key| !nodes.contains_key(key)) {
                urgent.pop_front();
            }
            let priority = (prefer_urgent || normal.is_empty()) && !urgent.is_empty();
            let key = if priority {
                urgent.pop_front()
            } else {
                normal.pop_front()
            }
            .expect("normal queue covers remaining work");
            let node = nodes.remove(&key).expect("queued node exists");
            let successor = successors[&key].clone();
            let at_normal_front = normal.front() == Some(&key);
            work.push(Work {
                key,
                node,
                urgent: priority,
                at_normal_front,
                successor,
            });
            prefer_urgent = !priority || at_normal_front;
        }
        work
    }

    // Save before starting I/O. An aborted cycle cannot repeatedly consume the
    // same lane. Priority cannot skip earlier ordinary work, but if both lanes
    // point at this PR it must advance both to avoid repeating a stalled read.
    pub fn started(&mut self, work: &Work) -> bool {
        let continuing = self.resume.contains(&work.key);
        self.resume.retain(|key| key != &work.key);
        self.prefer_urgent = !work.urgent || work.at_normal_front;
        if !work.urgent || work.at_normal_front {
            self.next = Some(work.successor.clone());
        }
        if let Some(index) = self.urgent.iter().position(|key| key == &work.key) {
            let key = self.urgent.remove(index).unwrap();
            self.urgent.push_back(key);
        }
        continuing
    }

    pub fn succeeded(&mut self, key: &Key) {
        self.urgent.retain(|queued| queued != key);
        self.resume.retain(|queued| queued != key);
    }

    // A newly retained completed job page lets the next cycle continue actual
    // progress. This spends the existing priority turn, never the ordinary
    // turn. The continuation is consumed before I/O and needs another newly
    // retained page to earn another turn.
    pub fn progressed(&mut self, key: &Key) {
        if !self.resume.contains(key) {
            self.resume.push_back(key.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn key(number: u64) -> Key {
        ("acme/demo".into(), number)
    }
    fn nodes(numbers: &[u64]) -> BTreeMap<Key, Value> {
        numbers
            .iter()
            .map(|number| {
                (
                    key(*number),
                    json!({"id":format!("PR_{number}"),"headRefOid":"old"}),
                )
            })
            .collect()
    }
    fn restart(schedule: Schedule) -> Schedule {
        serde_json::from_value(serde_json::to_value(schedule).unwrap()).unwrap()
    }

    #[test]
    fn new_heads_alternate_with_old_work_without_duplicates() {
        let mut current = nodes(&[1, 2, 3, 4]);
        let mut schedule = Schedule::baseline(&current, Some(key(2)));
        current.get_mut(&key(4)).unwrap()["headRefOid"] = json!("changed");
        current.extend(nodes(&[5]));
        schedule.reconcile(&current);
        let work = schedule.order(current);
        assert_eq!(
            work.iter().map(|work| work.key.1).collect::<Vec<_>>(),
            vec![4, 2, 5, 3, 1]
        );
    }

    #[test]
    fn interrupted_priority_and_ordinary_progress_survive_restarts_and_new_arrivals() {
        let mut current = nodes(&[1, 2, 3]);
        let mut schedule = Schedule::baseline(&current, None);
        for number in 4..10 {
            current.extend(nodes(&[number]));
            schedule.reconcile(&current);
            let work = schedule.order(current.clone());
            let work = &work[0];
            if number % 2 == 0 {
                assert!(work.urgent);
            } else {
                assert!(!work.urgent);
                assert_eq!(work.key.1, (number - 3) / 2);
            }
            // Simulate process death during every request. Neither unfinished
            // priority work nor the ordinary lane's progress may disappear.
            schedule.started(work);
            schedule = restart(schedule);
        }
        assert!(schedule.urgent.contains(&key(4)));
        assert_eq!(schedule.next, Some(key(4)));
    }

    #[test]
    fn priority_completion_is_head_specific_and_metadata_does_not_requeue_it() {
        let mut current = nodes(&[1, 2]);
        let mut schedule = Schedule::baseline(&current, None);
        current.get_mut(&key(2)).unwrap()["headRefOid"] = json!("changed");
        schedule.reconcile(&current);
        let work = schedule.order(current.clone());
        schedule.started(&work[0]);
        schedule.succeeded(&key(2));
        schedule = restart(schedule);
        current.get_mut(&key(2)).unwrap()["updatedAt"] = json!("later");
        schedule.reconcile(&current);
        assert!(schedule.urgent.is_empty());
        current.get_mut(&key(2)).unwrap()["headRefOid"] = json!("changed-again");
        schedule.reconcile(&current);
        assert_eq!(schedule.urgent, VecDeque::from([key(2)]));
        current.remove(&key(2));
        schedule.reconcile(&current);
        assert!(schedule.urgent.is_empty());
        assert_eq!(schedule.seen.len(), 1);
    }

    #[test]
    fn interrupted_priority_at_the_normal_cursor_does_not_repeat_before_other_work() {
        let current = nodes(&[1, 2]);
        let mut schedule = Schedule::baseline(&BTreeMap::new(), None);
        schedule.reconcile(&current);
        let work = schedule.order(current.clone());
        assert_eq!(work[0].key, key(1));
        schedule.started(&work[0]);
        let schedule = restart(schedule);
        let work = schedule.order(current);
        assert_eq!(work[0].key, key(2));
        assert!(work[0].at_normal_front);
    }

    #[test]
    fn disappearing_cursor_keeps_its_place_and_exhausted_priority_uses_normal_work() {
        let current = nodes(&[1, 3, 4]);
        let schedule = Schedule::baseline(&current, Some(key(2)));
        let work = schedule.order(current);
        assert_eq!(
            work.iter().map(|work| work.key.1).collect::<Vec<_>>(),
            vec![3, 4, 1]
        );
        assert!(work.iter().all(|work| !work.urgent));
    }

    #[test]
    fn retained_progress_resumes_once_and_cannot_take_the_ordinary_turn() {
        let current = nodes(&[1, 2, 3, 4, 5, 6]);
        let mut schedule = Schedule::baseline(&current, Some(key(2)));
        for ordinary in 2..=5 {
            schedule.progressed(&key(1));
            schedule.progressed(&key(1)); // Coalesced progress earns one turn.
            schedule = restart(schedule);
            let work = schedule.order(current.clone());
            assert_eq!(work[0].key, key(1));
            assert_eq!(work[1].key, key(ordinary));
            assert_eq!(work.len(), current.len());
            for item in &work[..2] {
                schedule.started(item);
            }
        }
        // A stalled or denied continuation does not renew itself on restart.
        let mut schedule = restart(schedule);
        assert_eq!(schedule.order(current.clone())[0].key, key(6));
        schedule.progressed(&key(1));
        let mut changed = current.clone();
        changed.get_mut(&key(1)).unwrap()["headRefOid"] = json!("new-head");
        schedule.reconcile(&changed);
        assert!(schedule.resume.is_empty());
        assert_eq!(schedule.urgent.front(), Some(&key(1)));
        schedule.progressed(&key(1));
        schedule.succeeded(&key(1));
        assert!(schedule.resume.is_empty());
        assert!(schedule.urgent.is_empty());
    }
}
