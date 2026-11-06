//! File operations edit the manifest; untouched assets never leave chunk storage.
use super::*;

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum FileChange {
    Save { text: String },
    Move { destination: String },
    Delete,
}

pub(crate) fn file_list(database: &Path, page: &Page) -> Result<Vec<(String, u64)>> {
    if page.syncing {
        return Err(Error::conflict("Wait for this page to finish syncing."));
    }
    let db = Store::open(database)?.into_database();
    Ok(manifest(&db, &page.deployment)?
        .ok_or_else(|| Error::conflict("Files are syncing."))?
        .iter()
        .map(|(path, file)| (path.clone(), file.size))
        .collect())
}

pub(crate) fn edit_file(
    database: &Path,
    name: &str,
    deployment: &str,
    updated_at: i64,
    path: &str,
    change: FileChange,
) -> Result<Page> {
    valid_name(name)?;
    let page = {
        let db = Store::open(database)?.into_database();
        let _lock = lock(database)?;
        let page = view_in(&db, name)?;
        if page.syncing || page.deployment != deployment || page.updated_at != updated_at {
            return Err(Error::conflict(
                "This page changed. Reopen Files to see the current version.",
            ));
        }
        let mut files = (*manifest(&db, deployment)?
            .ok_or_else(|| Error::conflict("Files are syncing."))?)
        .clone();
        let asset = files
            .remove(path)
            .ok_or_else(|| Error::invalid("File not found"))?;
        let mut chunks = BTreeMap::new();
        match change {
            FileChange::Save { text } => {
                if asset.size > 1024 * 1024
                    || text.len() > 1024 * 1024
                    || !matches!(
                        path.rsplit('.')
                            .next()
                            .unwrap_or("")
                            .to_ascii_lowercase()
                            .as_str(),
                        "md" | "markdown" | "html" | "htm" | "css" | "js" | "json" | "txt" | "svg"
                    )
                {
                    return Err(Error::invalid("Choose a text file up to 1 MiB."));
                }
                let mut asset = Asset {
                    size: text.len() as u64,
                    chunks: Vec::new(),
                };
                for bytes in text.as_bytes().chunks(CHUNK_SIZE) {
                    let hash = hey_boss_sync::digest(bytes);
                    write_chunk(database, &hash, bytes)?;
                    chunks.insert(hash.clone(), bytes.len());
                    asset.chunks.push(hash);
                }
                files.insert(path.to_owned(), asset);
            }
            FileChange::Move { destination } => {
                if path == "index.html"
                    || destination == path
                    || destination.starts_with(&format!("{path}/"))
                    || !valid_path(&destination)
                    || files.contains_key(&destination)
                    || destination
                        .match_indices('/')
                        .any(|(i, _)| files.contains_key(&destination[..i]))
                    || files
                        .range(format!("{destination}/")..)
                        .next()
                        .is_some_and(|(p, _)| p.starts_with(&format!("{destination}/")))
                {
                    return Err(Error::invalid(
                        "Choose an unused relative file path. index.html must stay at the root.",
                    ));
                }
                files.insert(destination, asset);
            }
            FileChange::Delete if path == "index.html" => {
                return Err(Error::invalid("index.html is required."));
            }
            FileChange::Delete => {}
        }
        commit_manifest(&db, name, files, chunks, Some((deployment, updated_at)))?
    };
    // Provider synchronization can run for minutes. Do not give this editor a
    // newer writer's revision as the base for its next save.
    if page.publications.iter().any(|p| p.auto_sync && !p.synced) {
        publishing::sync_automatic(database, name)?;
    }
    Ok(page)
}
