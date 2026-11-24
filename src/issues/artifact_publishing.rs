//! Markdown uses the same configured publisher contract as static Pages.
use super::artifacts;
use crate::{
    artifacts::Operation,
    database::Connection,
    issues::{Error, Project, Result},
    pages::publishing::{self, Content, Provider},
};
use rusqlite::params;
use serde_json::{Value, json};
use std::{fs, os::unix::fs::OpenOptionsExt, path::Path};

pub(super) fn migrate(db: &Connection) -> Result<()> {
    if db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='artifact_exports')",
        [],
        |r| r.get::<_, bool>(0),
    )? {
        return Ok(());
    }
    db.execute_batch("CREATE TABLE IF NOT EXISTS artifact_exports(project_id TEXT NOT NULL,artifact_id TEXT NOT NULL,provider TEXT NOT NULL,audience TEXT NOT NULL,remote_id TEXT NOT NULL DEFAULT '',url TEXT NOT NULL DEFAULT '',version TEXT NOT NULL DEFAULT '',content TEXT NOT NULL DEFAULT '',auto_sync INTEGER NOT NULL DEFAULT 0,error TEXT,PRIMARY KEY(project_id,artifact_id,provider));")?;
    Ok(())
}
fn fingerprint(artifact: &Value) -> String {
    hey_boss_sync::digest(
        json!([artifact["title"], artifact["body"]])
            .to_string()
            .as_bytes(),
    )
}
pub(super) fn list(db: &Connection, project: &str, id: &str, artifact: &Value) -> Result<Value> {
    let wanted = fingerprint(artifact);
    let rows = db.query_collect("SELECT provider,audience,remote_id,url,version,content,auto_sync,error FROM artifact_exports WHERE project_id=?1 AND artifact_id=?2 ORDER BY provider",params![project,id], |r| {
        let content: String = r.get(5)?;
        let error: Option<String> = r.get(7)?;
        Ok::<_,rusqlite::Error>(json!({"provider":r.get::<_,String>(0)?,"audience":r.get::<_,String>(1)?,"remote_id":r.get::<_,String>(2)?,"url":r.get::<_,String>(3)?,"version":r.get::<_,String>(4)?,"auto_sync":r.get::<_,bool>(6)?,"synced":content==wanted && error.is_none(),"error":error}))
    })?;
    Ok(json!(rows))
}

pub(super) fn execute(db: &Connection, project: &Project, operation: &Operation) -> Result<Value> {
    match operation {
        Operation::Providers => {
            let providers: Vec<_> = publishing::providers()?.into_iter()
                .filter(|(_,p)| p.content == Content::Artifact)
                .map(|(name,p)| json!({"name":name,"audience":p.audience,"auto_sync":p.automatic()})).collect();
            Ok(json!({"ok":true,"providers":providers,"config":publishing::config_path()?}))
        }
        Operation::Share {
            id,
            provider,
            link,
            auto_sync,
        } => {
            let configured = publishing::providers()?;
            let definition = configured
                .get(provider)
                .filter(|p| p.content == Content::Artifact)
                .ok_or_else(|| {
                    Error::invalid(format!(
                        "Artifact publisher {provider} is not configured in publishing.yaml"
                    ))
                })?;
            share(
                db,
                project,
                id,
                provider,
                definition,
                link.as_deref(),
                *auto_sync,
            )?;
            artifacts::view(db, project, id)
        }
        Operation::Sync { id } => {
            synchronize(db, project, id, false)?;
            artifacts::view(db, project, id)
        }
        _ => unreachable!(),
    }
}

fn share(
    db: &Connection,
    project: &Project,
    id: &str,
    provider: &str,
    definition: &Provider,
    link: Option<&str>,
    auto_sync: Option<bool>,
) -> Result<()> {
    definition.validate()?;
    if definition.content != Content::Artifact {
        return Err(Error::invalid(
            "This provider publishes Pages, not Markdown artifacts",
        ));
    }
    let root = Path::new(
        db.path()
            .ok_or_else(|| Error::invalid("Publishing requires a persistent store"))?,
    )
    .with_extension("artifact-publishing");
    crate::pages::private_dir(&root)?;
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(root.join(hey_boss_sync::digest(
            format!("{}:{id}:{provider}", project.id).as_bytes(),
        )))?;
    lock.lock()?;
    let artifact = artifacts::get(db, &project.id, id)?;
    let publications = list(db, &project.id, id, &artifact)?;
    let existing = publications
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["provider"] == provider);
    let automatic = auto_sync
        .or_else(|| existing.and_then(|p| p["auto_sync"].as_bool()))
        .unwrap_or_else(|| definition.automatic());
    if link.is_none() && existing.is_some_and(|p| p["remote_id"] == "") {
        return Err(Error::invalid(
            "Initial publication has an uncertain result. Find its remote link and use Share → Existing link; creation was not repeated",
        ));
    }
    let body = artifact["body"].as_str().unwrap();
    // Private attachment routes cannot be opened by a remote document service.
    if crate::artifacts::import::destinations(body)
        .iter()
        .any(|(_, url)| url.starts_with("/attachments/"))
    {
        return Err(Error::invalid(
            "This artifact contains private attachments. Replace their links with externally accessible URLs before publishing",
        ));
    }
    let title = artifact["title"].as_str().unwrap();
    if let Some(link) = link {
        let remote = definition.run(&definition.read, Path::new(""), title, link, "")?;
        db.execute("INSERT INTO artifact_exports(project_id,artifact_id,provider,audience,remote_id,url,version,auto_sync) VALUES(?1,?2,?3,?4,?5,?6,?7,?8) ON CONFLICT(project_id,artifact_id,provider) DO UPDATE SET remote_id=excluded.remote_id,url=excluded.url,version=excluded.version,content='',error=NULL,auto_sync=excluded.auto_sync,audience=excluded.audience",params![project.id,id,provider,definition.audience,remote.id,remote.url,remote.version,automatic])?;
    } else {
        db.execute("INSERT INTO artifact_exports(project_id,artifact_id,provider,audience,auto_sync) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(project_id,artifact_id,provider) DO UPDATE SET auto_sync=excluded.auto_sync", params![project.id,id,provider,definition.audience,automatic])?;
    }
    let publications = list(db, &project.id, id, &artifact)?;
    let publication = publications
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["provider"] == provider)
        .unwrap();
    if publication["synced"] == true {
        return Ok(());
    }
    let result = (|| -> Result<()> {
        let stage = tempfile::Builder::new()
            .prefix("upload-")
            .tempdir_in(&root)?;
        let source = stage.path().join("document.md");
        fs::write(&source, body)?;
        let remote_id = publication["remote_id"].as_str().unwrap();
        let remote = definition.run(
            if remote_id.is_empty() {
                &definition.create
            } else {
                &definition.update
            },
            &source,
            title,
            remote_id,
            publication["version"].as_str().unwrap(),
        )?;
        if !remote_id.is_empty()
            && (remote.id != remote_id || remote.url != publication["url"].as_str().unwrap())
        {
            return Err(Error::invalid(
                "Publisher changed the saved target; original link was retained",
            ));
        }
        db.execute("UPDATE artifact_exports SET remote_id=?4,url=?5,version=?6,content=?7,error=NULL WHERE project_id=?1 AND artifact_id=?2 AND provider=?3",params![project.id,id,provider,remote.id,remote.url,remote.version,fingerprint(&artifact)])?;
        Ok(())
    })();
    if let Err(error) = &result {
        db.execute("UPDATE artifact_exports SET error=?4 WHERE project_id=?1 AND artifact_id=?2 AND provider=?3", params![project.id,id,provider,error.message])?;
    }
    result
}

pub(super) fn synchronize(
    db: &Connection,
    project: &Project,
    id: &str,
    automatic: bool,
) -> Result<()> {
    let artifact = artifacts::get(db, &project.id, id)?;
    let publications = list(db, &project.id, id, &artifact)?;
    let targets: Vec<_> = publications
        .as_array()
        .unwrap()
        .iter()
        .filter(|p| p["synced"] != true && (!automatic || p["auto_sync"] == true))
        .collect();
    if targets.is_empty() {
        return Ok(());
    }
    let configured = publishing::providers();
    for publication in targets {
        let provider = publication["provider"].as_str().unwrap();
        let result = match &configured {
            Ok(providers) => match providers.get(provider) {
                Some(definition) => share(db, project, id, provider, definition, None, None),
                None => Err(Error::invalid(format!(
                    "Publisher {provider} is not configured on this machine"
                ))),
            },
            Err(error) => Err(Error::invalid(&error.message)),
        };
        if let Err(error) = result {
            db.execute("UPDATE artifact_exports SET error=?4 WHERE project_id=?1 AND artifact_id=?2 AND provider=?3", params![project.id,id,provider,error.message])?;
        }
    }
    Ok(())
}
