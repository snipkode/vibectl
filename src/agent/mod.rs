pub mod steer;

use crate::tools::{Tool, ToolDef};
use anyhow::{Context, Result};
use async_trait::async_trait;
use futures::StreamExt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

use crate::llm::provider::{ChatRequest, Message, Provider, ToolCall, ToolCallDelta};

#[derive(Debug, Clone, PartialEq)]
pub enum Approval {
    Allow,
    Deny,
}

#[async_trait]
pub trait Approver: Send + Sync {
    async fn approve(&self, description: String) -> Approval;
}

#[allow(dead_code)]
pub enum AgentEvent {
    Text(String),
    ToolCall {
        id: String,
        name: String,
    },
    ToolResult {
        id: String,
        name: String,
        content: String,
    },
    ToolError {
        id: String,
        name: String,
        error: String,
    },
    Done {
        finish_reason: Option<String>,
    },
    Error(String),
}

#[derive(Clone)]
pub struct Agent {
    pub model: String,
    pub temperature: f32,
    pub max_tokens: Option<u32>,
    pub system: String,
    pub provider: Arc<dyn Provider>,
    pub tools: Vec<ToolDef>,
    pub cwd: PathBuf,
    pub approver: Option<Arc<dyn Approver>>,
    pub allow_any_path: bool,
    messages: Arc<Mutex<Vec<Message>>>,
}

impl Agent {
    pub fn new(
        model: String,
        system: String,
        provider: Arc<dyn Provider>,
        tool_impls: &[Box<dyn Tool>],
        cwd: PathBuf,
    ) -> Self {
        let tools = tool_impls.iter().map(|t| t.def()).collect();
        Self {
            model,
            temperature: 0.2,
            max_tokens: Some(4096),
            system,
            provider,
            tools,
            cwd,
            approver: None,
            allow_any_path: false,
            messages: Arc::new(Mutex::new(vec![])),
        }
    }

    async fn snapshot(&self) -> Vec<Message> {
        self.messages.lock().unwrap().clone()
    }

    async fn push(&self, msg: Message) {
        self.messages.lock().unwrap().push(msg);
    }

    async fn trim_history(&self, keep_last: usize) {
        // Keep history bounded: drop oldest user/assistant turns beyond a cap.
        let mut messages = self.messages.lock().unwrap();
        if messages.len() > keep_last {
            let new_len = messages.len() - keep_last + 1;
            let mut pruned = messages.split_off(new_len);
            if pruned.len() >= keep_last {
                let first = pruned.remove(0);
                messages.clear();
                messages.push(first);
                messages.append(&mut pruned);
            }
        }
    }

    pub fn set_history(&self, v: Vec<Message>) {
        *self.messages.lock().unwrap() = v;
    }

    #[allow(dead_code)]
    pub fn history(&self) -> Vec<Message> {
        self.messages.lock().unwrap().clone()
    }

    pub async fn plan(&self, task: &str) -> Result<String> {
        let req = ChatRequest {
            model: self.model.clone(),
            messages: vec![
                Message::system(crate::agent::steer::plan_system_prompt()),
                Message::user(task.to_string()),
            ],
            temperature: 0.1,
            max_tokens: Some(1024),
            stream: false,
            tools: vec![],
            system: None,
        };
        let resp = self.provider.chat(&req).await?;
        resp.content.context("plan produced no output")
    }

    fn merge_tool_deltas(slots: &mut Vec<ToolCall>, deltas: &[ToolCallDelta]) {
        for d in deltas {
            if slots.len() <= d.index as usize {
                let placeholder = ToolCall {
                    id: String::new(),
                    name: String::new(),
                    arguments: String::new(),
                };
                while slots.len() < d.index as usize {
                    slots.push(placeholder.clone());
                }
                slots.push(ToolCall {
                    id: d.id.clone(),
                    name: d.name.clone(),
                    arguments: String::new(),
                });
            }
            let slot = &mut slots[d.index as usize];
            if !d.id.is_empty() {
                slot.id = d.id.clone();
            }
            if !d.name.is_empty() {
                slot.name = d.name.clone();
            }
            slot.arguments.push_str(&d.args_delta);
        }
    }

    pub fn spawn_run(
        &self,
        user_input: String,
    ) -> (mpsc::Receiver<AgentEvent>, tokio::task::JoinHandle<()>) {
        let (tx, rx) = mpsc::channel(256);
        let this = self.clone();
        let handle = tokio::spawn(async move {
            if let Err(e) = this.run_inner(tx.clone(), user_input).await {
                let _ = tx.send(AgentEvent::Error(e.to_string())).await;
            }
        });
        (rx, handle)
    }

    async fn run_inner(&self, tx: mpsc::Sender<AgentEvent>, user_input: String) -> Result<()> {
        self.trim_history(80).await;
        self.push(Message::user(user_input)).await;

        loop {
            let messages = self.snapshot().await;
            let req = ChatRequest {
                model: self.model.clone(),
                messages,
                temperature: self.temperature,
                max_tokens: self.max_tokens,
                stream: true,
                tools: self.tools.clone(),
                system: Some(self.system.clone()),
            };

            let stream = self
                .provider
                .chat_stream(&req)
                .await
                .context("LLM request failed")?;

            let mut text = String::new();
            let mut tool_slots: Vec<ToolCall> = Vec::new();
            let mut finish_reason: Option<String> = None;

            let mut stream = stream;
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.context("LLM stream error")?;
                if let Some(c) = chunk.content {
                    text.push_str(&c);
                    let _ = tx.send(AgentEvent::Text(c)).await;
                }
                Self::merge_tool_deltas(&mut tool_slots, &chunk.tool_deltas);
                if let Some(f) = chunk.finish_reason {
                    finish_reason = Some(f);
                }
            }

            let clean_calls: Vec<ToolCall> = tool_slots
                .into_iter()
                .filter(|c| !c.name.is_empty())
                .collect();

            if !clean_calls.is_empty() {
                self.push(Message::assistant_tool_calls(clean_calls.clone()))
                    .await;
                for call in &clean_calls {
                    let _ = tx
                        .send(AgentEvent::ToolCall {
                            id: call.id.clone(),
                            name: call.name.clone(),
                        })
                        .await;
                    match self.execute_tool(call).await {
                        Ok(res) => {
                            self.push(Message::tool_result(call.id.clone(), res.content.clone()))
                                .await;
                            let _ = tx
                                .send(AgentEvent::ToolResult {
                                    id: call.id.clone(),
                                    name: call.name.clone(),
                                    content: res.content,
                                })
                                .await;
                        }
                        Err(e) => {
                            let msg = format!("Tool error: {e}");
                            self.push(Message::tool_result(call.id.clone(), msg.clone()))
                                .await;
                            let _ = tx
                                .send(AgentEvent::ToolError {
                                    id: call.id.clone(),
                                    name: call.name.clone(),
                                    error: msg,
                                })
                                .await;
                        }
                    }
                }
                continue;
            }

            self.push(Message::assistant(text)).await;
            let _ = tx.send(AgentEvent::Done { finish_reason }).await;
            break;
        }

        Ok(())
    }

    async fn execute_tool(&self, call: &ToolCall) -> Result<crate::tools::ToolResult> {
        let name = call.name.clone();
        let tool_impls = crate::tools::all_tools();
        let tool = tool_impls
            .iter()
            .find(|t| t.def().name == name)
            .with_context(|| format!("unknown tool: {name}"))?;
        let args: serde_json::Value = serde_json::from_str(&call.arguments)
            .with_context(|| format!("invalid args for tool {name}: {}", call.arguments))?;

        if name == "shell_exec" {
            let cmd = args
                .get("command")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_string();
            if let Some(approver) = &self.approver
                && let Approval::Deny = approver.approve(format!("$ {cmd}")).await
            {
                return Ok(crate::tools::ToolResult {
                    content: format!("Command rejected by user approval: {cmd}"),
                });
            }
            return tool.run(&args, &self.cwd);
        }

        if name == "write_file" {
            let path_str = args
                .get("path")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_string();
            let target = crate::tools::read_file::resolve_path(&self.cwd, &path_str);
            let root = crate::agent::steer::find_project_root(&self.cwd)
                .unwrap_or_else(|| self.cwd.clone());
            let outside = !target.starts_with(&root);

            if outside && !self.allow_any_path {
                return Ok(crate::tools::ToolResult {
                    content: format!(
                        "SAFEGUARD: refusing to write {} — outside the project root {}. \
                         Writes are confined to the project. Override in config with allow_any_path: true.",
                        target.display(),
                        root.display()
                    ),
                });
            }

            if let Some(approver) = &self.approver {
                let bytes = args
                    .get("content")
                    .and_then(serde_json::Value::as_str)
                    .map(str::len)
                    .unwrap_or(0);
                let op = if target.exists() { "update" } else { "create" };
                let mut desc = format!("{op}: {} ({} bytes)", target.display(), bytes);
                if outside {
                    desc = format!("OUTSIDE PROJECT: {desc}");
                }
                if crate::agent::steer::read_plan(&self.cwd)
                    .ok()
                    .flatten()
                    .is_none()
                {
                    desc = format!("[no plan — run /plan <task> first] {desc}");
                }
                if let Approval::Deny = approver.approve(desc).await {
                    return Ok(crate::tools::ToolResult {
                        content: format!("Write rejected by user approval: {}", target.display()),
                    });
                }
            }
            return tool.run(&args, &self.cwd);
        }

        tool.run(&args, &self.cwd)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_creates_slot_with_args() {
        let mut slots = vec![];
        Agent::merge_tool_deltas(
            &mut slots,
            &[ToolCallDelta {
                index: 0,
                id: "c1".into(),
                name: "read_file".into(),
                args_delta: "".into(),
            }],
        );
        Agent::merge_tool_deltas(
            &mut slots,
            &[ToolCallDelta {
                index: 0,
                id: "".into(),
                name: "".into(),
                args_delta: "{\"path\":\"x\"}".into(),
            }],
        );
        assert_eq!(slots.len(), 1);
        assert_eq!(slots[0].id, "c1");
        assert_eq!(slots[0].name, "read_file");
        assert_eq!(slots[0].arguments, "{\"path\":\"x\"}");
    }

    #[test]
    fn merge_two_independent_calls() {
        let mut slots = vec![];
        Agent::merge_tool_deltas(
            &mut slots,
            &[
                ToolCallDelta {
                    index: 0,
                    id: "c1".into(),
                    name: "read_file".into(),
                    args_delta: "".into(),
                },
                ToolCallDelta {
                    index: 1,
                    id: "c2".into(),
                    name: "write_file".into(),
                    args_delta: "".into(),
                },
            ],
        );
        Agent::merge_tool_deltas(
            &mut slots,
            &[
                ToolCallDelta {
                    index: 0,
                    id: "".into(),
                    name: "".into(),
                    args_delta: "{\"path\":\"a\"}".into(),
                },
                ToolCallDelta {
                    index: 1,
                    id: "".into(),
                    name: "".into(),
                    args_delta: "{\"path\":\"b\"}".into(),
                },
            ],
        );
        assert_eq!(slots.len(), 2);
        assert_eq!(slots[0].name, "read_file");
        assert_eq!(slots[1].name, "write_file");
    }

    #[test]
    fn same_index_not_duplicated() {
        let mut slots = vec![];
        Agent::merge_tool_deltas(
            &mut slots,
            &[ToolCallDelta {
                index: 0,
                id: "c1".into(),
                name: "r".into(),
                args_delta: "".into(),
            }],
        );
        Agent::merge_tool_deltas(
            &mut slots,
            &[ToolCallDelta {
                index: 0,
                id: "".into(),
                name: "".into(),
                args_delta: "x".into(),
            }],
        );
        Agent::merge_tool_deltas(
            &mut slots,
            &[ToolCallDelta {
                index: 0,
                id: "".into(),
                name: "".into(),
                args_delta: "y".into(),
            }],
        );
        assert_eq!(slots.len(), 1);
        assert_eq!(slots[0].arguments, "xy");
    }
}
