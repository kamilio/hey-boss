//! External publishers are configured commands, not bundled hosting clients.
use super::*;
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use std::os::unix::ffi::OsStrExt;

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Provider {
    pub command: String,
    pub audience: String,
    create: Vec<String>,
    read: Vec<String>,
    update: Vec<String>,
    fields: Fields,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Fields {
    id: String,
    url: String,
    version: String,
}

pub fn config_path() -> Result<PathBuf> {
    let home =
        PathBuf::from(std::env::var_os("HOME").ok_or_else(|| Error::invalid("HOME is missing"))?);
    let state = crate::fleet::socket_path()?.parent().unwrap().to_owned();
    Ok(crate::fleet::config_directory(&home, &state).join("publishing.yaml"))
}
pub fn providers() -> Result<BTreeMap<String, Provider>> {
    let path = config_path()?;
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(e) => return Err(e.into()),
    };
    let providers: BTreeMap<String, Provider> = serde_yaml_ng::from_slice(&bytes)
        .map_err(|e| Error::invalid(format!("{}: {e}", path.display())))?;
    for (name, provider) in &providers {
        valid_name(name)?;
        provider.validate()?;
    }
    Ok(providers)
}
impl Provider {
    fn validate(&self) -> Result<()> {
        if self.command.trim().is_empty()
            || !matches!(
                self.audience.as_str(),
                "public" | "organization" | "private"
            )
            || [&self.create, &self.read, &self.update]
                .iter()
                .any(|args| args.is_empty())
            || [&self.fields.id, &self.fields.url, &self.fields.version]
                .iter()
                .any(|p| !p.starts_with('/'))
        {
            return Err(Error::invalid(
                "Publisher needs command, audience (public, organization or private), create/read/update arguments, and JSON field pointers",
            ));
        }
        Ok(())
    }
    fn run(
        &self,
        args: &[String],
        source: &Path,
        name: &str,
        id: &str,
        version: &str,
    ) -> Result<Remote> {
        self.validate()?;
        let args: Vec<_> = args
            .iter()
            .map(|arg| {
                STANDARD.encode(match arg.as_str() {
                    "{source}" => source.as_os_str().as_bytes(),
                    "{name}" => name.as_bytes(),
                    "{id}" => id.as_bytes(),
                    "{version}" => version.as_bytes(),
                    _ => arg.as_bytes(),
                })
            })
            .collect();
        let result = crate::utilities::execute(
            &json!({"utility":{"command":self.command},"args":args,"stdin":""}),
        );
        if result["ok"] != true {
            return Err(Error::invalid(
                result["error"]
                    .as_str()
                    .unwrap_or("Publisher could not run"),
            ));
        }
        if result["code"] != 0 {
            let stderr = crate::utilities::decode(&result["stderr"])?;
            return Err(Error::invalid(format!(
                "Publisher failed: {}",
                String::from_utf8_lossy(&stderr[..stderr.len().min(4096)]).trim()
            )));
        }
        let output: Value = serde_json::from_slice(&crate::utilities::decode(&result["stdout"])?)?;
        let field = |pointer: &str| -> Result<String> {
            output
                .pointer(pointer)
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty() && s.len() <= 4096 && !s.chars().any(char::is_control))
                .map(str::to_owned)
                .ok_or_else(|| {
                    Error::invalid(format!(
                        "Publisher response is missing string field {pointer}"
                    ))
                })
        };
        let remote = Remote {
            id: field(&self.fields.id)?,
            url: field(&self.fields.url)?,
            version: field(&self.fields.version)?,
        };
        let url = reqwest::Url::parse(&remote.url)
            .map_err(|_| Error::invalid("Publisher returned an invalid URL"))?;
        if !matches!(url.scheme(), "http" | "https")
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return Err(Error::invalid(
                "Publisher URL must be an HTTP(S) link without credentials",
            ));
        }
        Ok(remote)
    }
}
struct Remote {
    id: String,
    url: String,
    version: String,
}

pub(super) fn migrate(db: &Connection) -> Result<()> {
    if db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='page_exports')",
        [],
        |r| r.get::<_, bool>(0),
    )? {
        return Ok(());
    }
    db.execute_batch("CREATE TABLE IF NOT EXISTS page_exports(name TEXT NOT NULL,generation TEXT NOT NULL,provider TEXT NOT NULL,audience TEXT NOT NULL,remote_id TEXT NOT NULL DEFAULT '',url TEXT NOT NULL DEFAULT '',version TEXT NOT NULL DEFAULT '',deployment TEXT NOT NULL DEFAULT '',auto_sync INTEGER NOT NULL DEFAULT 1,error TEXT,PRIMARY KEY(name,generation,provider));")?;
    Ok(())
}

#[derive(Debug, Serialize)]
pub struct Publication {
    pub provider: String,
    pub audience: String,
    pub remote_id: String,
    pub url: String,
    pub version: String,
    pub deployment: String,
    pub auto_sync: bool,
    pub error: Option<String>,
    pub synced: bool,
}
pub(super) fn list_in(db: &Connection, name: &str, wanted: &str) -> Result<Vec<Publication>> {
    Ok(db.query_collect("SELECT e.provider,e.audience,e.remote_id,e.url,e.version,e.deployment,e.auto_sync,e.error FROM page_exports e JOIN published_pages p ON p.name=e.name AND p.generation=e.generation WHERE p.name=?1 AND p.deleted=0 ORDER BY e.provider", [name], |r| {
        let deployment: String = r.get(5)?;
        let error: Option<String> = r.get(7)?;
        Ok::<_, rusqlite::Error>(Publication {
            provider:r.get(0)?, audience:r.get(1)?, remote_id:r.get(2)?, url:r.get(3)?, version:r.get(4)?,
            synced: deployment == wanted && error.is_none(), deployment, auto_sync:r.get(6)?, error,
        })
    })?)
}

pub fn share(
    database: &Path,
    name: &str,
    provider: &str,
    link: Option<&str>,
    auto_sync: bool,
) -> Result<Page> {
    let providers = providers()?;
    let definition = providers.get(provider).ok_or_else(|| {
        Error::invalid(format!(
            "Publisher {provider} is not configured in {}",
            config_path()
                .map(|p| p.display().to_string())
                .unwrap_or_default()
        ))
    })?;
    share_with(database, name, provider, definition, link, auto_sync)?;
    view(database, name)
}

fn share_with(
    database: &Path,
    name: &str,
    provider: &str,
    definition: &Provider,
    link: Option<&str>,
    auto_sync: bool,
) -> Result<()> {
    valid_name(name)?;
    valid_name(provider)?;
    definition.validate()?;
    let db = Store::open(database)?.into_database();
    private_dir(&root(database))?;
    // Serialize this target locally without holding the content/database writer during network I/O.
    let lock_path = root(database).join(format!(
        ".publisher-{}",
        hey_boss_sync::digest(format!("{name}:{provider}").as_bytes())
    ));
    let lease = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(lock_path)?;
    lease.lock()?;
    let page = view_in(&db, name)?;
    if page.syncing {
        return Err(Error::invalid(
            "Page files are still syncing; retry when they arrive",
        ));
    }
    let generation: String = db.query_row(
        "SELECT generation FROM published_pages WHERE name=?1",
        [name],
        |r| r.get(0),
    )?;
    let existing = page.publications.iter().find(|p| p.provider == provider);
    if link.is_none() && existing.is_some_and(|p| p.remote_id.is_empty()) {
        return Err(Error::invalid(
            "Initial publication has an uncertain result. Find its link in the provider and use pages share --link; create was not repeated",
        ));
    }
    if let Some(link) = link {
        let remote = definition.run(&definition.read, Path::new(""), name, link, "")?;
        db.execute("INSERT INTO page_exports(name,generation,provider,audience,remote_id,url,version,auto_sync) VALUES(?1,?2,?3,?4,?5,?6,?7,?8) ON CONFLICT(name,generation,provider) DO UPDATE SET remote_id=excluded.remote_id,url=excluded.url,version=excluded.version,deployment='',error=NULL,auto_sync=excluded.auto_sync,audience=excluded.audience", params![name,generation,provider,definition.audience,remote.id,remote.url,remote.version,auto_sync])?;
    } else {
        db.execute("INSERT INTO page_exports(name,generation,provider,audience,auto_sync) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(name,generation,provider) DO UPDATE SET auto_sync=excluded.auto_sync", params![name,generation,provider,definition.audience,auto_sync])?;
    }
    let publication = list_in(&db, name, &page.deployment)?
        .into_iter()
        .find(|p| p.provider == provider)
        .ok_or_else(|| Error::invalid("Page changed while linking"))?;
    if publication.synced {
        return Ok(());
    }
    let result = (|| -> Result<()> {
        struct Stage(PathBuf);
        impl Drop for Stage {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
        let stage =
            Stage(root(database).join(format!(".upload-{}", crate::issues::worker::random_id()?)));
        {
            let _content_lock = lock(database)?;
            let current = view_in(&db, name)?;
            let current_generation: String = db.query_row(
                "SELECT generation FROM published_pages WHERE name=?1",
                [name],
                |r| r.get(0),
            )?;
            if current.deployment != page.deployment
                || current.syncing
                || current_generation != generation
            {
                return Err(Error::invalid(
                    "Page changed before upload; retry to publish the latest content",
                ));
            }
            download(database, name, &stage.0)?;
        }
        let source = stage.0.canonicalize()?;
        let remote = definition.run(
            if publication.remote_id.is_empty() {
                &definition.create
            } else {
                &definition.update
            },
            &source,
            name,
            &publication.remote_id,
            &publication.version,
        )?;
        if !publication.remote_id.is_empty()
            && (remote.id != publication.remote_id || remote.url != publication.url)
        {
            return Err(Error::invalid(
                "Publisher update changed the saved target; original link was retained",
            ));
        }
        db.execute("UPDATE page_exports SET remote_id=?4,url=?5,version=?6,deployment=?7,error=NULL WHERE name=?1 AND generation=?2 AND provider=?3", params![name,generation,provider,remote.id,remote.url,remote.version,page.deployment])?;
        Ok(())
    })();
    if let Err(error) = &result {
        db.execute(
            "UPDATE page_exports SET error=?4 WHERE name=?1 AND generation=?2 AND provider=?3",
            params![name, generation, provider, error.message],
        )?;
    }
    result
}

fn synchronize(database: &Path, name: &str, automatic: bool) -> Result<Page> {
    let page = view(database, name)?;
    let targets: Vec<_> = page
        .publications
        .iter()
        .filter(|p| !p.synced && (!automatic || p.auto_sync))
        .collect();
    if targets.is_empty() {
        return Ok(page);
    }
    let configured = providers();
    for publication in targets {
        let result = match &configured {
            Ok(providers) => match providers.get(&publication.provider) {
                Some(definition) => share_with(
                    database,
                    name,
                    &publication.provider,
                    definition,
                    None,
                    publication.auto_sync,
                ),
                None => Err(Error::invalid(format!(
                    "Publisher {} is not configured on this machine",
                    publication.provider
                ))),
            },
            Err(error) => Err(Error::invalid(error.message.clone())),
        };
        if let Err(error) = result {
            let db = Store::open(database)?.into_database();
            db.execute("UPDATE page_exports SET error=?3 WHERE name=?1 AND provider=?2 AND generation=(SELECT generation FROM published_pages WHERE name=?1)", params![name,publication.provider,error.message])?;
        }
    }
    view(database, name)
}
pub fn sync(database: &Path, name: &str) -> Result<Page> {
    synchronize(database, name, false)
}
pub(super) fn sync_automatic(database: &Path, name: &str) -> Result<Page> {
    synchronize(database, name, true)
}

#[cfg(test)]
#[path = "publishing_tests.rs"]
mod tests;
