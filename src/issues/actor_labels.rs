//! Display metadata is separate from the stable IDs used for ownership and links.
use super::*;

fn collect(value: &Value, ids: &mut BTreeSet<String>) {
    match value {
        Value::Object(object) => {
            for (key, value) in object {
                if matches!(
                    key.as_str(),
                    "id" | "actor"
                        | "actor_id"
                        | "author"
                        | "assignee"
                        | "created_by"
                        | "closed_by"
                        | "added_by"
                ) && let Some(id) = value.as_str()
                    && ["codex:", "claude:", "worker:", "agent:"]
                        .iter()
                        .any(|prefix| id.starts_with(prefix))
                {
                    ids.insert(id.to_owned());
                }
                if key == "assignees" {
                    for id in value
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                    {
                        ids.insert(id.to_owned());
                    }
                }
                if key == "session_id"
                    && let Some(session) = value.as_str()
                {
                    ids.insert(format!("codex:{session}"));
                }
                collect(value, ids);
            }
        }
        Value::Array(values) => values.iter().for_each(|value| collect(value, ids)),
        _ => {}
    }
}

pub(crate) fn models<'a>(
    db: &Connection,
    values: impl IntoIterator<Item = &'a Value>,
) -> Result<serde_json::Map<String, Value>> {
    let mut ids = BTreeSet::new();
    for value in values {
        collect(value, &mut ids);
    }
    if ids.is_empty() {
        return Ok(serde_json::Map::new());
    }
    // Drive primary-key lookups from this response, never scan all known agents.
    let mut query = db.prepare(
        "SELECT a.id,a.metadata FROM json_each(?1) selected JOIN agents a ON a.id=selected.value",
    )?;
    let rows = query.query_map([serde_json::to_string(&ids)?], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut models = serde_json::Map::new();
    for row in rows {
        let (id, metadata) = row?;
        let Ok(metadata) = serde_json::from_str::<Value>(&metadata) else {
            continue;
        };
        if let Some(model) = metadata["model"].as_str().map(str::trim)
            && !model.is_empty()
            && model.len() <= 256
            && !model.chars().any(char::is_control)
        {
            models.insert(id, json!(model));
        }
    }
    Ok(models)
}

pub(crate) fn enrich(db: &Connection, value: &mut Value) -> Result<()> {
    let models = models(db, [&*value])?;
    if !models.is_empty() {
        value["actor_models"] = Value::Object(models);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_cover_response_actors_without_exposing_private_metadata() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch(
            "CREATE TABLE agents(id TEXT PRIMARY KEY,metadata TEXT);
          INSERT INTO agents VALUES('codex:one','{\"model\":\"gpt-6-astra\",\"cwd\":\"private\"}'),
          ('claude:two','{\"model\":\"claude-opus-4-6\"}'),
          ('codex:unrelated','{\"model\":\"other\"}'),
          ('codex:bad','{\"model\":42}');",
        )
        .unwrap();
        let mut result = json!({"issues":[{"assignee":"codex:one","created_by":"human:boss"}],"comments":[{"author":"claude:two"}],"events":[{"actor":"codex:missing"},{"actor":"codex:bad"}],"nodes":[{"assignee":"codex:one"}]});
        enrich(&db, &mut result).unwrap();
        assert_eq!(
            result["actor_models"],
            json!({"codex:one":"gpt-6-astra","claude:two":"claude-opus-4-6"})
        );
        assert!(!result.to_string().contains("private"));
        assert_eq!(result["issues"][0]["assignee"], "codex:one");
    }

    #[test]
    fn activity_keeps_the_model_recorded_at_each_event() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE agents(id TEXT PRIMARY KEY,metadata TEXT);
          CREATE TABLE events(project_id TEXT,issue_number INTEGER,actor TEXT,action TEXT,created_at INTEGER,data TEXT);
          INSERT INTO agents VALUES('codex:one','{\"model\":\"first-model\"}');").unwrap();
        super::super::event(&db, "project", 1, "codex:one", "claimed", 1, &json!({})).unwrap();
        db.execute(
            "UPDATE agents SET metadata=?1",
            [json!({"model":"second-model"}).to_string()],
        )
        .unwrap();
        super::super::event(&db, "project", 1, "codex:one", "commented", 2, &json!({})).unwrap();
        let models = db
            .query_collect(
                "SELECT json_extract(data,'$.actor_model') FROM events ORDER BY created_at",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap();
        assert_eq!(models, ["first-model", "second-model"]);
    }
}
