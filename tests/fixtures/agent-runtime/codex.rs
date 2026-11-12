use super::*;
fn permissions() -> [&'static str; 3] {
    if setting("PERMISSIONS").as_deref() == Some("yolo") {
        ["never", "user", "danger-full-access"]
    } else {
        ["on-request", "auto_review", "workspace-write"]
    }
}
pub(super) fn check_args(args: &[String]) {
    let check = |key: &str, expected: Option<&str>| {
        let prefix = format!("{key}=");
        let values: Vec<_> = args
            .windows(2)
            .filter(|a| a[0] == "-c")
            .filter_map(|a| a[1].strip_prefix(&prefix))
            .collect();
        assert_eq!(
            values,
            expected
                .map(|v| json!(v).to_string())
                .into_iter()
                .collect::<Vec<_>>(),
            "{key}"
        );
    };
    for (k, v) in ["approval_policy", "approvals_reviewer", "sandbox_mode"]
        .into_iter()
        .zip(permissions())
    {
        check(
            k,
            (setting("PERMISSIONS").as_deref() != Some("inherit")).then_some(v),
        );
    }
    if let Some(v) = setting("EFFORT") {
        check(
            "model_reasoning_effort",
            (!v.is_empty()).then_some(v.as_str()),
        )
    }
}
impl Agent {
    pub(super) fn codex(&mut self, r: &Value) {
        let (id, p) = (&r["id"], &r["params"]);
        let method = value(&r["method"]);
        let text = value(&p["input"][0]["text"]);
        let fail = |msg: &str| send(json!({"id":id,"error":{"code":-32000,"message":msg}}));
        let mut result = json!({});
        match method {
            "" => {
                if id == "approval-1" {
                    assert_eq!(r["result"]["decision"], "decline");
                    self.complete(false)
                }
                return;
            }
            "initialized" => return,
            "initialize" if enabled("JOB_HOLD_INIT") => return,
            "thread/start" | "thread/resume" => {
                if enabled("FRESH_THREADS") && method == "thread/resume" {
                    self.session = value(&p["threadId"]).into()
                }
                for (k, v) in ["approvalPolicy", "approvalsReviewer", "sandbox"]
                    .into_iter()
                    .zip(permissions())
                {
                    if setting("PERMISSIONS").as_deref() == Some("inherit") {
                        assert!(p.get(k).is_none())
                    } else {
                        assert_eq!(p[k], v)
                    }
                }
                let model = p["model"].as_str().unwrap_or("default-model");
                self.check_model(model);
                if setting("MODEL_FAILURE").as_deref() == Some("reject") {
                    fail("Requested model unavailable");
                    return;
                }
                self.model = if setting("MODEL_FAILURE").as_deref() == Some("ignore") {
                    "default-model".into()
                } else {
                    model.into()
                };
                result = json!({"thread":{"id":self.session},"model":self.model});
            }
            "turn/start" => {
                if let Some(e) = setting("EFFORT") {
                    assert_eq!(value(&p["effort"]), e)
                }
                self.check_model(p["model"].as_str().unwrap_or("default-model"));
                self.turn += 1;
                result = json!({"turn":{"id":self.turn()}});
                match text {
                    "start before ack" => send(
                        json!({"method":"turn/started","params":{"threadId":self.session,"turn":{"id":self.turn()}}}),
                    ),
                    "missing turn acknowledgement" => result = json!({"turn":{}}),
                    "malformed acknowledgement" => {
                        send(json!({"id":id}));
                        self.prompt("hold ambiguous acknowledgement");
                        return;
                    }
                    _ => {}
                }
                if enabled("JOB_ACK_LOST") {
                    self.prompt(text);
                    return;
                }
            }
            "model/list" => {
                if enabled("DISCOVERY_FAILURE") {
                    fail("Model discovery unavailable");
                    return;
                }
                let next = enabled("CATALOG_PAGES")
                    && (p["cursor"].is_null()
                        || setting("CATALOG_PAGES").as_deref() == Some("loop"));
                result = json!({"data":[{"model":if p["cursor"].is_null(){"catalog-model"}else{"second-model"},"displayName":"Catalog model"}],"nextCursor":next.then_some("next")});
            }
            "turn/steer" if text == "reject steering" => {
                fail("fixture rejection");
                return;
            }
            "turn/interrupt" if self.output == "hold rejected interrupt" => {
                fail("fixture rejection");
                self.later(100);
                return;
            }
            "turn/steer" => result = json!({"turnId":self.turn()}),
            "thread/read" => {
                result = json!({"thread":{"id":self.session,"status":{"type":if self.streaming{"active"}else{"idle"}},"canAcceptDirectInput":self.streaming}})
            }
            "thread/backgroundTerminals/list" => {
                assert_eq!(p["threadId"], self.session);
                result = json!({"data":self.native.as_ref().map(|_|json!({"itemId":"native-item","processId":"native-tool"})).into_iter().collect::<Vec<_>>(),"nextCursor":null})
            }
            "thread/backgroundTerminals/terminate" => {
                assert_eq!(p["threadId"], self.session);
                assert_eq!(p["processId"], "native-tool");
                let mut child = self.native.take().unwrap();
                child.kill().unwrap();
                child.wait().unwrap();
                result = json!({"terminated":true})
            }
            _ => {}
        }
        send(json!({"id":id,"result":result}));
        if method == "turn/start" {
            self.prompt(text)
        } else if method == "turn/interrupt" {
            self.complete(true)
        }
    }
}
