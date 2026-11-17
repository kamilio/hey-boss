//! Compose source Markdown into messages. This module has no worker or Git side effects.
use crate::{
    agent_runtime::Provider,
    issues::{Error, Result},
};
use pulldown_cmark::{Event, Parser, Tag};
use serde_json::{Value, json};

pub fn compose_sequence(source: &str, settings: &Value) -> Result<Vec<Value>> {
    // Use source offsets so even nested fences and multiline code spans stay literal.
    let mut literal = Parser::new(source)
        .into_offset_iter()
        .filter_map(|(event, range)| {
            matches!(
                event,
                Event::Start(Tag::CodeBlock(_) | Tag::HtmlBlock) | Event::Code(_)
            )
            .then_some(range)
        })
        .peekable();
    let mut steps = Vec::new();
    let mut options = json!({});
    let mut body = String::new();
    let mut offset = 0;
    let mut size = 0;
    let mut header = None;
    for (index, raw) in source.split_inclusive('\n').enumerate() {
        let n = index + 1;
        let line = raw.trim_end_matches([' ', '\t', '\r', '\n']);
        while literal.next_if(|r| r.end <= offset).is_some() {}
        let protected = literal.peek().is_some_and(|r| r.contains(&offset));
        offset += raw.len();
        let append = if !protected && line.starts_with("@next(") {
            if header.is_some() || !body.trim().is_empty() {
                finish(&mut steps, options, &mut body, header)?;
            }
            body.clear();
            options = arguments(line, n, steps.is_empty())?;
            header = Some(n);
            continue;
        } else if !protected && line.starts_with("@include(") {
            let args = inside(line, "@include(", n)?;
            let key: String = serde_json::from_str(args)
                .map_err(|_| invalid(n, "expected @include(\"section\")"))?;
            let key = match key.as_str() {
                "workspace" if settings["worktree_enabled"] == true => "worktree",
                "workspace" => "checkout",
                "delivery" if settings["prs_enabled"] == true => "prs",
                "delivery" => "main",
                key => key,
            };
            let section = match key {
                "worker" => settings["prompt"].as_str(),
                _ if settings["prompt_defaults"].get(key).is_some() => settings["prompt_overrides"]
                    [key]
                    .as_str()
                    .or_else(|| settings["prompt_defaults"][key].as_str()),
                _ => None,
            }
            .ok_or_else(|| invalid(n, &format!("unknown prompt section {key:?}")))?;
            let ending = &raw[raw.trim_end_matches(['\r', '\n']).len()..];
            body.push_str(section);
            body.push_str(ending);
            section.len() + ending.len()
        } else {
            body.push_str(raw);
            raw.len()
        };
        size += append;
        if size > crate::issues::BODY_LIMIT {
            return Err(invalid(n, "composed prompt exceeds 1 MiB"));
        }
    }
    finish(&mut steps, options, &mut body, header)?;
    Ok(steps)
}

fn invalid(n: usize, message: &str) -> Error {
    Error::invalid(format!("Prompt line {n}: {message}"))
}

fn inside<'a>(line: &'a str, prefix: &str, n: usize) -> Result<&'a str> {
    line[prefix.len()..]
        .strip_suffix(')')
        .map(str::trim)
        .ok_or_else(|| invalid(n, "expected a standalone directive ending in ')'"))
}

fn arguments(line: &str, n: usize, first: bool) -> Result<Value> {
    let args = inside(line, "@next(", n)?;
    // An inline TOML table gives us named arguments, quoting and duplicate-key errors.
    let parsed = format!("{{{args}}}")
        .parse::<toml_edit::Value>()
        .map_err(|e| invalid(n, e.message()))?;
    let mut opts = json!({});
    for (key, val) in parsed.as_inline_table().expect("inline table").iter() {
        let value = val
            .as_str()
            .ok_or_else(|| invalid(n, "arguments must be quoted strings"))?;
        let valid = match key {
            "thread" => value == "new",
            "workspace" => matches!(value, "new-worktree" | "project-root"),
            "harness" => serde_json::from_value::<Provider>(json!(value)).is_ok(),
            "model" | "base" => {
                !value.is_empty()
                    && !value.starts_with('-')
                    && !value.chars().any(|c| c.is_control() || c.is_whitespace())
            }
            _ => return Err(invalid(n, &format!("unknown @next argument {key:?}"))),
        };
        if !valid {
            return Err(invalid(n, &format!("invalid {key}: {value:?}")));
        }
        opts[key] = json!(value);
    }
    if !opts["base"].is_null() && opts["workspace"] != "new-worktree" {
        return Err(invalid(n, "base requires workspace=\"new-worktree\""));
    }
    if !first
        && opts["thread"].is_null()
        && opts["workspace"].is_null()
        && (!opts["harness"].is_null() || !opts["model"].is_null())
    {
        return Err(invalid(
            n,
            "select a new thread or workspace to change harness or model",
        ));
    }
    Ok(opts)
}

fn finish(
    steps: &mut Vec<Value>,
    mut options: Value,
    body: &mut String,
    header: Option<usize>,
) -> Result<()> {
    if body.trim().is_empty() {
        return Err(invalid(header.unwrap_or(1), "step has no prompt"));
    }
    let first = steps.is_empty();
    let fresh = first || !options["thread"].is_null() || !options["workspace"].is_null();
    options["thread"] = json!(if fresh { "new" } else { "continue" });
    if options["workspace"].is_null() {
        options["workspace"] = json!(if first { "project-root" } else { "current" });
    }
    if let Some(previous) = steps.last() {
        if options["harness"].is_null() && previous.get("harness").is_some() {
            options["harness"] = previous["harness"].clone();
        }
        if options["model"].is_null()
            && options["harness"] == previous["harness"]
            && previous.get("model").is_some()
        {
            options["model"] = previous["model"].clone();
        }
    }
    options["prompt"] = Value::String(std::mem::take(body));
    steps.push(options);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn composes_rounds_with_workspace_and_provider_inheritance() {
        let settings = json!({"prompt_defaults":{"worktree":"default"}, "prompt_overrides":{"worktree":"saved\n@next()\n{{number}}"}});
        let steps = compose_sequence("@next(workspace=\"new-worktree\", base=\"origin/legacy\", harness=\"codex\", model=\"m\")\n@include(\"worktree\")\n@next()\nfollow\n@next(thread=\"new\", harness=\"claude\")\nreview", &settings).unwrap();
        assert_eq!(
            steps,
            vec![
                json!({"thread":"new","workspace":"new-worktree","base":"origin/legacy","harness":"codex","model":"m","prompt":"saved\n@next()\n{{number}}\n"}),
                json!({"thread":"continue","workspace":"current","harness":"codex","model":"m","prompt":"follow\n"}),
                json!({"thread":"new","workspace":"current","harness":"claude","prompt":"review"})
            ]
        );
    }

    #[test]
    fn only_standalone_directives_in_authored_markdown_are_active() {
        let md = "A\r\n\r\n~~~~markdown\r\n@next()\r\n~~~\r\n@include(\"absent\")\r\n~~~~\r\n\r\n> @next()\r\n\r\n    @next()\r\n\r\nInline `@next()` and\r\n`code\r\n@next()\r\nmore`\r\n";
        assert_eq!(
            compose_sequence(md, &Value::Null).unwrap(),
            vec![json!({"workspace":"project-root","thread":"new","prompt":md})]
        );
    }

    #[test]
    fn invalid_sequences_point_at_the_source_line() {
        for (source, line) in [
            ("@next(\nbody", 1),
            ("@next(base=\"main\")\nbody", 1),
            ("@next(thread=\"new\",thread=\"new\")\nx", 1),
            ("text\n@next(model=\"m\")\nx", 2),
            ("text\n@include(\"unknown\")", 2),
            ("a\n@next()\n@next()\nb", 2),
            ("a\n@next()\n", 2),
            ("@next(subagent=\"claude\")\nr", 1),
        ] {
            assert!(
                compose_sequence(source, &Value::Null)
                    .unwrap_err()
                    .message
                    .starts_with(&format!("Prompt line {line}:")),
                "{source}"
            );
        }
    }
}
