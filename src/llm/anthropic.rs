use super::provider::{
    ChatChunk, ChatRequest, ChatResponse, ChunkStream, Message, Provider, Role, ToolCall,
    ToolCallDelta, Usage,
};
use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use eventsource_stream::Eventsource;
use futures::StreamExt;
use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderValue};
use serde_json::{Value, json};

const ANTHROPIC_VERSION: &str = "2023-06-01";

pub struct AnthropicProvider {
    api_key: String,
    http: reqwest::Client,
}

impl AnthropicProvider {
    pub fn new(api_key: String) -> Self {
        Self {
            api_key,
            http: reqwest::Client::builder()
                .connect_timeout(std::time::Duration::from_secs(10))
                .timeout(std::time::Duration::from_secs(300))
                .build()
                .expect("failed to build http client"),
        }
    }

    fn headers(&self) -> Result<HeaderMap> {
        let mut h = HeaderMap::new();
        h.insert(
            "x-api-key",
            HeaderValue::from_str(&self.api_key).context("invalid api key")?,
        );
        h.insert(
            "anthropic-version",
            HeaderValue::from_static(ANTHROPIC_VERSION),
        );
        h.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        Ok(h)
    }

    fn build_body(req: &ChatRequest) -> Value {
        let mut body = json!({
            "model": req.model,
            "messages": Self::messages_json(&req.messages),
            "stream": req.stream,
            "max_tokens": req.max_tokens.unwrap_or(4096),
            "temperature": req.temperature,
        });
        if let Some(system) = &req.system {
            body["system"] = json!(system);
        }
        if !req.tools.is_empty() {
            let tools: Vec<Value> = req
                .tools
                .iter()
                .map(|t| {
                    json!({
                        "name": t.name,
                        "description": t.description,
                        "input_schema": t.parameters,
                    })
                })
                .collect();
            body["tools"] = json!(tools);
        }
        body
    }

    fn messages_json(messages: &[Message]) -> Vec<Value> {
        messages
            .iter()
            .filter(|m| !matches!(m.role, Role::System))
            .filter_map(|m| match m.role {
                Role::User => Some(json!({"role": "user", "content": m.content})),
                Role::Assistant if !m.tool_calls.is_empty() => {
                    let mut blocks: Vec<Value> = Vec::new();
                    if let Some(c) = &m.content {
                        blocks.push(json!({"type": "text", "text": c}));
                    }
                    for c in &m.tool_calls {
                        let args: Value = serde_json::from_str(&c.arguments).unwrap_or(Value::Null);
                        blocks.push(json!({
                            "type": "tool_use",
                            "id": c.id,
                            "name": c.name,
                            "input": args,
                        }));
                    }
                    Some(json!({"role": "assistant", "content": blocks}))
                }
                Role::Assistant => Some(json!({"role": "assistant", "content": m.content})),
                Role::Tool => Some(json!({
                    "role": "user",
                    "content": [{
                        "type": "tool_result",
                        "tool_use_id": m.tool_call_id,
                        "content": m.content,
                    }]
                })),
                Role::System => None,
            })
            .collect()
    }
}

#[async_trait]
impl Provider for AnthropicProvider {
    fn name(&self) -> &str {
        "anthropic"
    }

    async fn chat(&self, req: &ChatRequest) -> Result<ChatResponse> {
        let mut body = Self::build_body(req);
        body["stream"] = json!(false);

        let resp = self
            .http
            .post("https://api.anthropic.com/v1/messages")
            .headers(self.headers()?)
            .json(&body)
            .send()
            .await
            .context("anthropic request failed")?;

        let status = resp.status();
        let text = resp
            .text()
            .await
            .context("anthropic response read failed")?;
        if !status.is_success() {
            bail!("anthropic API error {}: {}", status, text);
        }
        let value: Value =
            serde_json::from_str(&text).context("invalid anthropic JSON response")?;

        let content: String = value
            .get("content")
            .and_then(Value::as_array)
            .map(|blocks| {
                blocks
                    .iter()
                    .filter_map(|b| b.get("text").and_then(Value::as_str))
                    .collect::<Vec<_>>()
                    .join("")
            })
            .unwrap_or_default();

        let tool_calls: Vec<ToolCall> = value
            .get("content")
            .and_then(Value::as_array)
            .map(|blocks| {
                blocks
                    .iter()
                    .filter(|b| b.get("type").and_then(Value::as_str) == Some("tool_use"))
                    .filter_map(|b| {
                        Some(ToolCall {
                            id: b.get("id")?.as_str()?.to_string(),
                            name: b.get("name")?.as_str()?.to_string(),
                            arguments: b.get("input").map(|i| i.to_string()).unwrap_or_default(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        let finish_reason = value
            .get("stop_reason")
            .and_then(Value::as_str)
            .map(String::from);

        let usage = value.get("usage").map(|u| Usage {
            prompt_tokens: u
                .get("input_tokens")
                .and_then(Value::as_u64)
                .map(|v| v as u32),
            completion_tokens: u
                .get("output_tokens")
                .and_then(Value::as_u64)
                .map(|v| v as u32),
            total_tokens: None,
        });

        Ok(ChatResponse {
            content: if content.is_empty() {
                None
            } else {
                Some(content)
            },
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
            .post("https://api.anthropic.com/v1/messages")
            .headers(self.headers()?)
            .json(&body)
            .send()
            .await
            .context("anthropic stream request failed")?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            bail!("anthropic API error {}: {}", status, text);
        }

        let stream = resp
            .bytes_stream()
            .eventsource()
            .filter_map(move |ev| async move {
                let ev = match ev {
                    Ok(e) => e,
                    Err(e) => return Some(Err(anyhow::anyhow!("stream error: {e}"))),
                };
                let data: Value = match serde_json::from_str(&ev.data) {
                    Ok(d) => d,
                    Err(_) => return None,
                };

                let typ = data.get("type").and_then(Value::as_str).unwrap_or("");

                match typ {
                    "content_block_delta" => {
                        let delta = data.get("delta").unwrap_or(&Value::Null);
                        let text = delta.get("text").and_then(Value::as_str);
                        let partial_json = delta.get("partial_json").and_then(Value::as_str);
                        let index = data.get("index").and_then(Value::as_u64).unwrap_or(0) as u32;

                        if let Some(t) = text {
                            Some(Ok(ChatChunk {
                                content: Some(t.to_string()),
                                tool_deltas: vec![],
                                finish_reason: None,
                            }))
                        } else {
                            partial_json.map(|js| {
                                Ok(ChatChunk {
                                    content: None,
                                    tool_deltas: vec![ToolCallDelta {
                                        index,
                                        id: String::new(),
                                        name: String::new(),
                                        args_delta: js.to_string(),
                                    }],
                                    finish_reason: None,
                                })
                            })
                        }
                    }
                    "message_delta" => {
                        let stop = data
                            .get("delta")
                            .and_then(|d| d.get("stop_reason"))
                            .and_then(Value::as_str);
                        Some(Ok(ChatChunk {
                            content: None,
                            tool_deltas: vec![],
                            finish_reason: stop.map(String::from),
                        }))
                    }
                    "content_block_start" => {
                        let block = data.get("content_block").unwrap_or(&Value::Null);
                        if block.get("type").and_then(Value::as_str) == Some("tool_use") {
                            let index =
                                data.get("index").and_then(Value::as_u64).unwrap_or(0) as u32;
                            Some(Ok(ChatChunk {
                                content: None,
                                tool_deltas: vec![ToolCallDelta {
                                    index,
                                    id: block
                                        .get("id")
                                        .and_then(Value::as_str)
                                        .unwrap_or("")
                                        .to_string(),
                                    name: block
                                        .get("name")
                                        .and_then(Value::as_str)
                                        .unwrap_or("")
                                        .to_string(),
                                    args_delta: String::new(),
                                }],
                                finish_reason: None,
                            }))
                        } else {
                            None
                        }
                    }
                    _ => None,
                }
            })
            .boxed();

        Ok(stream)
    }
}
