use serde::{Deserialize, Serialize};
use std::io;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Source {
    pub kind: String,
    pub repository: Option<String>,
    pub commit: Option<String>,
    pub ancestors: Vec<String>,
    pub build: String,
}

impl Source {
    pub fn same_release(&self, other: &Self) -> bool {
        self.kind == other.kind
            && self.repository == other.repository
            && self.commit == other.commit
            && self.build == other.build
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Receipt {
    pub generation: u64,
    pub source: Source,
    pub installed_at: u64,
}

pub fn authorize(
    source: &Source,
    previous: Option<&Receipt>,
    observed: Option<u64>,
) -> io::Result<()> {
    if !matches!(source.kind.as_str(), "main" | "development") {
        return Err(io::Error::other("Unknown source provenance"));
    }
    if source.kind == "main" && source.commit.is_none() {
        return Err(io::Error::other("Committed main requires a source commit"));
    }
    if source.kind == "development" && previous.map(|r| r.generation) != observed {
        return Err(io::Error::other(
            "An intervening upgrade superseded this development snapshot; stage it again explicitly",
        ));
    }
    if let Some(previous) = previous {
        if let Some(commit) = &previous.source.commit {
            if source.repository != previous.source.repository || !source.ancestors.contains(commit)
            {
                return Err(io::Error::other(format!(
                    "Refusing stale or unrelated source: installed commit {commit} is not an ancestor of the requested source"
                )));
            }
        } else if source.kind == "development" && previous.source.repository != source.repository {
            return Err(io::Error::other("Refusing unrelated development source"));
        }
        if source.kind == "main"
            && previous.source.kind == "main"
            && source.commit == previous.source.commit
            && source.build != previous.source.build
        {
            return Err(io::Error::other(
                "The same main commit cannot identify two different builds",
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(commit: &str, ancestors: &[&str], kind: &str) -> Source {
        Source {
            kind: kind.into(),
            repository: Some("repo".into()),
            commit: Some(commit.into()),
            ancestors: ancestors.iter().map(|s| (*s).into()).collect(),
            build: commit.into(),
        }
    }
    fn receipt(source: Source) -> Receipt {
        Receipt {
            generation: 2,
            source,
            installed_at: 0,
        }
    }

    #[test]
    fn staged_old_main_cannot_replace_verified_descendant_even_with_force() {
        let newer = receipt(source("new", &["new", "old"], "main"));
        assert!(authorize(&source("old", &["old"], "main"), Some(&newer), Some(2)).is_err());
    }

    #[test]
    fn queued_new_main_can_follow_an_intervening_ancestor_install() {
        let old = receipt(source("old", &["old"], "main"));
        assert!(authorize(&source("new", &["new", "old"], "main"), Some(&old), None).is_ok());
    }

    #[test]
    fn development_snapshot_cannot_replace_an_intervening_install() {
        let main = receipt(source("main", &["main"], "main"));
        let mut dev = source("main", &["main"], "development");
        dev.build = "dirty".into();
        assert!(authorize(&dev, Some(&main), Some(1)).is_err());
        assert!(authorize(&dev, Some(&main), Some(2)).is_ok());
    }

    #[test]
    fn development_based_on_old_commit_is_refused() {
        let new = receipt(source("new", &["new", "old"], "main"));
        assert!(authorize(&source("old", &["old"], "development"), Some(&new), Some(2)).is_err());
    }

    #[test]
    fn diverged_or_unknown_main_fails_closed() {
        let old = receipt(source("old", &["old"], "main"));
        assert!(authorize(&source("other", &["other"], "main"), Some(&old), Some(2)).is_err());
        let mut other_repo = source("new", &["new", "old"], "main");
        other_repo.repository = Some("different".into());
        assert!(authorize(&other_repo, Some(&old), Some(2)).is_err());
    }

    #[test]
    fn bootstrap_records_previously_unknown_installation_and_main_can_restore_development() {
        let main = source("main", &["main"], "main");
        assert!(authorize(&main, None, None).is_ok());
        let mut dev = main.clone();
        dev.kind = "development".into();
        dev.build = "dirty".into();
        assert!(authorize(&main, Some(&receipt(dev)), None).is_ok());
    }
}
