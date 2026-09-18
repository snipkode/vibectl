pub mod checkpoint;
pub mod intent;
pub mod steer;

use crate::tools::{Tool, ToolDef};
use anyhow::{Context, Result};
use async_trait::async_trait;
use futures::StreamExt;
use similar::{ChangeTag, TextDiff};
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
    /// Auto-generated plan emitted before execution for complex tasks.
    Plan(String),
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
    /// Cached tool implementations — avoids re-constructing on every tool call.
    pub tool_impls: Arc<Vec<Box<dyn Tool>>>,
    pub cwd: PathBuf,
    pub approver: Option<Arc<dyn Approver>>,
    pub allow_any_path: bool,
    /// When true, complex tasks automatically trigger a planning phase before execution.
    pub auto_plan: bool,
    /// Tracks whether a checkpoint stash has been created for the current run.
    /// Reset to false by spawn_run so each run gets at most one checkpoint.
    checkpoint_taken: Arc<Mutex<bool>>,
    messages: Arc<Mutex<Vec<Message>>>,
}

impl Agent {
    pub fn new(
        model: String,
        system: String,
        provider: Arc<dyn Provider>,
        tool_impls: Vec<Box<dyn Tool>>,
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
            tool_impls: Arc::new(tool_impls),
            cwd,
            approver: None,
            allow_any_path: false,
            auto_plan: true,
            checkpoint_taken: Arc::new(Mutex::new(false)),
            messages: Arc::new(Mutex::new(vec![])),
        }
    }

    async fn snapshot(&self) -> Vec<Message> {
        self.messages.lock().unwrap().clone()
    }

    async fn push(&self, msg: Message) {
        self.messages.lock().unwrap().push(msg);
    }

    /// Estimate token count for a string (rough: 1 token ≈ 4 chars).
    fn estimate_tokens(s: &str) -> usize {
        (s.len() + 3) / 4
    }

    /// Estimate token count for a single message (role + content).
    fn message_tokens(msg: &Message) -> usize {
        // role ~4 tokens overhead, plus content
        4 + Self::estimate_tokens(&msg.content_text())
    }

    /// Trim history so total estimated tokens stay within `max_tokens`.
    /// Always preserves the most recent messages. Never removes the system
    /// message (which lives in `self.system`, not in the history vec).
    async fn trim_history_by_tokens(&self, max_tokens: usize) {
        let mut messages = self.messages.lock().unwrap();
        if messages.is_empty() {
            return;
        }

        // Calculate total tokens from back to front.
        // We must keep at minimum the last message (the new user input).
        let mut kept = 0usize;
        let mut total = 0usize;
        for msg in messages.iter().rev() {
            let t = Self::message_tokens(msg);
            if total + t > max_tokens && kept > 0 {
                break;
            }
            total += t;
            kept += 1;
        }

        let drop_count = messages.len().saturating_sub(kept);
        if drop_count > 0 {
            messages.drain(0..drop_count);
        }
    }

    pub fn set_history(&self, v: Vec<Message>) {
        *self.messages.lock().unwrap() = v;
    }

    #[allow(dead_code)]
    pub fn history(&self) -> Vec<Message> {
        self.messages.lock().unwrap().clone()
    }

    /// Score the complexity of a user prompt (0–10).
    /// Returns a score ≥ PLAN_THRESHOLD if auto-planning should trigger.
    pub fn complexity_score(input: &str) -> u8 {
        let lower = input.to_lowercase();
        let word_count = input.split_whitespace().count();

        let mut score: u8 = 0;

        // Long prompts are inherently more complex.
        if word_count > 50 {
            score += 3;
        } else if word_count > 25 {
            score += 2;
        } else if word_count > 15 {
            score += 1;
        }

        // Action verbs that imply multi-step work.
        let complex_verbs = [
            "implement",
            "refactor",
            "migrate",
            "redesign",
            "rewrite",
            "add feature",
            "create",
            "build",
            "integrate",
            "setup",
            "convert",
            "upgrade",
            "extract",
            "split",
            "merge",
        ];
        for verb in &complex_verbs {
            if lower.contains(verb) {
                score += 2;
                break;
            }
        }

        // Multiple targets — multi-step connectors.
        let connectors = ["and then", " and ", " then ", " also ", " plus ", " + "];
        for conn in &connectors {
            if lower.contains(conn) {
                score += 1;
                break;
            }
        }

        // File count hints — count total file-like tokens (word contains a dot + ext).
        let file_token_count = input
            .split_whitespace()
            .filter(|w| {
                let w = w.trim_matches(|c: char| !c.is_alphanumeric() && c != '.' && c != '_');
                let exts = [
                    ".rs", ".py", ".js", ".ts", ".go", ".toml", ".md", ".json", ".yaml",
                ];
                exts.iter().any(|e| w.ends_with(e))
            })
            .count();
        if file_token_count >= 3 {
            score += 2;
        } else if file_token_count >= 2 {
            score += 1;
        }

        // "all files", "every file" hints.
        if lower.contains("all files") || lower.contains("every file") {
            score += 2;
        }

        // Explicit scope words.
        let scope_words = ["across", "throughout", "everywhere", "all of", "entire"];
        for w in &scope_words {
            if lower.contains(w) {
                score += 1;
                break;
            }
        }

        score.min(10)
    }

    /// Complexity score threshold above which auto-planning triggers.
    pub const PLAN_THRESHOLD: u8 = 3;

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

    /// Tools that are safe to run in parallel (read-only, no side effects).
    fn is_readonly_tool(name: &str) -> bool {
        matches!(
            name,
            "read_file" | "glob" | "grep" | "git" | "web_fetch" | "list_symbols"
        )
    }

    pub fn spawn_run(
        &self,
        user_input: String,
    ) -> (mpsc::Receiver<AgentEvent>, tokio::task::JoinHandle<()>) {
        // Reset checkpoint flag — each run gets at most one auto-checkpoint.
        *self.checkpoint_taken.lock().unwrap() = false;

        // Use current timestamp as a simple unique run ID for the stash message.
        let run_id = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        let (tx, rx) = mpsc::channel(256);
        let this = self.clone();
        let handle = tokio::spawn(async move {
            if let Err(e) = this.run_inner(tx.clone(), user_input, run_id).await {
                let _ = tx.send(AgentEvent::Error(e.to_string())).await;
            }
        });
        (rx, handle)
    }

    async fn run_inner(
        &self,
        tx: mpsc::Sender<AgentEvent>,
        user_input: String,
        run_id: u64,
    ) -> Result<()> {
        // Token-aware trim: keep history within ~80k tokens (safe for 128k context models).
        // System prompt itself is injected separately and not counted here.
        self.trim_history_by_tokens(80_000).await;

        // ── Auto-planning phase ────────────────────────────────────────────────
        // For complex tasks, generate a plan first and inject it into the context
        // so the LLM executes with clear step-by-step guidance.
        // Only runs on the first turn (history is empty before we push the user msg).
        let is_first_turn = self.messages.lock().unwrap().is_empty();

        // Classify intent first so we can force plan for Refactor regardless of score.
        // Note: intent classification happens before push(user_input) so history is
        // still empty at this point — that's intentional.
        let user_intent =
            intent::classify_intent(&user_input, &self.model, self.provider.clone()).await;

        // Force planning for Refactor intent (always) or complex tasks (heuristic).
        let should_plan = is_first_turn
            && self.auto_plan
            && (user_intent == intent::Intent::Refactor
                || (user_input.split_whitespace().count() >= 4
                    && Self::complexity_score(&user_input) >= Self::PLAN_THRESHOLD));

        if should_plan {
            match self.plan(&user_input).await {
                Ok(plan) => {
                    let _ = crate::agent::steer::save_plan(&self.cwd, &plan);
                    let _ = tx.send(AgentEvent::Plan(plan.clone())).await;
                    self.push(Message::system(format!(
                        "Auto-generated implementation plan for this task:\n\n{plan}\n\n\
                         Execute the steps above. Use tools to inspect, then implement."
                    )))
                    .await;
                }
                Err(e) => {
                    let _ = tx
                        .send(AgentEvent::Text(format!(
                            "[auto-plan failed: {e} — proceeding without plan]\n"
                        )))
                        .await;
                }
            }
        }

        self.push(Message::user(user_input.clone())).await;

        // Build tool list based on intent — protocol-level enforcement.
        let intent_tools = self.tools_for_intent(&user_intent);

        loop {
            let messages = self.snapshot().await;
            let req = ChatRequest {
                model: self.model.clone(),
                messages,
                temperature: self.temperature,
                max_tokens: self.max_tokens,
                stream: true,
                tools: intent_tools.clone(),
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

            // Ensure all tool calls have a non-empty ID.
            // Some providers (Ollama, some fine-tuned models) omit the id field.
            // Without a stable id the tool_result message will have tool_call_id=""
            // which many providers silently drop, breaking the conversation context.
            for (i, slot) in tool_slots.iter_mut().enumerate() {
                if slot.id.is_empty() {
                    slot.id = format!("call_{run_id}_{i}");
                }
            }

            let clean_calls: Vec<ToolCall> = tool_slots
                .into_iter()
                .filter(|c| !c.name.is_empty())
                .collect();

            // Safety guard: if the input was classified as conversational but
            // the model still emitted tool calls (e.g. older fine-tuned model),
            // discard them and treat the response as a plain text reply.
            if user_intent == intent::Intent::Conversational && !clean_calls.is_empty() {
                self.push(Message::assistant(text)).await;
                let _ = tx.send(AgentEvent::Done { finish_reason }).await;
                break;
            }

            if !clean_calls.is_empty() {
                // Push assistant message that contains BOTH the streamed text (if any)
                // AND the tool_calls. This preserves any "thinking" text in history
                // and ensures the tool_call ids are present for tool_result matching.
                self.push(Message::assistant_tool_calls_with_text(
                    clean_calls.clone(),
                    text.clone(),
                ))
                .await;

                // ── Partition into read-only (parallel) and approval-required (serial) ──
                //
                // All calls in this batch are either ALL read-only or MIXED.
                // If mixed, we run serially to preserve ordering semantics.
                // If all read-only, run concurrently.
                let all_readonly = clean_calls.iter().all(|c| Self::is_readonly_tool(&c.name));

                if all_readonly && clean_calls.len() > 1 {
                    // Emit ToolCall events first (ordering preserved).
                    for call in &clean_calls {
                        let _ = tx
                            .send(AgentEvent::ToolCall {
                                id: call.id.clone(),
                                name: call.name.clone(),
                            })
                            .await;
                    }

                    // Execute all in parallel.
                    let futures: Vec<_> = clean_calls
                        .iter()
                        .map(|call| self.execute_tool(call, run_id, &tx))
                        .collect();
                    let results = futures::future::join_all(futures).await;

                    // Push results in original order.
                    for (call, result) in clean_calls.iter().zip(results) {
                        match result {
                            Ok(res) => {
                                self.push(Message::tool_result(
                                    call.id.clone(),
                                    res.content.clone(),
                                ))
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
                } else {
                    // Serial execution: approval tools or mixed batch.
                    for call in &clean_calls {
                        let _ = tx
                            .send(AgentEvent::ToolCall {
                                id: call.id.clone(),
                                name: call.name.clone(),
                            })
                            .await;
                        match self.execute_tool(call, run_id, &tx).await {
                            Ok(res) => {
                                self.push(Message::tool_result(
                                    call.id.clone(),
                                    res.content.clone(),
                                ))
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
                }

                continue;
            }

            self.push(Message::assistant(text)).await;
            let _ = tx.send(AgentEvent::Done { finish_reason }).await;
            break;
        }

        Ok(())
    }

    /// Return the subset of tools to expose for a given intent.
    /// This is enforced at the protocol level — the LLM only sees tools in this list.
    fn tools_for_intent(&self, user_intent: &intent::Intent) -> Vec<crate::tools::ToolDef> {
        use intent::Intent::*;
        // Name sets per intent (from lowest to highest capability).
        let allowed: &[&str] = match user_intent {
            Conversational => &[],
            Informational => &[
                "read_file",
                "glob",
                "grep",
                "git",
                "list_symbols",
                "web_fetch",
            ],
            CodeWrite => &[
                "read_file",
                "glob",
                "grep",
                "git",
                "list_symbols",
                "write_file",
                "patch_file",
            ],
            Refactor => &[
                "read_file",
                "glob",
                "grep",
                "git",
                "list_symbols",
                "write_file",
                "patch_file",
            ],
            ShellExec => &[
                "read_file",
                "glob",
                "grep",
                "git",
                "list_symbols",
                "shell_exec",
            ],
            GitOp => &[
                "read_file",
                "glob",
                "grep",
                "git",
                "list_symbols",
                "shell_exec",
            ],
            Deploy => &[
                "read_file",
                "glob",
                "grep",
                "git",
                "list_symbols",
                "shell_exec",
                "web_fetch",
            ],
        };
        self.tools
            .iter()
            .filter(|t| allowed.contains(&t.name.as_str()))
            .cloned()
            .collect()
    }

    /// Build a unified diff preview string between `old` and `new` content.
    /// Returns at most `max_lines` of diff output to keep approval prompts readable.
    fn build_diff_preview(old: &str, new_content: &str, max_lines: usize) -> String {
        let diff = TextDiff::from_lines(old, new_content);
        let mut lines: Vec<String> = Vec::new();

        for change in diff.iter_all_changes() {
            let prefix = match change.tag() {
                ChangeTag::Delete => "- ",
                ChangeTag::Insert => "+ ",
                ChangeTag::Equal => "  ",
            };
            lines.push(format!("{}{}", prefix, change));
            if lines.len() >= max_lines {
                lines.push(format!("  ... ({} lines omitted)", diff.ops().len()));
                break;
            }
        }

        if lines.is_empty() {
            "(no changes)".to_string()
        } else {
            lines.join("")
        }
    }

    /// Create a git stash checkpoint before the first file modification in this run.
    /// Silently skips if: checkpoint already taken, no git repo, clean working tree.
    /// Emits a system message via `tx` so the user knows a checkpoint was created.
    async fn maybe_checkpoint(&self, run_id: u64, tx: &mpsc::Sender<AgentEvent>) {
        let already_taken = {
            let mut flag = self.checkpoint_taken.lock().unwrap();
            if *flag {
                return; // already done for this run
            }
            *flag = true;
            false
        };
        let _ = already_taken; // used above

        let root =
            crate::agent::steer::find_project_root(&self.cwd).unwrap_or_else(|| self.cwd.clone());

        match checkpoint::create(run_id, &root) {
            Ok(stash_ref) => {
                let _ = tx
                    .send(AgentEvent::Text(format!(
                        "[checkpoint created: {stash_ref} — use /undo to rollback]\n"
                    )))
                    .await;
            }
            Err(e) => {
                // Non-fatal: clean tree, no git, etc. Don't block the write.
                let msg = e.to_string();
                // Only surface non-trivial errors (skip "clean tree" noise).
                if !msg.contains("clean") {
                    let _ = tx
                        .send(AgentEvent::Text(format!("[checkpoint skipped: {msg}]\n")))
                        .await;
                }
            }
        }
    }

    async fn execute_tool(
        &self,
        call: &ToolCall,
        run_id: u64,
        tx: &mpsc::Sender<AgentEvent>,
    ) -> Result<crate::tools::ToolResult> {
        let name = call.name.clone();
        // Use cached tool_impls — no re-construction on every call.
        let tool = self
            .tool_impls
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

        if name == "patch_file" {
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
                        "SAFEGUARD: refusing to patch {} — outside the project root {}.",
                        target.display(),
                        root.display()
                    ),
                });
            }

            if let Some(approver) = &self.approver {
                let patch_str = args
                    .get("patch")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("");
                let preview: String = patch_str
                    .lines()
                    .take(40)
                    .map(|l| format!("{l}\n"))
                    .collect();
                let extra_lines = patch_str.lines().count().saturating_sub(40);
                let omit = if extra_lines > 0 {
                    format!("  ... ({extra_lines} more lines)\n")
                } else {
                    String::new()
                };
                let mut desc = format!("patch: {}\n{}{omit}", target.display(), preview);
                if outside {
                    desc = format!("OUTSIDE PROJECT: {desc}");
                }
                if let Approval::Deny = approver.approve(desc).await {
                    return Ok(crate::tools::ToolResult {
                        content: format!("Patch rejected by user approval: {}", target.display()),
                    });
                }
            }

            // Checkpoint before first write in this run.
            self.maybe_checkpoint(run_id, tx).await;
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
                let new_content = args
                    .get("content")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("");
                let op = if target.exists() { "update" } else { "create" };

                let diff_section = if target.exists() {
                    match std::fs::read_to_string(&target) {
                        Ok(old) => {
                            let preview = Self::build_diff_preview(&old, new_content, 40);
                            format!("\n{}\n", preview)
                        }
                        Err(_) => String::new(),
                    }
                } else {
                    let preview: String = new_content
                        .lines()
                        .take(20)
                        .map(|l| format!("+ {l}\n"))
                        .collect();
                    let total = new_content.lines().count();
                    let omitted = total.saturating_sub(20);
                    if omitted > 0 {
                        format!("\n{}+ ... ({omitted} more lines)\n", preview)
                    } else {
                        format!("\n{}\n", preview)
                    }
                };

                let mut desc = format!(
                    "{op}: {} ({} bytes){}",
                    target.display(),
                    new_content.len(),
                    diff_section
                );
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

            // Checkpoint before first write in this run.
            self.maybe_checkpoint(run_id, tx).await;
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

    #[test]
    fn token_estimate_basic() {
        // 4 chars = 1 token
        assert_eq!(Agent::estimate_tokens("abcd"), 1);
        assert_eq!(Agent::estimate_tokens("abcdefgh"), 2);
        assert_eq!(Agent::estimate_tokens(""), 0);
    }

    #[test]
    fn diff_preview_shows_changes() {
        let old = "line1\nline2\nline3\n";
        let new = "line1\nline2 modified\nline3\n";
        let preview = Agent::build_diff_preview(old, new, 40);
        assert!(preview.contains("+ line2 modified"));
        assert!(preview.contains("- line2"));
    }

    #[test]
    fn diff_preview_no_changes() {
        let content = "same\n";
        let preview = Agent::build_diff_preview(content, content, 40);
        // All equal lines — tidak ada + atau - line (hanya spaces)
        assert!(!preview.contains("+ "));
        assert!(!preview.contains("- "));
    }

    #[test]
    fn readonly_tool_classification() {
        assert!(Agent::is_readonly_tool("read_file"));
        assert!(Agent::is_readonly_tool("glob"));
        assert!(Agent::is_readonly_tool("grep"));
        assert!(Agent::is_readonly_tool("git"));
        assert!(Agent::is_readonly_tool("web_fetch"));
        assert!(Agent::is_readonly_tool("list_symbols"));
        assert!(!Agent::is_readonly_tool("shell_exec"));
        assert!(!Agent::is_readonly_tool("write_file"));
        assert!(!Agent::is_readonly_tool("patch_file"));
    }

    #[test]
    fn complexity_simple_question() {
        // Short, no action verbs → below threshold
        assert!(Agent::complexity_score("what does this do?") < Agent::PLAN_THRESHOLD);
        assert!(Agent::complexity_score("explain session.rs") < Agent::PLAN_THRESHOLD);
    }

    #[test]
    fn complexity_complex_task() {
        // Contains "implement" + multi-step connectors → above threshold
        assert!(
            Agent::complexity_score(
                "implement pagination for the users API and then add tests for all endpoints"
            ) >= Agent::PLAN_THRESHOLD
        );
    }

    #[test]
    fn complexity_refactor() {
        assert!(
            Agent::complexity_score("refactor the entire auth module to use the new token system")
                >= Agent::PLAN_THRESHOLD
        );
    }

    #[test]
    fn complexity_multi_file() {
        // Multiple file extensions → elevated score
        assert!(
            Agent::complexity_score(
                "update config.rs, session.rs, and main.rs to support the new provider format"
            ) >= Agent::PLAN_THRESHOLD
        );
    }

    // ── Tool result context persistence tests ─────────────────────────────────

    /// CASE 1: tool_result message is pushed with the correct matching id.
    #[test]
    fn tool_result_has_correct_id() {
        use crate::llm::provider::{Message, ToolCall};
        let call = ToolCall {
            id: "call_abc123".into(),
            name: "shell_exec".into(),
            arguments: r#"{"command":"cargo build"}"#.into(),
        };
        let result_msg = Message::tool_result(call.id.clone(), "Compiled ok");
        assert_eq!(result_msg.tool_call_id.as_deref(), Some("call_abc123"));
        assert_eq!(result_msg.content.as_deref(), Some("Compiled ok"));
    }

    /// CASE 2: assistant_tool_calls_with_text preserves streamed text in history.
    #[test]
    fn assistant_tool_calls_preserves_text() {
        use crate::llm::provider::{Message, ToolCall};
        let calls = vec![ToolCall {
            id: "call_1".into(),
            name: "shell_exec".into(),
            arguments: "{}".into(),
        }];
        let msg = Message::assistant_tool_calls_with_text(calls, "Let me run this.");
        assert_eq!(msg.content.as_deref(), Some("Let me run this."));
        assert_eq!(msg.tool_calls.len(), 1);
    }

    /// CASE 3: empty text produces None content, not empty string.
    #[test]
    fn assistant_tool_calls_empty_text_is_none() {
        use crate::llm::provider::{Message, ToolCall};
        let calls = vec![ToolCall {
            id: "call_2".into(),
            name: "read_file".into(),
            arguments: "{}".into(),
        }];
        let msg = Message::assistant_tool_calls_with_text(calls, "");
        assert!(msg.content.is_none());
    }

    /// CASE 4: fallback id generation is non-empty and stable.
    #[test]
    fn fallback_id_format() {
        let run_id: u64 = 1234567890;
        let id = format!("call_{run_id}_0");
        assert_eq!(id, "call_1234567890_0");
        assert!(!id.is_empty());
    }

    /// CASE 5: message history ordering — user → assistant_tool_calls → tool_result.
    #[test]
    fn message_history_ordering() {
        use crate::llm::provider::{Message, Role, ToolCall};
        let user_msg = Message::user("build the project");
        let tool_call = ToolCall {
            id: "c1".into(),
            name: "shell_exec".into(),
            arguments: "{}".into(),
        };
        let asst_msg = Message::assistant_tool_calls_with_text(vec![tool_call], "");
        let res_msg = Message::tool_result("c1", "Build succeeded");
        let history = vec![user_msg, asst_msg, res_msg];
        assert_eq!(history[0].role, Role::User);
        assert_eq!(history[1].role, Role::Assistant);
        assert!(!history[1].tool_calls.is_empty());
        assert_eq!(history[2].role, Role::Tool);
        assert_eq!(
            history[1].tool_calls[0].id,
            history[2].tool_call_id.as_deref().unwrap()
        );
    }

    /// CASE 6: error output goes into tool_result content (not discarded).
    #[test]
    fn tool_error_content_is_preserved() {
        use crate::llm::provider::Message;
        let err = "error[E0308]: mismatched types\n --> src/main.rs:5:10";
        let msg = Message::tool_result("call_err", err);
        assert!(msg.content.as_deref().unwrap().contains("error[E0308]"));
    }
}
