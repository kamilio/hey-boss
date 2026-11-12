use super::*;
impl Agent {
    pub(super) fn claude(&mut self, r: &Value) {
        match value(&r["type"]) {
            "control_request" => {
                let id = &r["request_id"];
                let method = value(&r["request"]["subtype"]);
                if method == "interrupt" {
                    match self.output.as_str() {
                        "hold malformed interrupt" => {
                            send(
                                json!({"type":"control_response","response":{"request_id":id,"subtype":"invalid"}}),
                            );
                            return;
                        }
                        "hold rejected interrupt" => {
                            send(
                                json!({"type":"control_response","response":{"request_id":id,"subtype":"error","error":"fixture rejection"}}),
                            );
                            self.later(100);
                            return;
                        }
                        _ => {}
                    }
                }
                let mut response = json!({});
                if method == "initialize" && !enabled("DISCOVERY_FAILURE") {
                    let mut model = json!({"value":"moving-alias","displayName":"Catalog model"});
                    if !enabled("LEGACY_MODELS") {
                        model["resolvedModel"] = json!("catalog-model")
                    }
                    response = json!({"models":[model]});
                }
                send(
                    json!({"type":"control_response","response":{"subtype":"success","request_id":id,"response":response}}),
                );
                if method == "interrupt" {
                    self.complete(true)
                }
            }
            "user" => {
                let text = value(&r["message"]["content"]);
                if self.streaming {
                    self.output = text.into()
                } else {
                    self.prompt(text)
                }
            }
            "control_response" => {
                let answer = &r["response"]["response"];
                if r["response"]["request_id"] == "question" {
                    self.output = if answer["behavior"] == "deny" {
                        "cancelled"
                    } else {
                        value(&answer["updatedInput"]["answers"]["Choose a color"])
                    }
                    .into()
                } else {
                    assert_eq!(answer["behavior"], "deny")
                }
                self.complete(false)
            }
            _ => {}
        }
    }
}
