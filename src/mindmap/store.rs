use super::{get_issue, resolve_project};
use crate::database::Connection;
use crate::issues::{Error, Project, Result};
use crate::mindmap::{BodyMode, Operation, ReadBudget, project_body};
use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;

pub(super) const SCHEMA: &str = include_str!("schema.sql");
// Additive lookup indexes also apply to databases created by early feature builds.
pub(super) const INDEXES: &str = "
CREATE INDEX IF NOT EXISTS mindmap_reference_lookup ON mindmap_nodes(kind,reference_project,reference,project_id);
CREATE INDEX IF NOT EXISTS issue_pr_canonical_url ON issue_pull_requests(rtrim(url,'/'),project_id,issue_number);
";
const COLUMNS: &str = "id,project_id,alias,parent_id,position,kind,title,body,reference,reference_project,created_at,updated_at,display_label";
fn row(r: &crate::database::Row<'_>) -> rusqlite::Result<Value> {
    let mut node = json!({"id":r.get::<_,String>(0)?,"project_id":r.get::<_,String>(1)?,"alias":r.get::<_,Option<String>>(2)?,"parent_id":r.get::<_,Option<String>>(3)?,"position":r.get::<_,i64>(4)?,"kind":r.get::<_,String>(5)?,"title":r.get::<_,String>(6)?,"body":r.get::<_,String>(7)?,"reference":r.get::<_,Option<String>>(8)?,"reference_project":r.get::<_,Option<String>>(9)?,"created_at":r.get::<_,i64>(10)?,"updated_at":r.get::<_,i64>(11)?,"automatic":false,"available":true});
    node["display_label"] = json!(r.get::<_, Option<String>>(12)?);
    Ok(node)
}
fn get(db: &Connection, id: &str) -> Result<Value> {
    db.query_row(
        &format!("SELECT {COLUMNS} FROM mindmap_nodes WHERE id=?1"),
        [id],
        row,
    )
    .optional()?
    .ok_or_else(|| Error::new("not_found", format!("Mindmap node {id:?} was not found")))
}
fn projected_columns(mode: BodyMode) -> String {
    let body = match mode {
        BodyMode::Full => "body",
        BodyMode::Preview => "substr(body,1,513)",
        BodyMode::None => "''",
    };
    format!(
        "{},(body<>'')",
        COLUMNS
            .split(',')
            .map(|column| if column == "body" { body } else { column })
            .collect::<Vec<_>>()
            .join(",")
    )
}
fn projected_row(r: &crate::database::Row<'_>) -> rusqlite::Result<Value> {
    let mut node = row(r)?;
    node["has_body"] = json!(r.get::<_, bool>(13)?);
    Ok(node)
}
enum ExternalNode {
    Saved(String),
    Resource(Value),
}

fn external_nodes(
    db: &Connection,
    endpoints: Vec<ExternalNode>,
    mode: BodyMode,
    budget: &mut ReadBudget,
) -> Result<Vec<Value>> {
    let batch_size = if mode == BodyMode::Full {
        16
    } else {
        endpoints.len().max(1)
    };
    let mut nodes = Vec::with_capacity(endpoints.len());
    for batch in endpoints.chunks(batch_size) {
        let ids: Vec<_> = batch
            .iter()
            .filter_map(|endpoint| match endpoint {
                ExternalNode::Saved(id) => Some(id),
                ExternalNode::Resource(_) => None,
            })
            .collect();
        let mut saved = BTreeMap::new();
        if !ids.is_empty() {
            let mut query = db.prepare(&format!(
                "SELECT {} FROM mindmap_nodes WHERE id IN (SELECT value FROM json_each(?1))",
                projected_columns(mode)
            ))?;
            for node in query.query_map([serde_json::to_string(&ids)?], projected_row)? {
                let node = node?;
                saved.insert(id(&node).to_owned(), node);
            }
        }
        let mut projected = batch
            .iter()
            .map(|endpoint| match endpoint {
                ExternalNode::Saved(id) => saved
                    .remove(id)
                    .ok_or_else(|| Error::new("not_found", "Mindmap node was not found")),
                ExternalNode::Resource(node) => Ok(node.clone()),
            })
            .collect::<Result<Vec<_>>>()?;
        live_nodes(db, &mut projected, mode, budget)?;
        nodes.extend(projected);
    }
    Ok(nodes)
}
fn id(node: &Value) -> &str {
    node["id"].as_str().unwrap()
}
fn new_id() -> Result<String> {
    let mut bytes = [0u8; 16];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(format!(
        "n-{}",
        bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()
    ))
}
fn canonical_ref(
    db: &Connection,
    p: &Project,
    kind: &str,
    reference: &str,
    ref_project: Option<&str>,
    verify_issue: bool,
) -> Result<(String, String)> {
    crate::issues::identifier(reference, "resource reference", 2048)?;
    match kind {
        "issue" => {
            let number = reference
                .parse::<i64>()
                .map_err(|_| Error::invalid("Issue reference must be a positive number"))?;
            if number <= 0 {
                return Err(Error::invalid("Issue number must be positive"));
            }
            let target = resolve_project(db, p, ref_project)?;
            if verify_issue {
                get_issue(db, &target.id, number, false)?;
            }
            Ok((number.to_string(), target.id))
        }
        "pr" => {
            let url = reference.trim_end_matches('/');
            let after = url
                .strip_prefix("https://")
                .or_else(|| url.strip_prefix("http://"))
                .ok_or_else(|| Error::invalid("PR reference must be an HTTP(S) URL"))?;
            if !after.contains('/')
                || after.starts_with('/')
                || url.chars().any(char::is_whitespace)
            {
                return Err(Error::invalid("PR reference must have a host and path"));
            }
            Ok((url.into(), String::new()))
        }
        "notification" => {
            crate::issues::identifier(reference, "notification ID", 256)?;
            Ok((reference.into(), String::new()))
        }
        _ => Err(Error::invalid("Unknown reference type")),
    }
}
struct NewNode<'a> {
    title: &'a str,
    body: &'a str,
    kind: &'a str,
    reference: Option<&'a str>,
    ref_project: Option<&'a str>,
    alias: Option<&'a str>,
    parent: Option<&'a str>,
}
fn insert(db: &Connection, p: &Project, node: NewNode<'_>, now: i64) -> Result<Value> {
    let NewNode {
        title,
        body,
        kind,
        reference,
        ref_project,
        alias,
        parent,
    } = node;
    let count: i64 = db.query_row(
        "SELECT count(*) FROM mindmap_nodes WHERE project_id=?1",
        [&p.id],
        |r| r.get(0),
    )?;
    if count >= 10000 {
        return Err(Error::conflict(
            "A project map supports at most 10000 nodes",
        ));
    }
    if let Some(alias) = alias
        && db.query_row(
            "SELECT EXISTS(SELECT 1 FROM mindmap_nodes WHERE project_id=?1 AND alias=?2)",
            params![p.id, alias],
            |r| r.get::<_, bool>(0),
        )?
    {
        return Err(Error::conflict(format!(
            "Alias {alias:?} is already in use"
        )));
    }
    let (reference, ref_project) = match reference {
        Some(r) => {
            let (r, p) = canonical_ref(db, p, kind, r, ref_project, true)?;
            (Some(r), Some(p))
        }
        None => (None, None),
    };
    if reference.is_some() && db.query_row("SELECT EXISTS(SELECT 1 FROM mindmap_nodes WHERE project_id=?1 AND kind=?2 AND reference_project=?3 AND reference=?4)",params![p.id,kind,ref_project,reference],|r|r.get::<_,bool>(0))? { return Err(Error::conflict("This resource is already in the map; use its reference or alias to move/link it")); }
    let position:i64=db.query_row("SELECT coalesce(max(position),-1)+1 FROM mindmap_nodes WHERE project_id=?1 AND parent_id IS ?2",params![p.id,parent],|r|r.get(0))?;
    let node = new_id()?;
    db.execute(
        "INSERT INTO mindmap_nodes(id,project_id,alias,parent_id,position,kind,title,body,reference,reference_project,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?11)",
        params![
            node,
            p.id,
            alias,
            parent,
            position,
            kind,
            title,
            body,
            reference,
            ref_project,
            now
        ],
    )?;
    get(db, &node)
}
fn select(
    db: &Connection,
    p: &Project,
    selector: &str,
    create: bool,
    now: i64,
    touched: &mut BTreeSet<String>,
) -> Result<Value> {
    crate::issues::identifier(selector, "node selector", 16384)?;
    // Typed references may themselves contain :: (IPv6 URLs or task IDs).
    // Qualification precedes the type prefix; never split inside a resource.
    let qualified = !["pr:", "issue:", "notice:"]
        .iter()
        .any(|prefix| selector.starts_with(prefix))
        && selector.contains("::");
    let (p, selector) = if qualified {
        let (project, selector) = selector.split_once("::").unwrap();
        (resolve_project(db, p, Some(project))?, selector)
    } else {
        (p.clone(), selector)
    };
    if selector.starts_with("n-") {
        let node = get(db, selector)?;
        if qualified && node["project_id"] != p.id {
            return Err(Error::new(
                "not_found",
                "Node ID does not belong to the qualified project",
            ));
        }
        if selector != selector.trim() {
            return Err(Error::invalid("Invalid node ID"));
        }
        return Ok(node);
    }
    let typed = selector
        .strip_prefix("issue:")
        .map(|r| ("issue", r))
        .or_else(|| selector.strip_prefix("pr:").map(|r| ("pr", r)))
        .or_else(|| {
            selector
                .strip_prefix("notice:")
                .map(|r| ("notification", r))
        });
    if let Some((kind, r)) = typed {
        let (reference, ref_project) = canonical_ref(db, &p, kind, r, None, false)?;
        let found=db.query_row(&format!("SELECT {COLUMNS} FROM mindmap_nodes WHERE project_id=?1 AND kind=?2 AND reference_project=?3 AND reference=?4"),params![p.id,kind,ref_project,reference],row).optional()?;
        if let Some(node) = found {
            return Ok(node);
        }
        if create {
            db.execute("INSERT OR IGNORE INTO projects(id,name,next_number,created_at,activity_at) VALUES(?1,?2,1,?3,?3)",params![p.id,p.name,now])?;
            let title = match kind {
                "issue" => format!("Issue #{reference}"),
                "pr" => reference.clone(),
                _ => format!("Notification {reference}"),
            };
            let node = insert(
                db,
                &p,
                NewNode {
                    title: &title,
                    body: "",
                    kind,
                    reference: Some(&reference),
                    ref_project: Some(&ref_project),
                    alias: None,
                    parent: None,
                },
                now,
            )?;
            touched.insert(p.id);
            return Ok(node);
        }
        if kind == "pr" {
            return Err(Error::new(
                "not_found",
                format!(
                    "No stored PR node for {reference:?} in {}. Automatic PR rows \
                     (automatic:true in mm show --json) are display resources, not editable stored nodes. \
                     Create an explicit node first: hey-boss mm pr {} --project {} \
                     (add --under issue:NUMBER to keep it under its issue, and --title TITLE for a label). \
                     Then reload hey-boss mm show --json and retry the batch. \
                     No changes were made.",
                    p.id,
                    crate::health::remote::quote(&reference),
                    crate::health::remote::quote(&p.id),
                ),
            ));
        }
    } else {
        let found = db
            .query_row(
                &format!("SELECT {COLUMNS} FROM mindmap_nodes WHERE project_id=?1 AND alias=?2"),
                params![p.id, selector],
                row,
            )
            .optional()?;
        if let Some(node) = found {
            return Ok(node);
        }
    }
    Err(Error::new(
        "not_found",
        format!("Node {selector:?} was not found in {}", p.id),
    ))
}
fn same_project(node: &Value, p: &Project) -> Result<()> {
    if node["project_id"] != p.id {
        Err(Error::invalid(
            "Nesting and node edits stay within a project; select that project with --project",
        ))
    } else {
        Ok(())
    }
}
fn depth(db: &Connection, parent: Option<&str>) -> Result<usize> {
    let mut parent = parent.map(str::to_owned);
    let mut depth = 0;
    while let Some(current) = parent {
        depth += 1;
        if depth > 32 {
            return Err(Error::conflict(
                "Mindmap nesting supports at most 32 levels",
            ));
        }
        parent = get(db, &current)?["parent_id"].as_str().map(str::to_owned);
    }
    Ok(depth)
}
fn descendants(db: &Connection, node: &str) -> Result<Vec<String>> {
    let mut stmt=db.prepare("WITH RECURSIVE tree(id) AS (SELECT id FROM mindmap_nodes WHERE id=?1 UNION ALL SELECT n.id FROM mindmap_nodes n JOIN tree t ON n.parent_id=t.id) SELECT id FROM tree")?;
    Ok(stmt
        .query_map([node], |r| r.get(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?)
}
fn version(db: &Connection, p: &str) -> Result<i64> {
    Ok(db
        .query_row(
            "SELECT version FROM mindmaps WHERE project_id=?1",
            [p],
            |r| r.get(0),
        )
        .optional()?
        .unwrap_or(0))
}
fn expected(op: &Operation) -> Option<i64> {
    match op {
        Operation::Batch { if_version, .. }
        | Operation::Add { if_version, .. }
        | Operation::Edit { if_version, .. }
        | Operation::Alias { if_version, .. }
        | Operation::Move { if_version, .. }
        | Operation::Remove { if_version, .. }
        | Operation::Link { if_version, .. }
        | Operation::Unlink { if_version, .. } => *if_version,
        _ => None,
    }
}

pub(super) fn execute(db: &Connection, p: &Project, op: &Operation, now: i64) -> Result<Value> {
    let replica: bool =
        db.query_row("SELECT role='agent' FROM fleet_meta WHERE id=1", [], |r| {
            r.get(0)
        })?;
    if replica {
        return Err(Error::invalid(
            "Mindmaps are not replicated on fleet companions; use --host SUPERVISOR (or HEY_BOSS_ISSUE_HOST) to read and author the authoritative map",
        ));
    }
    if let Some(expected) = expected(op)
        && expected != version(db, &p.id)?
    {
        return Err(Error::conflict(
            "Map changed; show it again and retry with the current version",
        ));
    }
    if let Operation::Batch { edits, .. } = op {
        return batch(db, p, edits, now);
    }
    execute_single(db, p, op, now, true)
}

fn organization_snapshot(db: &Connection, p: &Project) -> Result<BTreeMap<String, Value>> {
    let mut stmt = db.prepare("SELECT id,alias,parent_id,position,title,display_label FROM mindmap_nodes WHERE project_id=?1")?;
    Ok(stmt
        .query_map([&p.id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                json!({
                    "alias":r.get::<_,Option<String>>(1)?,
                    "parent_id":r.get::<_,Option<String>>(2)?,
                    "position":r.get::<_,i64>(3)?,
                    "title":r.get::<_,String>(4)?,
                    "display_label":r.get::<_,Option<String>>(5)?
                }),
            ))
        })?
        .collect::<rusqlite::Result<_>>()?)
}

type LinkKey = (String, String, String);

fn link_snapshot(db: &Connection, key: &LinkKey) -> Result<Value> {
    Ok(db
        .query_row(
            "SELECT description FROM mindmap_links WHERE source=?1 AND target=?2 AND kind=?3",
            params![key.0, key.1, key.2],
            |r| Ok(json!({"description":r.get::<_,Option<String>>(0)?})),
        )
        .optional()?
        .unwrap_or(Value::Null))
}

fn batch(
    db: &Connection,
    p: &Project,
    edits: &[crate::mindmap::BatchEdit],
    now: i64,
) -> Result<Value> {
    let base_version = version(db, &p.id)?;
    let before = organization_snapshot(db, p)?;
    db.execute_batch("SAVEPOINT mindmap_batch")?;
    // Bind all selectors against the original map. Changing an alias never
    // changes the meaning of a later selector in the same input.
    let mut touched = BTreeSet::new();
    let mut resolve = |selector: &str| -> Result<String> {
        let node = select(db, p, selector, false, now, &mut touched)?;
        same_project(&node, p)?;
        Ok(id(&node).to_owned())
    };
    let mut operations = edits
        .iter()
        .map(|edit| {
            let mut op = edit.operation();
            match &mut op {
                Operation::Edit { node, .. } | Operation::Alias { node, .. } => {
                    *node = resolve(node)?
                }
                Operation::Move {
                    node,
                    under,
                    before,
                    after,
                    ..
                } => {
                    *node = resolve(node)?;
                    for selector in [under, before, after].into_iter().flatten() {
                        *selector = resolve(selector)?;
                    }
                }
                Operation::Link { .. } => {}
                _ => unreachable!(),
            }
            Ok(op)
        })
        .collect::<Result<Vec<_>>>()?;
    // Bind organization selectors first: typed endpoints may add reference
    // nodes, but those must not become selectors for earlier/later node edits.
    let mut created_nodes = BTreeMap::new();
    let mut links_before = BTreeMap::new();
    let mut link_projects = BTreeMap::new();
    for op in &mut operations {
        if let Operation::Link { from, to, kind, .. } = op {
            let mut projects = BTreeSet::new();
            for selector in [&mut *from, &mut *to] {
                let node = match select(db, p, selector, false, now, &mut touched) {
                    Ok(node) => node,
                    Err(error) if error.code == "not_found" => {
                        let node = select(db, p, selector, true, now, &mut touched)?;
                        created_nodes.insert(
                            id(&node).to_owned(),
                            node["project_id"].as_str().unwrap().to_owned(),
                        );
                        node
                    }
                    Err(error) => return Err(error),
                };
                projects.insert(node["project_id"].as_str().unwrap().to_owned());
                *selector = id(&node).to_owned();
            }
            let key = (from.clone(), to.clone(), kind.clone());
            if !links_before.contains_key(&key) {
                links_before.insert(key.clone(), link_snapshot(db, &key)?);
                link_projects.insert(key, projects);
            }
        }
    }
    for op in &operations {
        execute_single(db, p, op, now, false)?;
    }
    let after = organization_snapshot(db, p)?;
    // Reject newly requested ambiguous labels, without making pre-existing
    // duplicate ordinary titles prevent unrelated organization.
    let mut labels = BTreeMap::<&str, usize>::new();
    for value in after.values() {
        let label = value["display_label"]
            .as_str()
            .unwrap_or(value["title"].as_str().unwrap());
        *labels.entry(label).or_default() += 1;
    }
    for op in &operations {
        if let Operation::Edit {
            node,
            title: Some(_),
            ..
        } = op
        {
            let value = &after[node];
            if value["title"] == before[node]["title"]
                && value["display_label"] == before[node]["display_label"]
            {
                continue;
            }
            let label = value["display_label"]
                .as_str()
                .unwrap_or(value["title"].as_str().unwrap());
            let collision = labels[label] > 1;
            if collision {
                return Err(Error::conflict(format!(
                    "Label {label:?} is already in use"
                )));
            }
        }
    }
    let mut changed_nodes = after
        .iter()
        .filter(|(id, value)| before.get(*id) != Some(*value))
        .map(|(id, value)| json!({"id":id,"before":before.get(id),"after":value}))
        .collect::<Vec<_>>();
    // Include reference nodes created in other maps without copying their bodies.
    for (node, project) in &created_nodes {
        if project != &p.id {
            let snapshot = db.query_row(
                "SELECT alias,parent_id,position,title,display_label FROM mindmap_nodes WHERE id=?1",
                [node],
                |r| Ok(json!({"alias":r.get::<_,Option<String>>(0)?,
                    "parent_id":r.get::<_,Option<String>>(1)?,"position":r.get::<_,i64>(2)?,
                    "title":r.get::<_,String>(3)?,"display_label":r.get::<_,Option<String>>(4)?})),
            )?;
            changed_nodes.push(json!({"id":node,"before":null,"after":snapshot}));
        }
    }
    let mut changed_projects = created_nodes.values().cloned().collect::<BTreeSet<_>>();
    if after != before {
        changed_projects.insert(p.id.clone());
    }
    let mut changed_links = Vec::new();
    for (key, before) in links_before {
        let after = link_snapshot(db, &key)?;
        if before != after {
            changed_projects.extend(link_projects[&key].iter().cloned());
            changed_links
                .push(json!({"from":key.0,"to":key.1,"kind":key.2,"before":before,"after":after}));
        }
    }
    let changed = !changed_nodes.is_empty() || !changed_links.is_empty();
    if !changed {
        // Also undo timestamps when a sequence cancels its own edits.
        db.execute_batch("ROLLBACK TO mindmap_batch")?;
    }
    for project in &changed_projects {
        db.execute("INSERT INTO mindmaps(project_id,version) VALUES(?1,1) ON CONFLICT(project_id) DO UPDATE SET version=version+1", [project])?;
        db.execute(
            "UPDATE projects SET activity_at=max(activity_at,?2) WHERE id=?1",
            params![project, now],
        )?;
    }
    db.execute_batch("RELEASE mindmap_batch")?;
    let affected_projects = changed_projects
        .iter()
        .map(|project| Ok(json!({"project":project,"version":version(db,project)?})))
        .collect::<Result<Vec<_>>>()?;
    let result = json!({"ok":true,"project":p,"changed":changed,
        "base_version":base_version,"version":version(db,&p.id)?,"changed_nodes":changed_nodes,
        "changed_links":changed_links,"affected_projects":affected_projects});
    ReadBudget::default().charge(&result)?;
    Ok(result)
}

fn execute_single(
    db: &Connection,
    p: &Project,
    op: &Operation,
    now: i64,
    bump_version: bool,
) -> Result<Value> {
    let mut touched = BTreeSet::new();
    let mut selected = None;
    let mut relationship = None;
    let mut changed = false;
    match op {
        Operation::Batch { .. } => unreachable!("Batches are dispatched before single operations"),
        Operation::Show { .. } => {}
        Operation::View { node, body_mode } => {
            let mut node = select(db, p, node, false, now, &mut touched)?;
            let project = resolve_project(db, p, node["project_id"].as_str())?;
            live(db, &mut node, *body_mode)?;
            let result = json!({"ok":true,"project":project,"version":version(db,&project.id)?,"body_mode":body_mode,"node":node,"nodes":[node],"external_nodes":[],"links":[]});
            ReadBudget::default().charge(&result)?;
            return Ok(result);
        }
        Operation::Projects => {
            let mut stmt=db.prepare("SELECT p.id,p.name,coalesce(m.version,0),count(n.id) FROM projects p LEFT JOIN mindmaps m ON m.project_id=p.id LEFT JOIN mindmap_nodes n ON n.project_id=p.id WHERE p.hidden_at IS NULL GROUP BY p.id HAVING count(n.id)>0 ORDER BY lower(p.name),p.id")?;
            let projects=stmt.query_map([],|r|Ok(json!({"id":r.get::<_,String>(0)?,"name":r.get::<_,String>(1)?,"version":r.get::<_,i64>(2)?,"node_count":r.get::<_,i64>(3)?})))?.collect::<rusqlite::Result<Vec<_>>>()?;
            return Ok(json!({"ok":true,"project":p,"projects":projects}));
        }
        Operation::Links { node } => {
            if let Some(node) = node {
                selected = Some(select(db, p, node, false, now, &mut touched)?);
            }
        }
        Operation::Add {
            title,
            display_label,
            body,
            kind,
            reference,
            reference_project,
            alias,
            under,
            ..
        } => {
            let parent = under
                .as_deref()
                .map(|s| select(db, p, s, false, now, &mut touched))
                .transpose()?;
            if let Some(parent) = &parent {
                same_project(parent, p)?;
            }
            if depth(db, parent.as_ref().map(id))? >= 32 {
                return Err(Error::conflict(
                    "Mindmap nesting supports at most 32 levels",
                ));
            }
            // Only an explicitly supplied issue label permits reusing an
            // existing reference. Omitted placement leaves its location intact.
            let existing = if display_label.is_some() {
                let (reference, ref_project) = canonical_ref(
                    db,
                    p,
                    kind,
                    reference.as_deref().unwrap(),
                    reference_project.as_deref(),
                    true,
                )?;
                db.query_row(
                    &format!("SELECT {COLUMNS} FROM mindmap_nodes WHERE project_id=?1 AND kind=?2 AND reference_project=?3 AND reference=?4"),
                    params![p.id, kind, ref_project, reference], row,
                ).optional()?
            } else {
                None
            };
            let node = if let Some(node) = existing {
                if alias
                    .as_ref()
                    .is_some_and(|alias| node["alias"] != json!(alias))
                    || (under.is_some() && node["parent_id"] != json!(parent.as_ref().map(id)))
                {
                    return Err(Error::conflict(
                        "This reference has a different alias or parent; use mm alias or mm move",
                    ));
                }
                node
            } else {
                changed = true;
                insert(
                    db,
                    p,
                    NewNode {
                        title,
                        body,
                        kind,
                        reference: reference.as_deref(),
                        ref_project: reference_project.as_deref(),
                        alias: alias.as_deref(),
                        parent: parent.as_ref().map(id),
                    },
                    now,
                )?
            };
            if let Some(label) = display_label
                && node["display_label"] != json!(label)
            {
                db.execute(
                    "UPDATE mindmap_nodes SET display_label=?2,updated_at=?3 WHERE id=?1",
                    params![id(&node), label, now],
                )?;
                changed = true;
                selected = Some(get(db, id(&node))?);
            } else {
                selected = Some(node);
            }
        }
        Operation::Edit {
            node,
            title,
            body,
            clear_label,
            ..
        } => {
            let node = select(db, p, node, false, now, &mut touched)?;
            same_project(&node, p)?;
            if node["kind"] == "issue" {
                if body.is_some() {
                    return Err(Error::invalid(
                        "Issue nodes accept --title or --clear-label only",
                    ));
                }
                if let Some(title) = title {
                    crate::issues::identifier(title, "node title", 512)?;
                }
                let label = if *clear_label { None } else { title.as_deref() };
                changed = node["display_label"] != json!(label);
                if changed {
                    db.execute(
                        "UPDATE mindmap_nodes SET display_label=?2,updated_at=?3 WHERE id=?1",
                        params![id(&node), label, now],
                    )?;
                }
                selected = Some(get(db, id(&node))?);
            } else {
                if *clear_label {
                    return Err(Error::invalid("--clear-label requires an issue node"));
                }
                if !["text", "markdown", "pr"].contains(&node["kind"].as_str().unwrap()) {
                    return Err(Error::invalid(
                        "Reference nodes use live content; edit the underlying resource instead",
                    ));
                }
                if node["kind"] == "pr" && body.is_some() {
                    return Err(Error::invalid("PR nodes accept --title only"));
                }
                let title = title.as_deref().unwrap_or(node["title"].as_str().unwrap());
                if node["kind"] != "pr" || node["reference"] != title.trim_end_matches('/') {
                    crate::issues::identifier(title, "node title", 512)?;
                }
                let body = body.as_deref().unwrap_or(node["body"].as_str().unwrap());
                changed = node["title"] != title || node["body"] != body;
                if changed {
                    db.execute("UPDATE mindmap_nodes SET title=?2,body=?3,kind=CASE WHEN kind IN ('text','markdown') AND length(?3)>0 THEN 'markdown' ELSE kind END,updated_at=?4 WHERE id=?1",params![id(&node),title,body,now])?;
                }
                selected = Some(get(db, id(&node))?);
            }
        }
        Operation::Alias { node, alias, .. } => {
            let node = select(db, p, node, false, now, &mut touched)?;
            same_project(&node, p)?;
            if let Some(alias) = alias {
                let collision: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM mindmap_nodes WHERE project_id=?1 AND alias=?2 AND id<>?3)",params![p.id,alias,id(&node)],|row|row.get(0))?;
                if collision {
                    return Err(Error::conflict(format!(
                        "Alias {alias:?} is already in use"
                    )));
                }
            }
            changed = node["alias"].as_str() != alias.as_deref();
            if changed {
                db.execute(
                    "UPDATE mindmap_nodes SET alias=?2,updated_at=?3 WHERE id=?1",
                    params![id(&node), alias, now],
                )?;
            }
            selected = Some(get(db, id(&node))?);
        }
        Operation::Move {
            node,
            under,
            before,
            after,
            ..
        } => {
            let node = select(db, p, node, false, now, &mut touched)?;
            same_project(&node, p)?;
            let anchor = before
                .as_deref()
                .or(after.as_deref())
                .map(|s| select(db, p, s, false, now, &mut touched))
                .transpose()?;
            if let Some(a) = &anchor {
                same_project(a, p)?;
                if id(a) == id(&node) {
                    return Err(Error::invalid("A node cannot anchor its own move"));
                }
            }
            let parent = if let Some(s) = under {
                let parent = select(db, p, s, false, now, &mut touched)?;
                same_project(&parent, p)?;
                Some(id(&parent).to_owned())
            } else {
                anchor
                    .as_ref()
                    .and_then(|a| a["parent_id"].as_str().map(str::to_owned))
            };
            if let Some(a) = &anchor
                && a["parent_id"].as_str() != parent.as_deref()
            {
                return Err(Error::invalid(
                    "Anchor must be a child of the destination parent",
                ));
            }
            let descendants = descendants(db, id(&node))?;
            if parent.as_ref().is_some_and(|p| descendants.contains(p)) {
                return Err(Error::conflict("Moving here would create a nesting cycle"));
            }
            let subtree_depth:i64=db.query_row("WITH RECURSIVE tree(id,depth) AS (SELECT id,1 FROM mindmap_nodes WHERE id=?1 UNION ALL SELECT n.id,t.depth+1 FROM mindmap_nodes n JOIN tree t ON n.parent_id=t.id) SELECT max(depth) FROM tree",[id(&node)],|r|r.get(0))?;
            if depth(db, parent.as_deref())? + subtree_depth as usize > 32 {
                return Err(Error::conflict(
                    "Mindmap nesting supports at most 32 levels",
                ));
            }
            let mut stmt=db.prepare("SELECT id FROM mindmap_nodes WHERE project_id=?1 AND parent_id IS ?2 AND id<>?3 ORDER BY position,id")?;
            let mut siblings = stmt
                .query_map(params![p.id, parent, id(&node)], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let index = anchor
                .as_ref()
                .map(|a| {
                    siblings.iter().position(|s| s == id(a)).unwrap() + usize::from(after.is_some())
                })
                .unwrap_or(siblings.len());
            // An unchanged destination/order is a real no-op, including timestamps.
            let current: Vec<String> = {
                let mut stmt = db.prepare("SELECT id FROM mindmap_nodes WHERE project_id=?1 AND parent_id IS ?2 ORDER BY position,id")?;
                stmt.query_map(params![p.id, parent], |r| r.get(0))?
                    .collect::<rusqlite::Result<_>>()?
            };
            siblings.insert(index, id(&node).into());
            changed = node["parent_id"].as_str() != parent.as_deref() || current != siblings;
            if !changed {
                selected = Some(node);
            } else {
                db.execute(
                    "UPDATE mindmap_nodes SET parent_id=?2,updated_at=?3 WHERE id=?1",
                    params![id(&node), parent, now],
                )?;
                for (position, sibling) in siblings.iter().enumerate() {
                    db.execute(
                        "UPDATE mindmap_nodes SET position=?2 WHERE id=?1",
                        params![sibling, position as i64],
                    )?;
                }
                selected = Some(get(db, id(&node))?);
            }
        }
        Operation::Remove {
            node, recursive, ..
        } => {
            let node = select(db, p, node, false, now, &mut touched)?;
            same_project(&node, p)?;
            let tree = descendants(db, id(&node))?;
            if tree.len() > 1 && !*recursive {
                return Err(Error::conflict(
                    "Node has children; move them first or use --recursive",
                ));
            }
            for child in tree.iter().rev() {
                let mut stmt=db.prepare("SELECT DISTINCT n.project_id FROM mindmap_links l JOIN mindmap_nodes n ON n.id=l.source OR n.id=l.target WHERE l.source=?1 OR l.target=?1")?;
                for project in stmt.query_map([child], |r| r.get::<_, String>(0))? {
                    touched.insert(project?);
                }
                db.execute("DELETE FROM mindmap_nodes WHERE id=?1", [child])?;
            }
            changed = true;
        }
        Operation::Link {
            from,
            to,
            kind,
            description,
            ..
        } => {
            let from = select(db, p, from, true, now, &mut touched)?;
            let to = select(db, p, to, true, now, &mut touched)?;
            if id(&from) == id(&to) {
                return Err(Error::invalid("Cannot link a node to itself"));
            }
            let description = description.as_deref().filter(|s| !s.trim().is_empty());
            let previous:Option<Option<String>>=db.query_row("SELECT description FROM mindmap_links WHERE source=?1 AND target=?2 AND kind=?3",params![id(&from),id(&to),kind],|r|r.get(0)).optional()?;
            changed = previous
                .as_ref()
                .is_none_or(|old| old.as_deref() != description);
            if changed {
                db.execute("INSERT INTO mindmap_links VALUES(?1,?2,?3,?4,?5) ON CONFLICT(source,target,kind) DO UPDATE SET description=excluded.description",params![id(&from),id(&to),kind,description,now])?;
                touched.insert(from["project_id"].as_str().unwrap().into());
                touched.insert(to["project_id"].as_str().unwrap().into());
            }
            relationship = Some(
                json!({"from":from["id"],"to":to["id"],"kind":kind,"description":description}),
            );
            selected = Some(from);
        }
        Operation::Unlink { from, to, kind, .. } => {
            let from = select(db, p, from, false, now, &mut touched)?;
            let to = select(db, p, to, false, now, &mut touched)?;
            changed = db.execute(
                "DELETE FROM mindmap_links WHERE source=?1 AND target=?2 AND kind=?3",
                params![id(&from), id(&to), kind],
            )? > 0;
            if changed {
                touched.insert(from["project_id"].as_str().unwrap().into());
                touched.insert(to["project_id"].as_str().unwrap().into());
            }
            selected = Some(from);
        }
    }
    if changed && !matches!(op, Operation::Link { .. } | Operation::Unlink { .. }) {
        touched.insert(p.id.clone());
    }
    for project in touched.iter().filter(|_| bump_version) {
        db.execute("INSERT INTO mindmaps(project_id,version) VALUES(?1,1) ON CONFLICT(project_id) DO UPDATE SET version=version+1",[project])?;
        db.execute(
            "UPDATE projects SET activity_at=max(activity_at,?2) WHERE id=?1",
            params![project, now],
        )?;
    }
    if op.writes() {
        // Return just the affected metadata. A full graph here would be copied
        // into every request-id receipt and make authoring grow quadratically.
        let mut result = json!({"ok":true,"project":p,"version":version(db,&p.id)?,"changed":changed||!touched.is_empty()});
        if let Some(node) = selected {
            result["node"] = node;
        }
        if let Some(link) = relationship {
            result["link"] = link;
        }
        let versions = touched
            .iter()
            .map(|project| Ok(json!({"project":project,"version":version(db,project)?})))
            .collect::<Result<Vec<_>>>()?;
        result["affected_projects"] = json!(versions);
        return Ok(result);
    }
    let viewed_project = if let Some(node) = selected
        .as_ref()
        .filter(|_| matches!(op, Operation::Links { .. }))
    {
        resolve_project(db, p, node["project_id"].as_str())?
    } else {
        p.clone()
    };
    let mode = match op {
        Operation::Show { body_mode } => *body_mode,
        Operation::Links { .. } => BodyMode::None,
        _ => BodyMode::Full,
    };
    let focus = selected
        .as_ref()
        .filter(|_| matches!(op, Operation::Links { .. }))
        .map(id);
    let mut graph = graph(db, &viewed_project, mode, focus)?;
    graph["changed"] = json!(changed || !touched.is_empty());
    if let Some(node) = selected {
        graph["node"] = graph["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .chain(graph["external_nodes"].as_array().unwrap())
            .find(|n| n["id"] == node["id"])
            .cloned()
            .unwrap_or_else(|| node.clone());
        if matches!(op, Operation::Links { .. }) {
            graph["links"]
                .as_array_mut()
                .unwrap()
                .retain(|l| l["from"] == node["id"] || l["to"] == node["id"]);
        }
    }
    Ok(graph)
}
fn live(db: &Connection, node: &mut Value, mode: BodyMode) -> Result<()> {
    live_batch(db, std::slice::from_mut(node), mode)
}

fn live_nodes(
    db: &Connection,
    nodes: &mut [Value],
    mode: BodyMode,
    budget: &mut ReadBudget,
) -> Result<()> {
    // Full issue bodies can each occupy 1 MiB. Check the output budget between
    // batches instead of loading every full body before rejecting a large map.
    let batch_size = if mode == BodyMode::Full {
        16
    } else {
        nodes.len().max(1)
    };
    for batch in nodes.chunks_mut(batch_size) {
        live_batch(db, batch, mode)?;
        for node in batch {
            budget.charge(node)?;
        }
    }
    Ok(())
}

fn live_batch(db: &Connection, nodes: &mut [Value], mode: BodyMode) -> Result<()> {
    let references: BTreeSet<_> = nodes
        .iter()
        .filter(|n| n["kind"] == "issue")
        .map(|n| {
            (
                n["reference_project"].as_str().unwrap(),
                n["reference"].as_str().unwrap().parse::<i64>().unwrap(),
            )
        })
        .collect();
    let mut issues = BTreeMap::new();
    if !references.is_empty() {
        let body = match mode {
            BodyMode::None => "''",
            // 513 UTF-8 characters occupy at most 2052 bytes. BLOB substr keeps
            // NULs; a partial final character lies beyond the 512-char preview.
            BodyMode::Preview => "coalesce(substr(CAST(i.body AS BLOB),1,2052),x'')",
            BodyMode::Full => "i.body",
        };
        let mut query = db.prepare(&format!("SELECT json_extract(selected.value,'$[0]'),json_extract(selected.value,'$[1]'),p.name,i.title,{body},i.state,i.assignee,i.version,octet_length(i.body)>0,i.labels
            FROM json_each(?1) selected
            LEFT JOIN projects p ON p.id=json_extract(selected.value,'$[0]')
            LEFT JOIN issues i ON i.project_id=json_extract(selected.value,'$[0]') AND i.number=json_extract(selected.value,'$[1]') AND i.deleted_at IS NULL"))?;
        for row in query.query_map([serde_json::to_string(&references)?], |r| {
            let project: String = r.get(0)?;
            let name = r.get::<_, Option<String>>(2)?.unwrap_or_else(|| project.clone());
            let issue = if let Some(title) = r.get::<_, Option<String>>(3)? {
                let body = if mode == BodyMode::Preview {
                    String::from_utf8_lossy(&r.get::<_, Vec<u8>>(4)?).into_owned()
                } else { r.get::<_, String>(4)? };
                Some((json!({"title":title,"body":body,"state":r.get::<_,String>(5)?,"assignee":r.get::<_,Option<String>>(6)?,"resource_version":r.get::<_,i64>(7)?,"has_body":r.get::<_,bool>(8)?}), r.get::<_,String>(9)?))
            } else { None };
            Ok(((project, r.get::<_,i64>(1)?), (name, issue)))
        })? { let (key, value) = row?; issues.insert(key, value); }
    }
    for node in nodes {
        if node["kind"] == "issue" {
            let key = (
                node["reference_project"].as_str().unwrap().to_owned(),
                node["reference"].as_str().unwrap().parse::<i64>().unwrap(),
            );
            let (name, issue) = &issues[&key];
            node["reference_project_name"] = json!(name);
            if let Some((issue, labels)) = issue {
                for field in [
                    "title",
                    "body",
                    "state",
                    "assignee",
                    "resource_version",
                    "has_body",
                ] {
                    node[field] = issue[field].clone();
                }
                node["labels"] =
                    serde_json::from_str(labels).map_err(|e| Error::invalid(e.to_string()))?;
            } else {
                node["available"] = json!(false);
                node["state"] = json!("unavailable");
            }
        } else if node["kind"] == "notification" {
            node["available"] = json!(false);
            node["state"] = json!("unavailable");
        }
        project_body(node, mode);
        if node["kind"] == "issue" {
            issue_display_title(node);
        }
    }
    Ok(())
}
fn issue_display_title(node: &mut Value) {
    node["original_title"] = node["title"].clone();
    if node["display_label"].is_string() {
        node["title"] = node["display_label"].clone();
    }
}
fn graph(db: &Connection, p: &Project, mode: BodyMode, focus: Option<&str>) -> Result<Value> {
    use std::collections::{HashMap, HashSet};
    let mut stmt = db.prepare(&format!(
        "SELECT {} FROM mindmap_nodes WHERE project_id=?1 AND (?2 IS NULL OR id=?2) ORDER BY position,created_at,id",
        projected_columns(mode)
    ))?;
    let mut budget = ReadBudget::default();
    let mut nodes = Vec::new();
    for node in stmt.query_map(params![p.id, focus], projected_row)? {
        nodes.push(node?);
    }
    live_nodes(db, &mut nodes, mode, &mut budget)?;
    let mut stmt=db.prepare("SELECT l.source,l.target,l.kind,l.description,l.created_at FROM mindmap_links l JOIN mindmap_nodes s ON s.id=l.source JOIN mindmap_nodes t ON t.id=l.target WHERE (s.project_id=?1 OR t.project_id=?1) AND (?2 IS NULL OR l.source=?2 OR l.target=?2) ORDER BY l.created_at,l.source,l.target,l.kind")?;
    let mut links = Vec::new();
    for link in stmt.query_map(params![p.id,focus],|r|Ok(json!({"from":r.get::<_,String>(0)?,"to":r.get::<_,String>(1)?,"kind":r.get::<_,String>(2)?,"description":r.get::<_,Option<String>>(3)?,"created_at":r.get::<_,i64>(4)?,"automatic":false})))? {
        let link = link?;
        budget.charge(&link)?;
        links.push(link);
    }
    let mut known: HashSet<String> = nodes.iter().map(|n| id(n).to_owned()).collect();
    let mut external = Vec::new();
    for link in &links {
        for field in ["from", "to"] {
            let key = link[field].as_str().unwrap();
            if known.insert(key.to_owned()) {
                external.push(ExternalNode::Saved(key.to_owned()));
            }
        }
    }
    let references: BTreeSet<_> = nodes
        .iter()
        .filter(|n| n["kind"] == "issue" && n["available"] == true)
        .map(|n| {
            (
                n["reference_project"].as_str().unwrap(),
                n["reference"].as_str().unwrap().parse::<i64>().unwrap(),
            )
        })
        .collect();
    let mut attached_prs: BTreeMap<(String, i64), Vec<String>> = BTreeMap::new();
    if !references.is_empty() {
        let mut query = db.prepare("SELECT pr.project_id,pr.issue_number,pr.url FROM json_each(?1) selected
            JOIN issue_pull_requests pr ON pr.project_id=json_extract(selected.value,'$[0]') AND pr.issue_number=json_extract(selected.value,'$[1]')
            ORDER BY pr.created_at,pr.url")?;
        for row in query.query_map([serde_json::to_string(&references)?], |r| {
            Ok((
                (r.get::<_, String>(0)?, r.get::<_, i64>(1)?),
                r.get::<_, String>(2)?,
            ))
        })? {
            let (key, url) = row?;
            attached_prs.entry(key).or_default().push(url);
        }
    }
    let mut explicit_prs: HashMap<String, String> = nodes
        .iter()
        .filter(|n| n["kind"] == "pr")
        .map(|n| {
            (
                n["reference"].as_str().unwrap().to_owned(),
                id(n).to_owned(),
            )
        })
        .collect();
    if focus.is_some() && !attached_prs.is_empty() {
        let urls: BTreeSet<_> = attached_prs
            .values()
            .flatten()
            .map(|url| url.trim_end_matches('/'))
            .collect();
        let mut query = db.prepare("SELECT reference,id FROM mindmap_nodes WHERE project_id=?1 AND kind='pr' AND reference_project='' AND reference IN (SELECT value FROM json_each(?2))")?;
        for row in query.query_map(params![p.id, serde_json::to_string(&urls)?], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })? {
            let (url, id) = row?;
            explicit_prs.insert(url, id);
        }
    }
    let mut automatic = Vec::new();
    let mut automatic_links = HashSet::new();
    for node in &nodes {
        if focus.is_some_and(|focus| focus != id(node)) {
            continue;
        }
        if node["kind"] == "issue" && node["available"] == true {
            let key = (
                node["reference_project"].as_str().unwrap().to_owned(),
                node["reference"].as_str().unwrap().parse::<i64>().unwrap(),
            );
            for url in attached_prs.get(&key).into_iter().flatten() {
                let url = url.trim_end_matches('/');
                let explicit = explicit_prs.get(url).cloned();
                let target = explicit.unwrap_or_else(|| format!("auto:{}:{url}", id(node)));
                if !automatic_links.insert((id(node).to_owned(), target.clone())) {
                    continue;
                }
                if target.starts_with("auto:") {
                    let automatic_node = json!({"id":target,"project_id":p.id,"parent_id":node["id"],"position":i64::MAX,"kind":"pr","title":url,"body":"","reference":url,"reference_project":"","automatic":true,"available":true});
                    budget.charge(&automatic_node)?;
                    automatic.push(automatic_node);
                } else if known.insert(target.clone()) {
                    external.push(ExternalNode::Saved(target.clone()));
                }
                let link = json!({"from":node["id"],"to":target,"kind":"pull-request","description":null,"automatic":true});
                budget.charge(&link)?;
                links.push(link);
            }
        }
    }
    // PR attachments are authoritative even if no issue node has been placed in
    // a map. Prefer a saved node in this map, then the issue's own project map;
    // otherwise expose a live resource-only endpoint, without persisting a node.
    let pr_urls: BTreeSet<_> = nodes
        .iter()
        .filter(|n| n["kind"] == "pr")
        .map(|n| n["reference"].as_str().unwrap())
        .collect();
    let mut pr_issues: BTreeMap<String, Vec<(String, i64, String)>> = BTreeMap::new();
    if !pr_urls.is_empty() {
        let mut query = db.prepare(
            "SELECT DISTINCT rtrim(pr.url,'/'),pr.project_id,pr.issue_number,p.name
            FROM json_each(?1) selected
            JOIN issue_pull_requests pr ON rtrim(pr.url,'/')=selected.value
            JOIN issues i ON i.project_id=pr.project_id AND i.number=pr.issue_number
            JOIN projects p ON p.id=pr.project_id WHERE i.deleted_at IS NULL
            ORDER BY rtrim(pr.url,'/'),pr.project_id,pr.issue_number",
        )?;
        for attachment in query.query_map([serde_json::to_string(&pr_urls)?], |r| {
            Ok((
                r.get::<_, String>(0)?,
                (
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, String>(3)?,
                ),
            ))
        })? {
            let (url, issue) = attachment?;
            pr_issues.entry(url).or_default().push(issue);
        }
    }
    let issue_refs: BTreeSet<_> = pr_issues
        .values()
        .flatten()
        .map(|(project, number, _)| (project, number.to_string()))
        .collect();
    let mut saved_issues = BTreeMap::new();
    if !issue_refs.is_empty() {
        let mut query = db.prepare("SELECT json_extract(selected.value,'$[0]'),json_extract(selected.value,'$[1]'),
            (SELECT id FROM mindmap_nodes
             WHERE kind='issue' AND reference_project=json_extract(selected.value,'$[0]') AND reference=json_extract(selected.value,'$[1]')
             ORDER BY (project_id=?2) DESC,(project_id=json_extract(selected.value,'$[0]')) DESC,project_id,id LIMIT 1)
            FROM json_each(?1) selected")?;
        for row in query.query_map(params![serde_json::to_string(&issue_refs)?, p.id], |r| {
            Ok((
                (r.get::<_, String>(0)?, r.get::<_, String>(1)?),
                r.get::<_, Option<String>>(2)?,
            ))
        })? {
            let (key, saved) = row?;
            saved_issues.insert(key, saved);
        }
    }
    for node in &nodes {
        if focus.is_some_and(|focus| focus != id(node)) {
            continue;
        }
        if node["kind"] != "pr" {
            continue;
        }
        for (project, number, name) in pr_issues
            .get(node["reference"].as_str().unwrap())
            .into_iter()
            .flatten()
        {
            let saved = saved_issues
                .get(&(project.clone(), number.to_string()))
                .and_then(Option::as_ref);
            let issue_id = saved
                .cloned()
                .unwrap_or_else(|| format!("auto-issue:{project}:{number}"));
            if automatic_links.insert((issue_id.clone(), id(node).to_owned())) {
                let link = json!({"from":issue_id,"to":node["id"],"kind":"pull-request","description":null,"automatic":true});
                budget.charge(&link)?;
                links.push(link);
            }
            if known.insert(issue_id.clone()) {
                external.push(if saved.is_some() {
                    ExternalNode::Saved(issue_id)
                } else {
                    ExternalNode::Resource(json!({"id":issue_id,"project_id":project,"project_name":name,"parent_id":null,"position":0,"kind":"issue","title":format!("Issue #{number}"),"body":"","reference":number.to_string(),"reference_project":project,"automatic":true,"resource_only":true,"available":true}))
                });
            }
        }
    }
    let mut external = external_nodes(db, external, mode, &mut budget)?;
    nodes.extend(automatic);
    super::artifacts::enrich_nodes(db, &mut nodes)?;
    super::artifacts::enrich_nodes(db, &mut external)?;
    for node in nodes.iter().chain(external.iter()) {
        budget.charge(&node["artifacts"])?;
    }
    Ok(
        json!({"ok":true,"project":p,"version":version(db,&p.id)?,"body_mode":mode,"nodes":nodes,"external_nodes":external,"links":links}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graph_batches_pr_backreferences_and_external_endpoints() {
        let mut measurements = Vec::new();
        for count in [16, 128] {
            let root = std::env::temp_dir().join(format!(
                "hb-map-backrefs-{}",
                crate::issues::worker::random_id().unwrap()
            ));
            let path = root.join("issues.db");
            let store = crate::issues::Store::open(&path).unwrap();
            store.db.execute_batch(&format!("
                INSERT INTO projects(id,name,next_number) VALUES('named:Map','Map',1),('named:Tasks','Tasks',1),('named:Other','Other',1);
                INSERT INTO agents VALUES('creator','{{}}',0);
                WITH RECURSIVE n(id) AS (VALUES(1) UNION ALL SELECT id+1 FROM n WHERE id<{count})
                INSERT INTO issues(project_id,number,title,body,state,created_by,created_at,updated_at,version,labels,deleted_at)
                SELECT 'named:Tasks',id,'Live '||id,'Body '||id,'open','creator',0,0,7,'[]',CASE WHEN id%8=0 THEN 1 END FROM n;
                INSERT INTO mindmap_nodes(id,project_id,position,kind,title,body,reference,reference_project,created_at,updated_at)
                SELECT 'pr-'||number,'named:Map',number,'pr','Pull request','','https://github.com/example/repo/pull/'||number,'',0,0 FROM issues;
                INSERT INTO mindmap_nodes(id,project_id,position,kind,title,body,reference,reference_project,created_at,updated_at,display_label)
                SELECT 'local-'||number,'named:Map',number+{count},'issue','Placeholder','',CAST(number AS TEXT),'named:Tasks',0,0,'Custom '||number FROM issues WHERE number%4=1;
                INSERT INTO mindmap_nodes(id,project_id,position,kind,title,body,reference,reference_project,created_at,updated_at)
                SELECT 'own-'||number,'named:Tasks',number,'issue','Placeholder','',CAST(number AS TEXT),'named:Tasks',0,0 FROM issues WHERE number%4 IN (1,2);
                INSERT INTO mindmap_nodes(id,project_id,position,kind,title,body,reference,reference_project,created_at,updated_at)
                SELECT 'other-'||number,'named:Other',number,'issue','Placeholder','',CAST(number AS TEXT),'named:Tasks',0,0 FROM issues WHERE number%4 IN (1,2,3);
                INSERT INTO mindmap_nodes(id,project_id,position,kind,title,body,created_at,updated_at)
                SELECT 'text-'||number,'named:Other',number+{count},'markdown','External '||number,'Stored body '||number,0,0 FROM issues;
                INSERT INTO mindmap_links(source,target,kind,created_at)
                SELECT 'pr-'||number,'text-'||number,'related',number FROM issues;
                INSERT INTO issue_pull_requests(project_id,issue_number,url,added_by,created_at)
                SELECT project_id,number,'https://github.com/example/repo/pull/'||number,'creator',number FROM issues;
                INSERT INTO issue_pull_requests(project_id,issue_number,url,added_by,created_at)
                SELECT project_id,number,'https://github.com/example/repo/pull/'||number||'/','creator',number FROM issues;
            ")).unwrap();
            let project = Project {
                id: "named:Map".into(),
                name: "Map".into(),
            };
            let uncrowded = graph(&store.db, &project, BodyMode::None, None).unwrap();
            store.db.execute_batch("
                WITH RECURSIVE n(id) AS (VALUES(1000) UNION ALL SELECT id+1 FROM n WHERE id<9191)
                INSERT INTO issues(project_id,number,title,body,state,created_by,created_at,updated_at,version,labels)
                SELECT 'named:Tasks',id,'Unrelated','','open','creator',0,0,1,'[]' FROM n;
                INSERT INTO issue_pull_requests(project_id,issue_number,url,added_by,created_at)
                SELECT project_id,number,'https://github.com/example/unrelated/pull/'||number,'creator',number FROM issues WHERE number>=1000;
            ").unwrap();
            let mut owner = crate::database::Owner::start(&path).unwrap().unwrap();
            for mode in [BodyMode::None, BodyMode::Preview, BodyMode::Full] {
                let expected = graph(&store.db, &project, mode, None).unwrap();
                if mode == BodyMode::None {
                    assert_eq!(expected, uncrowded);
                }
                let (db, transport) = crate::database::tests::measured_connection(&path);
                let actual = graph(&db, &project, mode, None).unwrap();
                assert_eq!(actual, expected);
                let links = actual["links"].as_array().unwrap();
                assert_eq!(links.len(), count + count - count / 8);
                let nodes: BTreeMap<_, _> = actual["nodes"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .chain(actual["external_nodes"].as_array().unwrap())
                    .map(|n| (id(n), n))
                    .collect();
                let mut external_ids: Vec<_> = (1..=count).map(|n| format!("text-{n}")).collect();
                external_ids.extend((1..=count).filter_map(|n| match n % 4 {
                    2 => Some(format!("own-{n}")),
                    3 => Some(format!("other-{n}")),
                    0 if n % 8 != 0 => Some(format!("auto-issue:named:Tasks:{n}")),
                    _ => None,
                }));
                assert_eq!(
                    actual["external_nodes"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(id)
                        .collect::<Vec<_>>(),
                    external_ids
                );
                for number in 1..=count {
                    assert!(nodes.contains_key(format!("text-{number}").as_str()));
                    if number % 8 == 0 {
                        continue;
                    }
                    let key = match number % 4 {
                        1 => format!("local-{number}"),
                        2 => format!("own-{number}"),
                        3 => format!("other-{number}"),
                        _ => format!("auto-issue:named:Tasks:{number}"),
                    };
                    let node = nodes[key.as_str()];
                    assert_eq!(
                        node["title"],
                        format!(
                            "{} {number}",
                            if number % 4 == 1 { "Custom" } else { "Live" }
                        )
                    );
                    assert_eq!(
                        node["body"],
                        if mode == BodyMode::None {
                            String::new()
                        } else {
                            format!("Body {number}")
                        }
                    );
                    assert_eq!(node["resource_version"], 7);
                    assert!(links.iter().any(|l| l["from"] == key
                        && l["to"] == format!("pr-{number}")
                        && l["automatic"] == true));
                }
                drop(db);
                let (commands, steps) = transport.join().unwrap();
                eprintln!("{count} PR nodes ({mode:?}): {commands} RPCs/{steps} steps");
                measurements.push((count, mode, commands, steps));
            }
            drop(store);
            owner.stop();
            std::fs::remove_dir_all(root).unwrap();
        }
        for (count, mode, commands, steps) in measurements {
            let limit = if mode == BodyMode::Full { 100 } else { 25 };
            assert!(
                commands < limit,
                "{count} PR nodes ({mode:?}) used {commands} RPCs"
            );
            assert!(
                steps < 500 * count as i64,
                "{count} PR nodes scanned {steps} steps with unrelated attachments"
            );
        }
    }

    #[test]
    fn graph_batches_live_issue_fields_and_attached_pull_requests() {
        let mut measurements = Vec::new();
        for count in [16, 128] {
            let root = std::env::temp_dir().join(format!(
                "hb-map-live-{}",
                crate::issues::worker::random_id().unwrap()
            ));
            let path = root.join("issues.db");
            let store = crate::issues::Store::open(&path).unwrap();
            store.db.execute_batch(&format!("
                INSERT INTO projects(id,name,next_number) VALUES('named:Map','Map',1),('named:Tasks','Tasks',1);
                INSERT INTO agents VALUES('creator','{{}}',0);
                WITH RECURSIVE n(id) AS (VALUES(1) UNION ALL SELECT id+1 FROM n WHERE id<{count})
                INSERT INTO issues(project_id,number,title,body,state,created_by,created_at,updated_at,version,labels,deleted_at)
                SELECT 'named:Tasks',id,'Live '||id,'','open','creator',0,0,7,'[\"work\"]',CASE WHEN id%8=0 THEN 1 END FROM n;
                INSERT INTO mindmap_nodes(id,project_id,position,kind,title,body,reference,reference_project,created_at,updated_at,display_label)
                SELECT 'node-'||number,'named:Map',number,'issue','Placeholder','',CAST(number AS TEXT),'named:Tasks',0,0,CASE WHEN number%4=1 THEN 'Custom '||number END FROM issues;
                INSERT INTO issue_pull_requests(project_id,issue_number,url,added_by,created_at)
                SELECT project_id,number,'https://github.com/example/repo/pull/'||number,'creator',number FROM issues;
            ")).unwrap();
            let body = format!("\0{}", "🧭".repeat(600));
            store
                .db
                .execute("UPDATE issues SET body=?1", [&body])
                .unwrap();
            let project = Project {
                id: "named:Map".into(),
                name: "Map".into(),
            };
            let mut owner = crate::database::Owner::start(&path).unwrap().unwrap();
            for mode in [BodyMode::None, BodyMode::Preview, BodyMode::Full] {
                let expected = graph(&store.db, &project, mode, None).unwrap();
                let (db, transport) = crate::database::tests::measured_connection(&path);
                let actual = graph(&db, &project, mode, None).unwrap();
                assert_eq!(actual, expected);
                assert_eq!(actual["links"].as_array().unwrap().len(), count - count / 8);
                for node in actual["nodes"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|n| n["kind"] == "issue")
                {
                    let number: usize = node["reference"].as_str().unwrap().parse().unwrap();
                    assert_eq!(node["reference_project_name"], "Tasks");
                    assert_eq!(node["available"], !number.is_multiple_of(8));
                    if number.is_multiple_of(8) {
                        continue;
                    }
                    assert_eq!(node["original_title"], format!("Live {number}"));
                    assert_eq!(
                        node["title"],
                        if number % 4 == 1 {
                            format!("Custom {number}")
                        } else {
                            format!("Live {number}")
                        }
                    );
                    assert_eq!(node["labels"], json!(["work"]));
                    assert_eq!(node["resource_version"], 7);
                    assert_eq!(
                        node["body"],
                        match mode {
                            BodyMode::None => String::new(),
                            BodyMode::Preview => body.chars().take(512).collect(),
                            BodyMode::Full => body.clone(),
                        }
                    );
                }
                drop(db);
                let (commands, steps) = transport.join().unwrap();
                eprintln!("{count} issue nodes ({mode:?}): {commands} RPCs/{steps} steps");
                measurements.push((count, commands));
            }
            drop(store);
            owner.stop();
            std::fs::remove_dir_all(root).unwrap();
        }
        for (count, commands) in measurements {
            assert!(commands < 30, "{count} issue nodes used {commands} RPCs");
        }
    }
}
