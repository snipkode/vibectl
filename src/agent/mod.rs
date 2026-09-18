pub mod checkpoint;
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
        if word_count > 50 { score += 3; }
        else if word_count > 25 { score += 2; }
        else if word_count > 15 { score += 1; }

        // Action verbs that imply multi-step work.
        let complex_verbs = [
            "implement", "refactor", "migrate", "redesign", "rewrite",
            "add feature", "create", "build", "integrate", "setup",
            "convert", "upgrade", "extract", "split", "merge",
        ];
        for verb in &complex_verbs {
            if lower.contains(verb) { score += 2; break; }
        }

        // Multiple targets — multi-step connectors.
        let connectors = ["and then", " and ", " then ", " also ", " plus ", " + "];
        for conn in &connectors {
            if lower.contains(conn) { score += 1; break; }
        }

        // File count hints — count total file-like tokens (word contains a dot + ext).
        let file_token_count = input
            .split_whitespace()
            .filter(|w| {
                let w = w.trim_matches(|c: char| !c.is_alphanumeric() && c != '.' && c != '_');
                let exts = [".rs", ".py", ".js", ".ts", ".go", ".toml", ".md", ".json", ".yaml"];
                exts.iter().any(|e| w.ends_with(e))
            })
            .count();
        if file_token_count >= 3 { score += 2; }
        else if file_token_count >= 2 { score += 1; }

        // "all files", "every file" hints.
        if lower.contains("all files") || lower.contains("every file") { score += 2; }

        // Explicit scope words.
        let scope_words = ["across", "throughout", "everywhere", "all of", "entire"];
        for w in &scope_words {
            if lower.contains(w) { score += 1; break; }
        }

        score.min(10)
    }

    /// Complexity score threshold above which auto-planning triggers.
    pub const PLAN_THRESHOLD: u8 = 3;

    /// Fast rule-based intent pre-check.
    /// Returns `Some(true)` = definitely conversational,
    ///         `Some(false)` = definitely task,
    ///         `None` = uncertain, needs LLM fallback.
    pub fn intent_rules(input: &str) -> Option<bool> {
        let trimmed = input.trim();
        let lower = trimmed.to_lowercase();
        let word_count = trimmed.split_whitespace().count();

        // ── Definite task signals ─────────────────────────────────────────────
        let has_shell_signal = lower.contains("cargo ")
            || lower.contains("git ")
            || lower.contains("npm ")
            || lower.contains("pip ")
            || lower.contains(" --")
            || lower.starts_with("$ ");
        if has_shell_signal { return Some(false); }

        let has_file_ref = trimmed.split_whitespace().any(|w| {
            let w = w.trim_matches(|c: char| ",;:?!".contains(c));
            let code_exts = [".rs", ".py", ".js", ".ts", ".go", ".toml", ".md",
                             ".json", ".yaml", ".yml", ".html", ".css", ".sh"];
            code_exts.iter().any(|e| w.ends_with(e))
                || (w.contains('/') && !w.starts_with("http"))
        });

        let task_verbs = [
            "implement", "refactor", "migrate", "redesign", "rewrite",
            "create ", "add ", "fix ", "build ", "write ", "update ",
            "delete ", "remove ", "install ", "deploy ", "generate ", "scaffold ",
            "buat ", "tambahkan ", "perbaiki ", "hapus ", "jalankan ",
            "ubah ", "refaktor ", "implementasi ",
        ];
        let has_task_verb = task_verbs.iter().any(|v| lower.contains(v));

        if has_file_ref && has_task_verb { return Some(false); }
        if has_task_verb && word_count > 4 { return Some(false); }

        // ── Definite conversational signals ───────────────────────────────────
        if word_count <= 3 {
            let short_task_signals = [
                "run ", "fix ", "add ", "buat ", "test ", "build ",
                "install ", "deploy ", "delete ", "remove ",
            ];
            let has_short_task = short_task_signals.iter().any(|s| lower.starts_with(s));
            let has_path = trimmed.contains('/') || trimmed.contains('.')
                || trimmed.contains("--");
            if !has_short_task && !has_path {
                return Some(true);
            }
        }

        let question_starters = [
            "what ", "how ", "why ", "when ", "where ", "who ", "which ",
            "can you ", "could you ", "do you ", "did you ", "is it ", "are you ",
            "apa ", "bagaimana ", "kenapa ", "mengapa ", "kapan ", "siapa ",
            "boleh ", "bisa ", "apakah ", "tolong jelaskan", "jelaskan ", "explain ", "describe ", "tell me ",
        ];
        let pref_phrases = [
            "pake ", "pakai ", "gunakan ", "speak ", "talk ",
            "bahasa ", "language ", "in english", "in indonesian",
            "please ", "mohon ", "tolong ",
        ];
        let has_question = question_starters.iter().any(|q| lower.starts_with(q));
        let has_pref = pref_phrases.iter().any(|p| lower.starts_with(p) || lower.contains(p));

        if (has_question || has_pref) && !has_task_verb {
            // If there's a file ref in a question, let LLM decide (could be
            // "what does session.rs do?" vs "fix session.rs please").
            if has_file_ref {
                return None;
            }
            return Some(true);
        }

        // ── Uncertain ─────────────────────────────────────────────────────────
        None
    }

    /// Classify whether user input is conversational or a task.
    /// Uses fast rule-based check first; falls back to a lightweight LLM call
    /// returning `{"intent":"conversational"}` or `{"intent":"task"}`.
    /// On any error, defaults to task mode (safe: tools available but not forced).
    pub async fn classify_intent(&self, input: &str) -> bool {
        // Fast path
        if let Some(result) = Self::intent_rules(input) {
            return result;
        }

        // LLM fallback — single message, no tools, max 20 tokens
        let prompt = format!(
            "Classify this user message.\nRespond ONLY with JSON:              {{\"intent\":\"conversational\"}} or {{\"intent\":\"task\"}}\n\n             conversational = greeting, small talk, preference setting, language request,              general question, acknowledgement.\n             task = coding task, file edit, shell command, implementation request.\n\n             Message: {input}"
        );

        let req = ChatRequest {
            model: self.model.clone(),
            messages: vec![Message::user(prompt)],
            temperature: 0.0,
            max_tokens: Some(20),
            stream: false,
            tools: vec![],
            system: None,
        };

        match self.provider.chat(&req).await {
            Ok(resp) => {
                let text = resp.content.unwrap_or_default();
                // Try strict JSON parse first
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(text.trim()) {
                    if let Some(intent) = val.get("intent").and_then(|v| v.as_str()) {
                        return intent == "conversational";
                    }
                }
                // Fallback: scan raw text
                let lower = text.to_lowercase();
                lower.contains("\"conversational\"")
                    || (lower.contains("conversational") && !lower.contains("task"))
            }
            Err(_) => false, // Default to task on error
        }
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
        if self.auto_plan
            && is_first_turn
            && user_input.split_whitespace().count() >= 4
            && Self::complexity_score(&user_input) >= Self::PLAN_THRESHOLD
        {
            match self.plan(&user_input).await {
                Ok(plan) => {
                    // Save to disk (best-effort).
                    let _ = crate::agent::steer::save_plan(&self.cwd, &plan);
                    // Emit plan event so TUI / headless can display it.
                    let _ = tx.send(AgentEvent::Plan(plan.clone())).await;
                    // Inject the plan as a system-level context message so the
                    // agent executes step-by-step.
                    self.push(Message::system(format!(
                        "Auto-generated implementation plan for this task:\n\n{plan}\n\n\
                         Execute the steps above. Use tools to inspect, then implement."
                    )))
                    .await;
                }
                Err(e) => {
                    // Planning failure is non-fatal — log and continue.
                    let _ = tx
                        .send(AgentEvent::Text(format!(
                            "[auto-plan failed: {e} — proceeding without plan]\n"
                        )))
                        .await;
                }
            }
        }

        self.push(Message::user(user_input.clone())).await;

        // If the input is conversational, strip tools entirely so the LLM
        // cannot physically call shell_exec or write_file.
        let conversational = self.classify_intent(&user_input).await;

        loop {
            let messages = self.snapshot().await;
            let req = ChatRequest {
                model: self.model.clone(),
                messages,
                temperature: self.temperature,
                max_tokens: self.max_tokens,
                stream: true,
                // Protocol-level enforcement: conversational inputs get NO tools.
                tools: if conversational { vec![] } else { self.tools.clone() },
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

            // Safety guard: if the input was classified as conversational but
            // the model still emitted tool calls (e.g. older fine-tuned model),
            // discard them and treat the response as a plain text reply.
            if conversational && !clean_calls.is_empty() {
                self.push(Message::assistant(text)).await;
                let _ = tx.send(AgentEvent::Done { finish_reason }).await;
                break;
            }

            if !clean_calls.is_empty() {
                self.push(Message::assistant_tool_calls(clean_calls.clone()))
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

        let root = crate::agent::steer::find_project_root(&self.cwd)
            .unwrap_or_else(|| self.cwd.clone());

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
                        .send(AgentEvent::Text(format!(
                            "[checkpoint skipped: {msg}]\n"
                        )))
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
            Agent::complexity_score(
                "refactor the entire auth module to use the new token system"
            ) >= Agent::PLAN_THRESHOLD
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

    #[test]
    fn conversational_greetings() {
        assert_eq!(Agent::intent_rules("halo"), Some(true));
        assert_eq!(Agent::intent_rules("hi"), Some(true));
        assert_eq!(Agent::intent_rules("hello"), Some(true));
        assert_eq!(Agent::intent_rules("hai"), Some(true));
        assert_eq!(Agent::intent_rules("hali"), Some(true));
        assert_eq!(Agent::intent_rules("thanks"), Some(true));
        assert_eq!(Agent::intent_rules("terima kasih"), Some(true));
        assert_eq!(Agent::intent_rules("ok"), Some(true));
        assert_eq!(Agent::intent_rules("mantap"), Some(true));
    }

    #[test]
    fn conversational_short_inputs() {
        assert_eq!(Agent::intent_rules("ok sip"), Some(true));
        assert_eq!(Agent::intent_rules("noted"), Some(true));
        assert_eq!(Agent::intent_rules("pake bahasa indonesia"), Some(true));
        assert_eq!(Agent::intent_rules("use english please"), Some(true));
        assert_eq!(Agent::intent_rules("speak indonesian"), Some(true));
        assert_eq!(Agent::intent_rules("bahasa indonesia ya"), Some(true));
    }

    #[test]
    fn conversational_questions_no_action() {
        // Questions without file refs → rule detects as conversational
        assert_eq!(Agent::intent_rules("how does the agent loop work?"), Some(true));
        assert_eq!(Agent::intent_rules("explain the tool dispatch"), Some(true));
        assert_eq!(Agent::intent_rules("what is a steering file?"), Some(true));
        // Question WITH file ref but no task verb → uncertain, falls back to LLM
        assert_eq!(Agent::intent_rules("what does session.rs do?"), None);
    }

    #[test]
    fn not_conversational_tasks() {
        // Clear task signals → Some(false)
        assert_eq!(Agent::intent_rules("implement pagination for the API endpoint"), Some(false));
        assert_eq!(Agent::intent_rules("fix the bug in session.rs"), Some(false));
        assert_eq!(Agent::intent_rules("add unit tests to agent/mod.rs"), Some(false));
        assert_eq!(Agent::intent_rules("refactor the auth module to use new tokens"), Some(false));
        assert_eq!(Agent::intent_rules("run cargo test and fix all failures"), Some(false));
        assert_eq!(Agent::intent_rules("buat fungsi baru di tools/mod.rs"), Some(false));
    }

    #[test]
    fn not_conversational_question_with_action() {
        // Question starter + action verb → Some(false)
        assert_eq!(Agent::intent_rules("how do I implement oauth login in session.rs?"), Some(false));
    }

    #[test]
    fn intent_none_llm_fallback_cases() {
        // These are ambiguous — rules return None, LLM must decide.

        // Question with file ref but no task verb
        assert_eq!(Agent::intent_rules("what does session.rs do?"), None);
        assert_eq!(Agent::intent_rules("how does agent/mod.rs work?"), None);

        // Medium-length input with no clear signal either way
        // (>8 words, no task verb, no file ref, no question starter)
        assert_eq!(Agent::intent_rules("the agent seems to be calling tools unexpectedly"), None);

        // Ambiguous instruction that could be conversational or task
        assert_eq!(Agent::intent_rules("show me the current model configuration"), None);
    }
}
