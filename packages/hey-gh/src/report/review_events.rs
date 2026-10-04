//! Reuse an already complete timeline without changing the public event shape.
use serde_json::{Value, json};

pub(super) fn from_timeline(timeline: &[Value]) -> Option<Vec<Value>> {
    let mut events = Vec::new();
    for event in timeline {
        let kind = match event["event"].as_str()? {
            "review_requested" => "ReviewRequestedEvent",
            "review_request_removed" => "ReviewRequestRemovedEvent",
            _ => continue,
        };
        let id = text(&event["node_id"])?;
        let at = text(&event["created_at"])?;
        let user = &event["requested_reviewer"];
        let team = &event["requested_team"];
        let reviewer = if user["type"] == "User" && team.is_null() {
            json!({"__typename":"User","login":text(&user["login"])?})
        } else if user.is_null() && team.is_object() {
            json!({"__typename":"Team","slug":text(&team["slug"])?,"name":text(&team["name"])?})
        } else {
            // Deleted, redacted, unfamiliar or ambiguous reviewers need the
            // original query; missing REST fields do not establish GraphQL null.
            return None;
        };
        events.push(json!({"__typename":kind,"id":id,"createdAt":at,"requestedReviewer":reviewer}));
    }
    Some(events)
}

fn text(value: &Value) -> Option<&str> {
    value.as_str().filter(|text| !text.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn incomplete_fields_do_not_become_successful_empty_or_partial_history() {
        let event = json!({"node_id":"E1","event":"review_requested","created_at":"2026-10-04T00:00:00Z","requested_reviewer":{"type":"User","login":"reviewer"}});
        for key in ["node_id", "created_at", "requested_reviewer", "event"] {
            let mut incomplete = event.clone();
            incomplete.as_object_mut().unwrap().remove(key);
            assert!(
                from_timeline(&[event.clone(), incomplete]).is_none(),
                "{key}"
            );
        }
        for reviewer in [
            Value::Null,
            json!({"type":"Bot","login":"bot"}),
            json!({"type":"User"}),
            json!({"type":"User","login":""}),
        ] {
            let mut incomplete = event.clone();
            incomplete["requested_reviewer"] = reviewer;
            assert!(from_timeline(&[incomplete]).is_none());
        }
        for team in [json!({"slug":"team"}), json!({"name":"Team"})] {
            let mut incomplete = event.clone();
            incomplete["requested_reviewer"] = Value::Null;
            incomplete["requested_team"] = team;
            assert!(from_timeline(&[incomplete]).is_none());
        }
        let mut ambiguous = event;
        ambiguous["requested_team"] = json!({"slug":"team","name":"Team"});
        assert!(from_timeline(&[ambiguous]).is_none());
        assert_eq!(from_timeline(&[]), Some(vec![]));
        assert_eq!(from_timeline(&[json!({"event":"commented"})]), Some(vec![]));
    }
}
