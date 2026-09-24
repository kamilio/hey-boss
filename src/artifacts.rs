//! Persistent project Markdown documents, independent of their referring resources.
use crate::issues::{BODY_LIMIT, Error, Result, identifier};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    Preview {
        body: String,
    },
    List {
        query: Option<String>,
        #[serde(default)]
        archived: bool,
        #[serde(default)]
        offset: usize,
    },
    View {
        id: String,
    },
    Create {
        title: String,
        body: String,
        issue: Option<i64>,
        node: Option<String>,
    },
    Edit {
        id: String,
        title: Option<String>,
        body: Option<String>,
        if_version: i64,
    },
    Archive {
        id: String,
        archived: bool,
        if_version: i64,
    },
    Delete {
        id: String,
        if_version: i64,
    },
    Comment {
        id: String,
        body: String,
        quote: Option<String>,
        prefix: Option<String>,
        suffix: Option<String>,
        parent: Option<i64>,
    },
    Resolve {
        id: String,
        comment_id: i64,
        resolved: bool,
    },
    Link {
        id: String,
        issue: Option<i64>,
        node: Option<String>,
    },
    Unlink {
        id: String,
        issue: Option<i64>,
        node: Option<String>,
    },
    Links {
        issue: Option<i64>,
        node: Option<String>,
    },
}
impl Operation {
    pub fn writes(&self) -> bool {
        !matches!(
            self,
            Self::Preview { .. } | Self::List { .. } | Self::View { .. } | Self::Links { .. }
        )
    }
    pub fn validate(&self) -> Result<()> {
        let text = |s: &str| {
            if s.len() > BODY_LIMIT {
                Err(Error::invalid("Markdown must be at most 1 MiB"))
            } else {
                Ok(())
            }
        };
        match self {
            Self::Preview { body } => text(body)?,
            Self::List { query, offset, .. } => {
                if let Some(q) = query {
                    text(q)?;
                }
                if *offset > 1_000_000 {
                    return Err(Error::invalid("Invalid offset"));
                }
            }
            Self::Create {
                title,
                body,
                issue,
                node,
            } => {
                identifier(title, "title", 512)?;
                text(body)?;
                target(*issue, node.as_deref(), false)?;
            }
            Self::Edit {
                id,
                title,
                body,
                if_version,
            } => {
                identifier(id, "artifact ID", 128)?;
                if let Some(t) = title {
                    identifier(t, "title", 512)?;
                }
                if let Some(b) = body {
                    text(b)?;
                }
                revision(*if_version)?;
            }
            Self::Archive { id, if_version, .. } | Self::Delete { id, if_version } => {
                identifier(id, "artifact ID", 128)?;
                revision(*if_version)?;
            }
            Self::Comment {
                id,
                body,
                quote,
                prefix,
                suffix,
                parent,
            } => {
                identifier(id, "artifact ID", 128)?;
                text(body)?;
                if body.trim().is_empty() {
                    return Err(Error::invalid("Comment must not be blank"));
                }
                for value in [quote, prefix, suffix].into_iter().flatten() {
                    if value.len() > 8192 {
                        return Err(Error::invalid("Comment anchor must be at most 8 KiB"));
                    }
                }
                if quote.as_ref().is_some_and(|q| q.is_empty()) || parent.is_some_and(|p| p <= 0) {
                    return Err(Error::invalid("Invalid comment anchor or parent"));
                }
            }
            Self::Resolve { id, comment_id, .. } => {
                identifier(id, "artifact ID", 128)?;
                if *comment_id <= 0 {
                    return Err(Error::invalid("Invalid comment ID"));
                }
            }
            Self::Link { id, issue, node } | Self::Unlink { id, issue, node } => {
                identifier(id, "artifact ID", 128)?;
                target(*issue, node.as_deref(), true)?;
            }
            Self::Links { issue, node } => target(*issue, node.as_deref(), true)?,
            Self::View { id } => identifier(id, "artifact ID", 128)?,
        }
        Ok(())
    }
}
fn revision(v: i64) -> Result<()> {
    if v < 1 {
        Err(Error::invalid("A current artifact revision is required"))
    } else {
        Ok(())
    }
}
fn target(issue: Option<i64>, node: Option<&str>, required: bool) -> Result<()> {
    if issue.is_some() && node.is_some()
        || required && issue.is_none() && node.is_none()
        || issue.is_some_and(|n| n <= 0)
    {
        return Err(Error::invalid("Specify one issue or mindmap node"));
    }
    if let Some(n) = node {
        identifier(n, "mindmap node", 2048)?;
    }
    Ok(())
}
