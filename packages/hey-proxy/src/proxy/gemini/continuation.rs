//! A native generation can span several HTTP responses. Keep opaque resume
//! tokens out of converters, replay carriers and diagnostics.
use anyhow::{Result, ensure};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::HashSet;

const COUNTS: [&str; 6] = [
    "promptTokenCount",
    "candidatesTokenCount",
    "thoughtsTokenCount",
    "cachedContentTokenCount",
    "totalTokenCount",
    "toolUsePromptTokenCount",
];
const MAX_CHUNKS: usize = 128;
const MAX_BYTES: usize = 64 * 1024 * 1024;

pub(in crate::proxy) struct Continuation {
    base: Value,
    budget: Option<u64>,
    token: Option<String>,
    seen: HashSet<[u8; 32]>,
    reason: Option<String>,
    blocked: bool,
    usage: Value,
    prior_usage: Value,
    bytes: usize,
}

impl Continuation {
    pub fn new(base: Value, budget: Option<u64>) -> Self {
        Self {
            base,
            budget,
            token: None,
            seen: HashSet::new(),
            reason: None,
            blocked: false,
            usage: json!({}),
            prior_usage: json!({}),
            bytes: 0,
        }
    }

    pub fn feed(&mut self, native: &mut Value) -> Result<()> {
        if let Some(candidate) = native
            .pointer_mut("/candidates/0")
            .and_then(Value::as_object_mut)
        {
            if let Some(token) = candidate.remove("continuationToken") {
                ensure!(
                    self.token.is_none(),
                    "Gemini returned multiple continuation tokens in one chunk"
                );
                self.token = Some(
                    token
                        .as_str()
                        .filter(|s| !s.is_empty())
                        .ok_or_else(|| {
                            anyhow::anyhow!("Gemini continuation requires a nonempty token")
                        })?
                        .to_owned(),
                );
            }
            if let Some(reason) = candidate.get("finishReason") {
                let reason = reason
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("Invalid Gemini finish reason"))?;
                ensure!(
                    self.reason.as_deref().is_none_or(|r| r == reason),
                    "Gemini returned conflicting finish reasons"
                );
                self.reason = Some(reason.to_owned());
                if reason == "CONTINUATION" {
                    candidate.remove("finishReason");
                }
            }
        }
        self.blocked |= native.pointer("/promptFeedback/blockReason").is_some();
        // Usage frames are snapshots within one HTTP chunk, additive between
        // chunks. Preserve sparse trailing metadata without counting it twice.
        if let Some(usage) = native.get_mut("usageMetadata") {
            let fields = usage
                .as_object()
                .ok_or_else(|| anyhow::anyhow!("Invalid Gemini usage metadata"))?;
            for key in COUNTS {
                if let Some(n) = fields.get(key) {
                    ensure!(n.as_u64().is_some(), "Invalid Gemini token accounting");
                }
            }
            self.usage.as_object_mut().unwrap().extend(fields.clone());
            *usage = self.total_usage();
        }
        // Count only retained output, not the growing opaque resume token.
        self.bytes = self.bytes.saturating_add(serde_json::to_vec(native)?.len());
        ensure!(self.bytes <= MAX_BYTES, "Gemini response exceeds 64 MiB");
        Ok(())
    }

    fn total_usage(&self) -> Value {
        let mut total = self.prior_usage.clone();
        for (key, value) in self.usage.as_object().unwrap() {
            total[key] = if COUNTS.contains(&key.as_str()) {
                json!(
                    self.prior_usage[key]
                        .as_u64()
                        .unwrap_or(0)
                        .saturating_add(value.as_u64().unwrap_or(0))
                )
            } else {
                value.clone()
            };
        }
        if self.usage.get("totalTokenCount").is_none()
            && COUNTS[..3].iter().any(|key| self.usage.get(key).is_some())
        {
            total["totalTokenCount"] = json!(COUNTS[..3].iter().fold(
                self.prior_usage["totalTokenCount"].as_u64().unwrap_or(0),
                |sum, key| sum.saturating_add(self.usage[key].as_u64().unwrap_or(0))
            ));
        }
        total
    }

    /// Call only at EOF so a marker never discards trailing usage frames.
    pub fn next(&mut self) -> Result<Option<Value>> {
        if self.reason.as_deref() != Some("CONTINUATION") || self.blocked {
            return Ok(None);
        }
        let token = self
            .token
            .take()
            .ok_or_else(|| anyhow::anyhow!("Gemini continuation requires a nonempty token"))?;
        ensure!(
            self.seen.insert(Sha256::digest(token.as_bytes()).into()),
            "Gemini repeated a continuation token"
        );
        let usage = self.total_usage();
        let output = usage["candidatesTokenCount"]
            .as_u64()
            .unwrap_or(0)
            .saturating_add(usage["thoughtsTokenCount"].as_u64().unwrap_or(0));
        if self.budget.is_some_and(|budget| output >= budget) {
            self.reason = Some("MAX_TOKENS".into());
            return Ok(None);
        }
        ensure!(
            self.seen.len() < MAX_CHUNKS,
            "Gemini continuation exceeded 128 chunks"
        );
        let mut body = self.base.clone();
        body["continuationToken"] = json!(token);
        self.prior_usage = usage;
        self.usage = json!({});
        self.reason = None;
        Ok(Some(body))
    }

    pub fn terminal(&self) -> Value {
        let mut frame = json!({});
        if let Some(reason) = &self.reason {
            frame["candidates"] = json!([{"index":0,"finishReason":reason}]);
        }
        if !self.total_usage().as_object().unwrap().is_empty() {
            frame["usageMetadata"] = self.total_usage();
        }
        frame
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn chunk(token: Value) -> Value {
        json!({"candidates":[{"finishReason":"CONTINUATION","continuationToken":token}]})
    }

    #[test]
    fn empty_chunks_sparse_usage_and_original_request_are_preserved() {
        let base = json!({"contents":[{"role":"user","parts":[{"text":"test"}]}],"generationConfig":{"maxOutputTokens":100},"tools":[]});
        let mut state = Continuation::new(base.clone(), Some(100));
        for (n, token) in ["one", "two", "three"].into_iter().enumerate() {
            let mut native = chunk(json!(token));
            native["usageMetadata"] = json!({"promptTokenCount":10,"candidatesTokenCount":2});
            state.feed(&mut native).unwrap();
            assert_eq!(native["candidates"][0], json!({}));
            assert_eq!(native["usageMetadata"]["promptTokenCount"], (n + 1) * 10);
            state
                .feed(&mut json!({"usageMetadata":{"thoughtsTokenCount":1}}))
                .unwrap();
            let mut expected = base.clone();
            expected["continuationToken"] = json!(token);
            assert_eq!(state.next().unwrap(), Some(expected));
        }
        state.feed(&mut json!({"candidates":[{"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":10,"candidatesTokenCount":2}})).unwrap();
        assert!(state.next().unwrap().is_none());
        assert_eq!(state.terminal()["usageMetadata"]["promptTokenCount"], 40);
        assert_eq!(state.terminal()["usageMetadata"]["thoughtsTokenCount"], 3);
        assert_eq!(state.terminal()["usageMetadata"]["totalTokenCount"], 51);
    }

    #[test]
    fn missing_empty_repeated_and_cyclic_tokens_fail_without_exposing_tokens() {
        for token in [Value::Null, json!(""), json!(42)] {
            let mut state = Continuation::new(json!({}), None);
            assert!(state.feed(&mut chunk(token)).is_err());
        }
        let mut state = Continuation::new(json!({}), None);
        state
            .feed(&mut json!({"candidates":[{"finishReason":"CONTINUATION"}]}))
            .unwrap();
        assert!(state.next().is_err());
        for tokens in [
            ["private-one", "private-one"],
            ["private-one", "private-two"],
        ] {
            let mut state = Continuation::new(json!({}), None);
            state.feed(&mut chunk(json!(tokens[0]))).unwrap();
            state.next().unwrap();
            state.feed(&mut chunk(json!(tokens[1]))).unwrap();
            let error = if tokens[0] == tokens[1] {
                state.next().unwrap_err()
            } else {
                state.next().unwrap();
                state.feed(&mut chunk(json!(tokens[0]))).unwrap();
                state.next().unwrap_err()
            };
            assert!(!error.to_string().contains("private"));
        }
    }

    #[test]
    fn budget_loop_and_output_bounds_stop_generation() {
        let mut state = Continuation::new(json!({}), Some(3));
        let mut native = chunk(json!("one"));
        native["usageMetadata"] = json!({"candidatesTokenCount":2,"thoughtsTokenCount":1});
        state.feed(&mut native).unwrap();
        assert!(state.next().unwrap().is_none());
        assert_eq!(
            state.terminal()["candidates"][0]["finishReason"],
            "MAX_TOKENS"
        );
        let mut state = Continuation::new(json!({}), None);
        for n in 0..MAX_CHUNKS {
            state.feed(&mut chunk(json!(n.to_string()))).unwrap();
            assert_eq!(state.next().is_err(), n == MAX_CHUNKS - 1);
        }
        let mut state = Continuation::new(json!({}), None);
        state.bytes = MAX_BYTES;
        assert!(state.feed(&mut json!({})).is_err());
    }
}
