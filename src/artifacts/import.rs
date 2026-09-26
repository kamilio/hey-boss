//! Import caller-owned files without making the server read caller-supplied paths.
use crate::{
    attachments,
    issues::{Error, Result},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use pulldown_cmark::{Event, LinkType, Parser, Tag};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    ops::Range,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct File {
    pub destination: String,
    pub name: String,
    pub data: String,
}

fn destination_span(source: &str, start: usize) -> Range<usize> {
    let bytes = source.as_bytes();
    let mut i = start;
    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    let angle = bytes.get(i) == Some(&b'<');
    if angle {
        i += 1;
    }
    let begin = i;
    let mut depth = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' if i + 1 < bytes.len() => {
                i += 2;
                continue;
            }
            b'>' if angle => break,
            b'(' if !angle => depth += 1,
            b')' if !angle && depth == 0 => break,
            b')' if !angle => depth -= 1,
            c if !angle && c.is_ascii_whitespace() => break,
            _ => {}
        }
        i += 1;
    }
    begin..i
}

fn decoded_markdown(raw: &str) -> String {
    let mut result = String::new();
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' && chars.peek().is_some_and(|c| c.is_ascii_punctuation()) {
            result.push(chars.next().unwrap());
        } else {
            result.push(c);
        }
    }
    html_escape::decode_html_entities(&result).into_owned()
}

pub fn destinations(body: &str) -> Vec<(Range<usize>, String)> {
    let parser = Parser::new_ext(body, crate::markdown::parser_options());
    let mut parser = parser.into_offset_iter();
    let mut spans = BTreeMap::new();
    while let Some((event, range)) = parser.next() {
        let (kind, destination, id) = match event {
            Event::Start(
                Tag::Link {
                    link_type,
                    dest_url,
                    id,
                    ..
                }
                | Tag::Image {
                    link_type,
                    dest_url,
                    id,
                    ..
                },
            ) => (link_type, dest_url, id),
            _ => continue,
        };
        let span = if kind == LinkType::Inline {
            // Only inspect parser-confirmed links; code, HTML and plain filenames
            // never become upload requests. Match the parser's decoded destination.
            body[range.clone()]
                .match_indices("](")
                .find_map(|(offset, _)| {
                    let span = destination_span(body, range.start + offset + 2);
                    (span.end <= range.end
                        && decoded_markdown(&body[span.clone()]) == destination.as_ref())
                    .then_some(span)
                })
        } else if matches!(
            kind,
            LinkType::Reference | LinkType::Collapsed | LinkType::Shortcut
        ) {
            parser.reference_definitions().get(&id).and_then(|def| {
                body[def.span.clone()]
                    .find("]:")
                    .map(|offset| destination_span(body, def.span.start + offset + 2))
            })
        } else {
            None
        };
        if let Some(span) = span {
            spans.insert(span.start, (span, destination.into_string()));
        }
    }
    spans.into_values().collect()
}

pub fn rewrite(body: &str, replacements: &BTreeMap<String, String>) -> String {
    let mut result = body.to_owned();
    for (span, destination) in destinations(body).into_iter().rev() {
        if let Some(replacement) = replacements.get(&destination) {
            result.replace_range(span, replacement);
        }
    }
    result
}

fn local_path(destination: &str, base: &Path) -> Result<Option<PathBuf>> {
    if destination.is_empty()
        || destination.starts_with(['#', '?'])
        || destination.starts_with("//")
        || destination.starts_with("/attachments/")
    {
        return Ok(None);
    }
    if destination.starts_with("file:") {
        return reqwest::Url::parse(destination)
            .ok()
            .and_then(|url| url.to_file_path().ok())
            .map(Some)
            .ok_or_else(|| Error::invalid("Invalid local file URL"));
    }
    if destination
        .split_once(':')
        .is_some_and(|(scheme, _)| !scheme.contains('/'))
    {
        return Ok(None);
    }
    let path = destination.split(['#', '?']).next().unwrap();
    let mut bytes = Vec::new();
    let mut i = 0;
    while i < path.len() {
        if path.as_bytes()[i] == b'%'
            && i + 2 < path.len()
            && path.as_bytes()[i + 1..i + 3]
                .iter()
                .all(u8::is_ascii_hexdigit)
        {
            let byte = u8::from_str_radix(&path[i + 1..i + 3], 16).unwrap();
            bytes.push(byte);
            i += 3;
            continue;
        }
        bytes.push(path.as_bytes()[i]);
        i += 1;
    }
    let path =
        String::from_utf8(bytes).map_err(|_| Error::invalid("Attachment path must be UTF-8"))?;
    Ok(Some(base.join(path)))
}

pub fn local_destinations(body: &str) -> Result<Vec<String>> {
    let mut result = std::collections::BTreeSet::new();
    for (_, destination) in destinations(body) {
        if local_path(&destination, Path::new("."))?.is_some() {
            result.insert(destination);
        }
    }
    Ok(result.into_iter().collect())
}

pub fn collect(body: &str, base: &Path) -> Result<Vec<File>> {
    let mut files = BTreeMap::new();
    let mut total = 0;
    for (_, destination) in destinations(body) {
        if files.contains_key(&destination) {
            continue;
        }
        let Some(path) = local_path(&destination, base)? else {
            continue;
        };
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| Error::invalid("Attachment needs a UTF-8 filename"))?
            .to_owned();
        attachments::validate_name(&name)?;
        let bytes = attachments::read_file(&path)
            .map_err(|e| Error::invalid(format!("Cannot import {}: {e}", path.display())))?;
        total += bytes.len();
        if total > attachments::FILE_LIMIT {
            return Err(Error::invalid(
                "Markdown attachments must total at most 10 MiB per import",
            ));
        }
        files.insert(
            destination.clone(),
            File {
                destination,
                name,
                data: STANDARD.encode(bytes),
            },
        );
    }
    Ok(files.into_values().collect())
}
