use super::*;
impl Agent {
    pub(super) fn restore_pi(&mut self, path: PathBuf) {
        for entry in fs::read_to_string(&path)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str::<Value>(l).unwrap())
        {
            if entry["type"] == "model_change" {
                self.model = value(&entry["modelId"]).into();
                self.route = value(&entry["provider"]).into()
            }
        }
        self.file = Some(path);
    }
    fn save_pi(&mut self, persist: bool) {
        if self.file.is_some() && !persist {
            return;
        }
        let file = self.file.get_or_insert_with(|| {
            let root = setting("ROOT")
                .map(PathBuf::from)
                .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("out/pi-fixtures"));
            let dir = root.join(uuid::Uuid::new_v4().to_string());
            fs::create_dir_all(&dir).unwrap();
            dir.join(format!("{}.jsonl", self.session))
        });
        fs::write(
            file,
            format!(
                "{}\n{}\n",
                json!({"type":"session","id":self.session}),
                json!({"type":"model_change","modelId":self.model,"provider":self.route})
            ),
        )
        .unwrap();
    }
    pub(super) fn pi(&mut self, r: &Value) {
        let (id, command, text) = (&r["id"], value(&r["type"]), value(&r["message"]));
        let fail = |msg: &str| {
            send(json!({"id":id,"type":"response","command":command,"success":false,"error":msg}))
        };
        let mut ack = json!({"id":id,"type":"response","command":command,"success":true});
        match command {
            "get_commands" => {
                ack["data"] = json!({"commands":[
                    {"name":"local","source":"extension"}, {"name":"model","source":"extension"},
                    {"name":"template","source":"prompt"}
                ]});
            }
            "prompt" if text.starts_with("/local") => {
                if text == "/local fail" {
                    send(json!({"type":"extension_error","event":"command","error":"handler failed"}));
                }
                send(ack);
                return;
            }
            "prompt" if text == "/model" => {
                send(json!({"type":"agent_start"}));
                send(ack);
                self.output = "model reply".into();
                self.later(100);
                return;
            }
            "extension_ui_response" => {
                if let Some((id, reject)) = self.preflight.take() {
                    let mut v =
                        json!({"id":id,"type":"response","command":"prompt","success":!reject});
                    if reject {
                        v["error"] = json!("preflight rejected")
                    }
                    send(v);
                    if !reject {
                        self.output = "preflight accepted".into();
                        self.complete(false)
                    }
                } else {
                    assert_eq!(r["cancelled"], true);
                    self.output = "input cancelled".into();
                    self.complete(false)
                }
                return;
            }
            "prompt" if text.starts_with("preflight") => {
                self.preflight = Some((id.clone(), text == "preflight reject"));
                send(
                    json!({"type":"extension_ui_request","id":"preflight-input","method":"select","title":"Preflight","options":["one","two"]}),
                );
                return;
            }
            "prompt" if text == "malformed prompt acknowledgement" => {
                send(json!({"id":id,"type":"response","command":command}));
                self.prompt("hold malformed prompt");
                return;
            }
            "steer" if text == "reject steering" => {
                fail("fixture rejection");
                return;
            }
            "clear_queue" if self.output == "hold rejected interrupt" => {
                fail("fixture rejection");
                self.later(100);
                return;
            }
            "get_available_models" => {
                if enabled("DISCOVERY_FAILURE") {
                    fail("Model discovery unavailable");
                    return;
                }
                ack["data"] = json!({"models":[{"id":"catalog-model","provider":"proxy","name":"Catalog model"}]});
            }
            "set_model" => {
                let model = value(&r["modelId"]);
                self.check_model(model);
                assert_eq!(r["provider"], "proxy");
                if setting("MODEL_FAILURE").as_deref() == Some("reject") {
                    fail("Model unavailable");
                    return;
                }
                self.model = if setting("MODEL_FAILURE").as_deref() == Some("ignore") {
                    "default-model"
                } else {
                    model
                }
                .into();
                self.route = "proxy".into();
                self.save_pi(true);
                ack["data"] = json!({"id":self.model,"provider":self.route});
            }
            "get_state" => {
                self.save_pi(false);
                ack["data"] = json!({"sessionFile":self.file,"isStreaming":self.streaming,"model":{"id":self.model,"provider":self.route}});
                if self.output != "missing session acknowledgement" {
                    ack["data"]["sessionId"] = json!(self.session);
                }
            }
            _ => {}
        }
        send(ack);
        if command == "prompt" {
            self.prompt(text)
        } else if command == "abort" {
            self.complete(true)
        }
    }
}
