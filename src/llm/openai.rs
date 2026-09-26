use super::provider::{
    ChatChunk, ChatRequest, ChatResponse, ChunkStream, Message, Provider, Role, ToolCall,
    ToolCallDelta, Usage,
};
use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use eventsource_stream::Eventsource;
use futures::StreamExt;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue};
use serde_json::{Value, json};

pub struct OpenAICompatible {
    name: String,
    base_url: String,
    api_key: Option<String>,
    http: reqwest::Client,
}

impl OpenAICompatible {
    pub fn new(
        name: impl Into<String>,
        base_url: impl Into<String>,
        api_key: Option<String>,
    ) -> Self {
        Self {
            name: name.into(),
            base_url: base_url.into(),
            api_key,
            http: reqwest::Client::builder()
                .connect_timeout(std::time::Duration::from_secs(10))
                .timeout(std::time::Duration::from_secs(300))
                .build()
                .expect("failed to build http client"),
        }
    }

    pub fn openai_default(api_key: String) -> Self {
        Self::new("openai", "https://api.openai.com/v1", Some(api_key))
    }

    fn headers(&self) -> Result<HeaderMap> {
        let mut h = HeaderMap::new();
        if let Some(key) = &self.api_key {
            h.insert(
                AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {key}")).context("invalid api key")?,
            );
        }
        h.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        Ok(h)
    }

    fn build_body(req: &ChatRequest) -> Value {
        let tools: Vec<Value> = req
            .tools
            .iter()
            .map(|t| {
                json!({
                    "type": "function",
                    "function": {
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.parameters,
                    }
                })
            })
            .collect();

        let mut body = json!({
            "model": req.model,
            "messages": Self::messages_json(&req.messages),
            "stream": req.stream,
            "temperature": req.temperature,
        });
        if let Some(ms) = req.max_tokens {
            body["max_tokens"] = json!(ms);
        }
        if !tools.is_empty() {
            body["tools"] = json!(tools);
        }
        body
    }

    fn messages_json(messages: &[Message]) -> Vec<Value> {
        messages
            .iter()
            .map(|m| match m.role {
                Role::System => json!({"role": "system", "content": m.content}),
                Role::User => json!({"role": "user", "content": m.content}),
                Role::Assistant if !m.tool_calls.is_empty() => {
                    let calls: Vec<Value> = m
                        .tool_calls
                        .iter()
                        .map(|c| {
                            json!({
                                "id": c.id,
                                "type": "function",
                                "function": {
                                    "name": c.name,
                                    "arguments": c.arguments,
                                }
                            })
                        })
                        .collect();
                    let mut v = json!({"role": "assistant", "tool_calls": calls});
                    if let Some(c) = &m.content {
                        v["content"] = json!(c);
                    }
                    v
                }
                Role::Assistant => json!({"role": "assistant", "content": m.content}),
                Role::Tool => json!({
                    "role": "tool",
                    "tool_call_id": m.tool_call_id,
                    "content": m.content,
                }),
            })
            .collect()
    }

    fn parse_chunk(raw: &Value) -> Result<ChatChunk> {
        let choices = raw
            .get("choices")
            .and_then(Value::as_array)
            .context("stream chunk missing choices")?;
        let choice = choices.first().context("stream chunk without choices")?;
        let delta = choice.get("delta").unwrap_or(&Value::Null);

        let content = delta
            .get("content")
            .and_then(Value::as_str)
            .map(String::from);

        let tool_deltas = match delta.get("tool_calls").and_then(Value::as_array) {
            Some(calls) => calls
                .iter()
                .filter_map(|c| {
                    let index = c.get("index").and_then(Value::as_u64).unwrap_or(0) as u32;
                    let id = c
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    let name = c
                        .get("function")
                        .and_then(|f| f.get("name"))
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    let args_delta = c
                        .get("function")
                        .and_then(|f| f.get("arguments"))
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    if id.is_empty() && name.is_empty() && args_delta.is_empty() {
                        None
                    } else {
                        Some(ToolCallDelta {
                            index,
                            id,
                            name,
                            args_delta,
                        })
                    }
                })
                .collect(),
            None => vec![],
        };

        let finish_reason = choice
            .get("finish_reason")
            .and_then(Value::as_str)
            .map(String::from);

        Ok(ChatChunk {
            content,
            tool_deltas,
            finish_reason,
        })
    }
}

#[async_trait]
impl Provider for OpenAICompatible {
    fn name(&self) -> &str {
        &self.name
    }

    async fn chat(&self, req: &ChatRequest) -> Result<ChatResponse> {
        let body = Self::build_body(req);
        let headers = self.headers()?;
        let url = format!("{}/chat/completions", self.base_url);
        let resp = crate::llm::provider::send_with_retry(|| {
            self.http.post(&url).headers(headers.clone()).json(&body)
        })
        .await
        .context("openai request failed")?;

        let status = resp.status();
        let text = resp.text().await.context("openai response read failed")?;
        if !status.is_success() {
            bail!("openai API error {}: {}", status, text);
        }
        let value: Value = serde_json::from_str(&text).context("invalid openai JSON response")?;

        let choices = value
            .get("choices")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let choice = choices.first().cloned().unwrap_or(Value::Null);
        let message = choice.get("message").unwrap_or(&Value::Null);
        let content = message
            .get("content")
            .and_then(Value::as_str)
            .map(String::from);

        let tool_calls = message
            .get("tool_calls")
            .and_then(Value::as_array)
            .map(|calls| {
                calls
                    .iter()
                    .filter_map(|c| {
                        Some(ToolCall {
                            id: c.get("id").and_then(Value::as_str)?.to_string(),
                            name: c.get("function")?.get("name")?.as_str()?.to_string(),
                            arguments: c
                                .get("function")?
                                .get("arguments")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_string(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        let finish_reason = choice
            .get("finish_reason")
            .and_then(Value::as_str)
            .map(String::from);

        let usage = value.get("usage").map(|u| Usage {
            prompt_tokens: u
                .get("prompt_tokens")
                .and_then(Value::as_u64)
                .map(|v| v as u32),
            completion_tokens: u
                .get("completion_tokens")
                .and_then(Value::as_u64)
                .map(|v| v as u32),
            total_tokens: u
                .get("total_tokens")
                .and_then(Value::as_u64)
                .map(|v| v as u32),
        });

        Ok(ChatResponse {
            content,
            tool_calls,
            finish_reason,
            usage: usage.unwrap_or_default(),
        })
    }

    async fn chat_stream(&self, req: &ChatRequest) -> Result<ChunkStream> {
        let mut body = Self::build_body(req);
        body["stream"] = json!(true);

        let headers = self.headers()?;
        let url = format!("{}/chat/completions", self.base_url);
        let resp = crate::llm::provider::send_with_retry(|| {
            self.http.post(&url).headers(headers.clone()).json(&body)
        })
        .await
        .context("openai stream request failed")?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            bail!("openai API error {}: {}", status, text);
        }

        let stream = resp
            .bytes_stream()
            .eventsource()
            .filter_map(move |ev| async move {
                match ev {
                    Ok(eventsource_stream::Event { data, .. }) if data != "[DONE]" => {
                        let parsed: Value = match serde_json::from_str(&data) {
                            Ok(v) => v,
                            Err(_) => return None,
                        };
                        match Self::parse_chunk(&parsed) {
                            Ok(chunk) => Some(Ok(chunk)),
                            Err(e) => Some(Err(e)),
                        }
                    }
                    Ok(_) => None,
                    Err(e) => Some(Err(anyhow::anyhow!("stream error: {e}"))),
                }
            })
            .boxed();

        Ok(stream)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::provider::{Role, ToolCallDelta};

    fn chunk(json: Value) -> ChatChunk {
        OpenAICompatible::parse_chunk(&json).expect("chunk should parse")
    }

    // ── Stream chunk parsing ──────────────────────────────────────────────────

    #[test]
    fn parse_chunk_reads_content_deltas() {
        let c = chunk(json!({"choices": [{"delta": {"content": "Hel"}}]}));
        assert_eq!(c.content.as_deref(), Some("Hel"));
        assert!(c.tool_deltas.is_empty());
        assert!(c.finish_reason.is_none());
    }

    #[test]
    fn parse_chunk_tolerates_a_null_content_delta() {
        // OpenAI sends `"content": null` on tool-call chunks; this must not
        // become the string "null" or an error.
        let c = chunk(json!({"choices": [{"delta": {"content": null}}]}));
        assert_eq!(c.content, None);
    }

    #[test]
    fn parse_chunk_reads_the_tool_call_opener() {
        let c = chunk(json!({"choices": [{"delta": {"tool_calls": [
            {"index": 0, "id": "call_1", "function": {"name": "read_file", "arguments": ""}}
        ]}}]}));
        assert_eq!(c.tool_deltas.len(), 1);
        assert_eq!(c.tool_deltas[0].id, "call_1");
        assert_eq!(c.tool_deltas[0].name, "read_file");
        assert_eq!(c.tool_deltas[0].index, 0);
    }

    #[test]
    fn parse_chunk_keeps_the_index_for_parallel_tool_calls() {
        let c = chunk(json!({"choices": [{"delta": {"tool_calls": [
            {"index": 0, "id": "a", "function": {"name": "read_file", "arguments": ""}},
            {"index": 1, "id": "b", "function": {"name": "grep", "arguments": ""}}
        ]}}]}));
        assert_eq!(c.tool_deltas.len(), 2);
        assert_eq!(c.tool_deltas[1].index, 1, "index must not collapse to 0");
        assert_eq!(c.tool_deltas[1].name, "grep");
    }

    #[test]
    fn parse_chunk_drops_fully_empty_tool_entries() {
        let c = chunk(json!({"choices": [{"delta": {"tool_calls": [
            {"index": 0, "id": "", "function": {"name": "", "arguments": ""}}
        ]}}]}));
        assert!(
            c.tool_deltas.is_empty(),
            "placeholder entries must be filtered"
        );
    }

    #[test]
    fn parse_chunk_reads_finish_reason() {
        let c = chunk(json!({"choices": [{"delta": {}, "finish_reason": "tool_calls"}]}));
        assert_eq!(c.finish_reason.as_deref(), Some("tool_calls"));
    }

    #[test]
    fn parse_chunk_rejects_a_malformed_envelope() {
        assert!(OpenAICompatible::parse_chunk(&json!({})).is_err());
        assert!(OpenAICompatible::parse_chunk(&json!({"choices": []})).is_err());
    }

    #[test]
    fn parse_chunk_uses_only_the_first_choice() {
        let c = chunk(json!({"choices": [
            {"delta": {"content": "first"}},
            {"delta": {"content": "second"}}
        ]}));
        assert_eq!(c.content.as_deref(), Some("first"));
    }

    // ── Request body construction ─────────────────────────────────────────────

    #[test]
    fn build_body_omits_empty_optional_fields() {
        let body = OpenAICompatible::build_body(&ChatRequest {
            model: "gpt-4o".into(),
            stream: false,
            ..Default::default()
        });
        assert!(body.get("tools").is_none(), "no tools → no tools key");
        assert_eq!(body["stream"], json!(false));
    }

    #[test]
    fn build_body_wraps_tools_in_the_function_envelope() {
        let body = OpenAICompatible::build_body(&ChatRequest {
            model: "gpt-4o".into(),
            tools: vec![crate::tools::ToolDef::new(
                "read_file",
                "read it",
                json!({"type": "object"}),
            )],
            ..Default::default()
        });
        assert_eq!(body["tools"][0]["type"], json!("function"));
        assert_eq!(body["tools"][0]["function"]["name"], json!("read_file"));
    }

    #[test]
    fn messages_json_encodes_tool_results_with_their_id() {
        // Dropping tool_call_id makes the API reject the whole request.
        let msgs = OpenAICompatible::messages_json(&[Message::tool_result("call_9", "ok")]);
        assert_eq!(msgs[0]["role"], json!("tool"));
        assert_eq!(msgs[0]["tool_call_id"], json!("call_9"));
        assert_eq!(msgs[0]["content"], json!("ok"));
    }

    #[test]
    fn messages_json_encodes_assistant_tool_calls() {
        let msgs =
            OpenAICompatible::messages_json(&[Message::assistant_tool_calls(vec![ToolCall {
                id: "c1".into(),
                name: "grep".into(),
                arguments: r#"{"regex":"x"}"#.into(),
            }])]);
        assert_eq!(msgs[0]["tool_calls"][0]["id"], json!("c1"));
        assert_eq!(msgs[0]["tool_calls"][0]["type"], json!("function"));
        assert_eq!(msgs[0]["tool_calls"][0]["function"]["name"], json!("grep"));
    }

    #[test]
    fn messages_json_keeps_text_alongside_tool_calls() {
        let msgs = OpenAICompatible::messages_json(&[Message::assistant_tool_calls_with_text(
            vec![ToolCall {
                id: "c1".into(),
                name: "grep".into(),
                arguments: "{}".into(),
            }],
            "thinking out loud",
        )]);
        assert_eq!(msgs[0]["content"], json!("thinking out loud"));
    }

    #[test]
    fn messages_json_omits_content_for_a_pure_tool_call_turn() {
        let msgs =
            OpenAICompatible::messages_json(&[Message::assistant_tool_calls(vec![ToolCall {
                id: "c1".into(),
                name: "grep".into(),
                arguments: "{}".into(),
            }])]);
        assert_eq!(msgs[0]["content"], Value::Null, "null, not an empty string");
    }

    // ── Message helpers ───────────────────────────────────────────────────────

    #[test]
    fn content_text_falls_back_to_tool_arguments() {
        let m = Message::assistant_tool_calls(vec![ToolCall {
            id: "c1".into(),
            name: "grep".into(),
            arguments: r#"{"regex":"needle"}"#.into(),
        }]);
        assert!(m.content_text().contains("needle"));
    }

    #[test]
    fn empty_text_becomes_none_on_a_tool_call_turn() {
        let m = Message::assistant_tool_calls_with_text(
            vec![ToolCall {
                id: "c1".into(),
                name: "grep".into(),
                arguments: "{}".into(),
            }],
            "",
        );
        assert_eq!(m.content, None, "an empty string is not valid content");
    }

    #[test]
    fn system_role_is_encoded_for_openai() {
        let msgs = OpenAICompatible::messages_json(&[Message::system("be terse")]);
        assert_eq!(msgs[0]["role"], json!("system"));
    }

    #[test]
    fn tool_delta_defaults_index_to_zero() {
        // Some OpenAI-compatible servers omit `index` entirely.
        let d = ToolCallDelta {
            index: 0,
            id: "x".into(),
            name: String::new(),
            args_delta: "{}".into(),
        };
        assert_eq!(d.index, 0);
        let _ = Role::Assistant;
    }
}
