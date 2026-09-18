use crate::session::Session;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

pub const HELP_TEXT: &str = "\
  vibectl — keyboard reference
  ════════════════════════════════════════════════

  Sending
  ────────────────────────────────────────────────
  Enter          Send message
  Shift+Enter    Insert newline
  Ctrl+C         Cancel running task / clear input
  Ctrl+D         Quit

  Navigation
  ────────────────────────────────────────────────
  ↑ / ↓          Input history
  PgUp / PgDn    Scroll conversation
  Scroll wheel   Scroll conversation
  Ctrl+L         Jump to latest message
  Esc            Close help / cancel

  Slash commands
  ────────────────────────────────────────────────
  /model <name>  Switch model  e.g. /model gpt-4o
  /plan <task>   Generate a step-by-step plan
  /steer <text>  Append a rule to .vibectl/steer.md
  /new           Reset conversation history
  /spec          Show current plan
  /cfg           Print effective config
  /provider      Show provider + model info
  /clear         Clear the message view
  /help or ?     Toggle this help

  Shell commands and file writes pause for approval.
  Writes outside the project root are refused.
";

/// Wall-clock minutes since midnight, as HH:MM.
pub fn now_hhmm() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mins = (secs % 86400) / 60;
    format!("{:02}:{:02}", mins / 60, mins % 60)
}

#[derive(Debug, Clone, PartialEq)]
pub enum MsgRole {
    User,
    Assistant,
    Tool,
    Error,
    System,
    Plan,
}

#[derive(Debug, Clone)]
pub struct MessageItem {
    pub role: MsgRole,
    pub text: String,
    /// Tool name or plan title
    pub kind: Option<String>,
    /// Whether the tool call succeeded
    pub ok: bool,
    /// HH:MM timestamp
    pub ts: String,
}

impl MessageItem {
    fn new(role: MsgRole, text: String) -> Self {
        Self {
            role,
            text,
            kind: None,
            ok: true,
            ts: now_hhmm(),
        }
    }

    pub fn user(text: String) -> Self {
        Self::new(MsgRole::User, text)
    }
    pub fn assistant(text: String) -> Self {
        Self::new(MsgRole::Assistant, text)
    }
    pub fn system(text: String) -> Self {
        Self::new(MsgRole::System, text)
    }
    pub fn error(text: String) -> Self {
        Self {
            ok: false,
            ..Self::new(MsgRole::Error, text)
        }
    }
    pub fn tool(kind: String, text: String, ok: bool) -> Self {
        Self {
            kind: Some(kind),
            ok,
            ..Self::new(MsgRole::Tool, text)
        }
    }
    pub fn plan(text: String) -> Self {
        Self {
            kind: Some("Plan".into()),
            ..Self::new(MsgRole::Plan, text)
        }
    }
}

pub struct App {
    pub session: Session,
    pub messages: Vec<MessageItem>,
    pub input: String,
    pub cursor: usize,
    pub history: Vec<String>,
    pub history_pos: Option<usize>,
    pub busy: bool,
    pub show_help: bool,
    pub scroll_offset: usize,
    pub frame: u64,
    pub run_handle: Option<JoinHandle<()>>,
    pub running_tool: Option<String>,
    pub active_assistant: Option<usize>,
    pub last_status: String,
    pub pending_approval: Option<String>,
    pub pending_approval_tx: Option<oneshot::Sender<bool>>,
    /// How many consecutive Ctrl+C presses while idle (resets on any other key)
    pub ctrl_c_count: u8,
    /// frame at which the last idle Ctrl+C was pressed (for timeout)
    pub ctrl_c_frame: u64,
    /// Monotonically increasing run ID — incremented each begin_run().
    /// Events arriving with a stale ID are silently dropped after interrupt.
    pub run_id: u64,
}

impl App {
    pub fn new(session: Session) -> Self {
        let welcome = format!(
            "vibectl  {}  {}",
            session.provider_label,
            session.agent.model
        );
        Self {
            session,
            messages: vec![MessageItem::system(welcome)],
            input: String::new(),
            cursor: 0,
            history: vec![],
            history_pos: None,
            busy: false,
            show_help: false,
            scroll_offset: 0,
            frame: 0,
            run_handle: None,
            running_tool: None,
            active_assistant: None,
            last_status: String::new(),
            pending_approval: None,
            pending_approval_tx: None,
            ctrl_c_count: 0,
            ctrl_c_frame: 0,
            run_id: 0,
        }
    }

    #[allow(dead_code)]
    pub fn status_line(&self) -> String {
        format!(
            " {} │ {} │ {}",
            self.session.provider_label,
            self.session.agent.model,
            self.session.cwd.display()
        )
    }

    pub fn insert_char(&mut self, c: char) {
        // handle multi-byte correctly
        let byte_pos = char_to_byte(&self.input, self.cursor);
        self.input.insert(byte_pos, c);
        self.cursor += 1;
    }

    pub fn backspace(&mut self) {
        if self.cursor > 0 && !self.input.is_empty() {
            let byte_pos = char_to_byte(&self.input, self.cursor - 1);
            self.input.remove(byte_pos);
            self.cursor -= 1;
        }
    }

    pub fn delete_at_cursor(&mut self) {
        if self.cursor < char_count(&self.input) {
            let byte_pos = char_to_byte(&self.input, self.cursor);
            self.input.remove(byte_pos);
        }
    }

    pub fn move_left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    pub fn move_right(&mut self) {
        if self.cursor < char_count(&self.input) {
            self.cursor += 1;
        }
    }

    pub fn move_home(&mut self) {
        self.cursor = 0;
    }

    pub fn move_end(&mut self) {
        self.cursor = char_count(&self.input);
    }

    pub fn history_prev(&mut self) {
        if self.history.is_empty() {
            return;
        }
        let pos = match self.history_pos {
            Some(p) if p > 0 => p - 1,
            _ => self.history.len().saturating_sub(1),
        };
        self.history_pos = Some(pos);
        self.input = self.history[pos].clone();
        self.cursor = char_count(&self.input);
    }

    pub fn history_next(&mut self) {
        match self.history_pos {
            Some(p) if p + 1 < self.history.len() => {
                let np = p + 1;
                self.history_pos = Some(np);
                self.input = self.history[np].clone();
            }
            Some(_) => {
                self.history_pos = None;
                self.input.clear();
            }
            None => {}
        }
        self.cursor = char_count(&self.input);
    }

    pub fn submit(&mut self) -> String {
        let text = std::mem::take(&mut self.input);
        self.cursor = 0;
        self.history_pos = None;
        if !text.trim().is_empty() {
            self.history.push(text.clone());
            if self.history.len() > 200 {
                self.history.remove(0);
            }
        }
        self.scroll_offset = 0;
        text
    }

    pub fn push_user(&mut self, text: String) {
        self.messages.push(MessageItem::user(text));
    }

    #[allow(dead_code)]
    pub fn push_assistant(&mut self, text: String) {
        self.messages.push(MessageItem::assistant(text));
    }

    pub fn push_system(&mut self, text: String) {
        self.messages.push(MessageItem::system(text));
    }

    pub fn push_error(&mut self, text: String) {
        self.messages.push(MessageItem::error(text));
    }

    pub fn push_tool(&mut self, kind: String, text: String, ok: bool) {
        self.messages.push(MessageItem::tool(kind, text, ok));
    }

    pub fn push_plan(&mut self, text: String) {
        self.messages.push(MessageItem::plan(text));
    }

    pub fn begin_run(&mut self) {
        self.run_id = self.run_id.wrapping_add(1);
        self.busy = true;
        self.running_tool = None;
        self.active_assistant = None;
        self.pending_approval = None;
    }

    pub fn stream_text(&mut self, delta: &str) {
        if let Some(i) = self.active_assistant {
            if i < self.messages.len() {
                self.messages[i].text.push_str(delta);
                return;
            }
        }
        self.messages
            .push(MessageItem::assistant(delta.to_string()));
        self.active_assistant = Some(self.messages.len() - 1);
    }

    pub fn tool_start(&mut self, name: String) {
        self.running_tool = Some(name);
    }

    pub fn tool_end(&mut self, name: String, ok: bool, content: String) {
        self.running_tool = None;
        self.push_tool(name, content, ok);
    }

    pub fn finish_run(&mut self, status: Option<String>) {
        self.busy = false;
        self.running_tool = None;
        self.active_assistant = None;
        self.pending_approval = None;
        self.pending_approval_tx = None;
        self.run_handle = None;
        if let Some(s) = status {
            self.last_status = s;
        }
    }

    pub fn interrupt(&mut self) {
        if let Some(h) = self.run_handle.take() {
            h.abort();
        }
        // bump run_id — any in-flight events from the aborted task will be dropped
        self.run_id = self.run_id.wrapping_add(1);
        self.finish_run(None);
        self.push_system("⚡ interrupted".to_string());
    }

    pub fn scroll_up(&mut self, lines: usize) {
        self.scroll_offset = self.scroll_offset.saturating_add(lines);
    }

    pub fn scroll_down(&mut self, lines: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(lines);
    }

    pub fn follow_bottom(&mut self) {
        self.scroll_offset = 0;
    }
}

// ─── Char/byte index helpers ──────────────────────────────────────────────────

fn char_count(s: &str) -> usize {
    s.chars().count()
}

fn char_to_byte(s: &str, char_idx: usize) -> usize {
    s.char_indices()
        .nth(char_idx)
        .map(|(b, _)| b)
        .unwrap_or(s.len())
}
