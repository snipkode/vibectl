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
        let resp = self
            .http
            .post(format!("{}/chat/completions", self.base_url))
            .headers(self.headers()?)
            .json(&body)
            .send()
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

        let resp = self
            .http
            .post(format!("{}/chat/completions", self.base_url))
            .headers(self.headers()?)
            .json(&body)
            .send()
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
