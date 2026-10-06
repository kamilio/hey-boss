//! Keep the person selected by gh independent of the request's credentials.
use graphql_parser::query::{
    Definition, OperationDefinition, Selection, SelectionSet, Value as GqlValue,
};
use serde_json::Value;
use std::collections::HashSet;
#[cfg(unix)]
mod transport;
#[cfg(unix)]
pub use transport::run;
#[cfg(not(unix))]
pub fn run(
    _: std::process::Command,
    _: String,
    _: String,
) -> Result<std::process::ExitStatus, Box<dyn std::error::Error>> {
    Err("GitHub App user selection requires Unix socket support".into())
}

fn rewrite_search(query: &str, login: &str) -> String {
    const QUALIFIERS: &[&str] = &[
        "author",
        "assignee",
        "mentions",
        "commenter",
        "involves",
        "review-requested",
        "reviewed-by",
        "user",
        "committer",
    ];
    let bytes = query.as_bytes();
    let mut result = String::new();
    let (mut index, mut copied) = (0, 0);
    while index < bytes.len() {
        if bytes[index] == b'"' {
            index += 1;
            while index < bytes.len() {
                match bytes[index] {
                    b'\\' => index += 2,
                    b'"' => {
                        index += 1;
                        break;
                    }
                    _ => index += 1,
                }
            }
            continue;
        }
        if index == 0 || bytes[index - 1].is_ascii_whitespace() || bytes[index - 1] == b'(' {
            let start = index + usize::from(bytes[index] == b'-');
            if let Some(qualifier) = QUALIFIERS.iter().find(|q| {
                query
                    .get(start..)
                    .is_some_and(|s| s.starts_with(**q) && s.as_bytes().get(q.len()) == Some(&b':'))
            }) {
                let value = start + qualifier.len() + 1;
                let quoted = bytes.get(value) == Some(&b'"');
                let me = value + usize::from(quoted);
                let end = me + 3;
                let after = end + usize::from(quoted);
                if query.get(me..end) == Some("@me")
                    && (!quoted || bytes.get(end) == Some(&b'"'))
                    && bytes
                        .get(after)
                        .is_none_or(|b| b.is_ascii_whitespace() || matches!(b, b')' | b','))
                {
                    result.push_str(&query[copied..me]);
                    result.push_str(login);
                    copied = end;
                    index = after;
                    continue;
                }
            }
        }
        index += 1;
    }
    result.push_str(&query[copied..]);
    result
}

fn rewrite_graphql(body: &mut Value, login: &str) -> Result<bool, String> {
    let Some(query) = body["query"].as_str() else {
        return Ok(false);
    };
    // Leave invalid/ambiguous requests to gh/GitHub, preserving their diagnostics.
    let Ok(mut document) = graphql_parser::parse_query::<String>(query).map(|d| d.into_static())
    else {
        return Ok(false);
    };
    let operation_name = body["operationName"].as_str();
    let operations = document
        .definitions
        .iter()
        .filter(|d| matches!(d, Definition::Operation(_)))
        .count();
    if operation_name.is_none() && operations != 1 {
        return Ok(false);
    }
    let selected = document.definitions.iter().position(|d| match d {
        Definition::Operation(OperationDefinition::Query(q)) => {
            operation_name.is_none() || q.name.as_deref() == operation_name
        }
        Definition::Operation(OperationDefinition::SelectionSet(_)) => operation_name.is_none(),
        _ => false,
    });
    let Some(selected) = selected else {
        return Ok(false);
    };
    let mut variables = HashSet::new();
    let mut fragments = Vec::new();
    let mut changed = false;
    fn visit(
        set: &mut SelectionSet<'static, String>,
        root: bool,
        login: &str,
        variables: &mut HashSet<String>,
        fragments: &mut Vec<(String, bool)>,
        changed: &mut bool,
    ) {
        for selection in &mut set.items {
            match selection {
                Selection::Field(field) => {
                    if root && field.name == "viewer" {
                        field.alias.get_or_insert_with(|| "viewer".into());
                        field.name = "user".into();
                        field.arguments = vec![("login".into(), GqlValue::String(login.into()))];
                        *changed = true;
                    }
                    // Only search inputs are rewritten; titles, bodies and other
                    // arbitrary strings containing @me must remain literal.
                    if field.name == "search" {
                        for (name, value) in &mut field.arguments {
                            if name != "query" {
                                continue;
                            }
                            match value {
                                GqlValue::String(text) => {
                                    let rewritten = rewrite_search(text, login);
                                    *changed |= rewritten != *text;
                                    *text = rewritten;
                                }
                                GqlValue::Variable(name) => {
                                    variables.insert(name.clone());
                                }
                                _ => {}
                            }
                        }
                    }
                    visit(
                        &mut field.selection_set,
                        false,
                        login,
                        variables,
                        fragments,
                        changed,
                    );
                }
                Selection::InlineFragment(fragment) => visit(
                    &mut fragment.selection_set,
                    root,
                    login,
                    variables,
                    fragments,
                    changed,
                ),
                Selection::FragmentSpread(fragment) => {
                    fragments.push((fragment.fragment_name.clone(), root))
                }
            }
        }
    }
    let set = match &mut document.definitions[selected] {
        Definition::Operation(OperationDefinition::Query(q)) => &mut q.selection_set,
        Definition::Operation(OperationDefinition::SelectionSet(set)) => set,
        _ => unreachable!(),
    };
    visit(
        set,
        true,
        login,
        &mut variables,
        &mut fragments,
        &mut changed,
    );
    let mut visited = HashSet::new();
    while let Some((name, root)) = fragments.pop() {
        if !visited.insert((name.clone(), root)) {
            continue;
        }
        if let Some(Definition::Fragment(fragment)) = document
            .definitions
            .iter_mut()
            .find(|d| matches!(d, Definition::Fragment(f) if f.name == name))
        {
            visit(
                &mut fragment.selection_set,
                root,
                login,
                &mut variables,
                &mut fragments,
                &mut changed,
            );
        }
    }
    for name in variables {
        if let Some(Value::String(text)) = body.get_mut("variables").and_then(|v| v.get_mut(&name))
        {
            let rewritten = rewrite_search(text, login);
            changed |= rewritten != *text;
            *text = rewritten;
        } else if body.get("variables").and_then(|v| v.get(&name)).is_none()
            && let Definition::Operation(OperationDefinition::Query(q)) =
                &mut document.definitions[selected]
            && let Some(variable) = q.variable_definitions.iter_mut().find(|v| v.name == name)
            && let Some(GqlValue::String(text)) = &mut variable.default_value
        {
            let rewritten = rewrite_search(text, login);
            changed |= rewritten != *text;
            *text = rewritten;
        }
    }
    if changed {
        body["query"] = Value::String(document.to_string());
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn every_person_search_qualifier_uses_the_selected_user() {
        for qualifier in [
            "author",
            "assignee",
            "mentions",
            "commenter",
            "involves",
            "review-requested",
            "reviewed-by",
            "user",
            "committer",
        ] {
            let query = format!(
                "is:pr ({qualifier}:@me OR -{qualifier}:\"@me\") author:someone \"{qualifier}:@me\""
            );
            assert_eq!(
                rewrite_search(&query, "octocat"),
                format!(
                    "is:pr ({qualifier}:octocat OR -{qualifier}:\"octocat\") author:someone \"{qualifier}:@me\""
                )
            );
        }
        assert_eq!(
            rewrite_search("@me author:@me-other label:@me body:author:@me", "octocat"),
            "@me author:@me-other label:@me body:author:@me"
        );
    }

    #[test]
    fn implicit_viewer_and_search_variables_keep_app_auth_and_select_the_person() {
        let mut body = json!({"query":"query Status($q:String!) { me: viewer { login pullRequests(first:10) { totalCount } } search(query:$q,type:ISSUE,first:10) { issueCount } repository(owner:\"acme\",name:\"demo\") { viewerPermission } }", "variables":{"q":"author:@me review-requested:@me"}});
        assert!(rewrite_graphql(&mut body, "octocat").unwrap());
        let query = body["query"].as_str().unwrap();
        assert!(query.contains("me: user(login: \"octocat\")"), "{query}");
        assert!(query.contains("viewerPermission"));
        assert_eq!(
            body["variables"]["q"],
            "author:octocat review-requested:octocat"
        );
    }

    #[test]
    fn selected_queries_and_fragments_are_rewritten_but_mutations_are_untouched() {
        let mut body = json!({"query":"query Mine($q:String = \"assignee:@me\") { ...MineFields search(query:$q,type:ISSUE,first:10) { issueCount } } fragment MineFields on Query { viewer { login } } mutation Post { addComment(input:{subjectId:\"id\",body:\"author:@me\"}) { clientMutationId } }", "operationName":"Mine"});
        assert!(rewrite_graphql(&mut body, "octocat").unwrap());
        let query = body["query"].as_str().unwrap();
        assert!(
            query.contains("viewer: user(login: \"octocat\")"),
            "{query}"
        );
        assert!(query.contains("assignee:octocat"), "{query}");
        assert!(query.contains("body: \"author:@me\""), "{query}");
        body["operationName"] = json!("Post");
        let unchanged = body.clone();
        assert!(!rewrite_graphql(&mut body, "different").unwrap());
        assert_eq!(body, unchanged);
    }
}
