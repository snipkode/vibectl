use crate::session::Session;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

pub const HELP_TEXT: &str = "\
Controls
════════
Enter            Send / follow stream to bottom
Shift+Enter      Newline
↑/↓              History navigation
PgUp/PgDn, wheel Scroll through conversation
Ctrl+C           Cancel running task / clear input
Ctrl+D           Exit
Ctrl+L           Jump to latest message
Esc              Close help / cancel
? or /help       Toggle this help

Confirmations
═══════════════
File writes and shell commands pause for your approval:
[y]es / [n]o (Esc = cancel). Writes outside the project
root are refused.

Slash commands
══════════════
/model <name>    Switch model (e.g. /model gpt-4o-mini)
/plan <task>     Generate a step-by-step plan first
/steer <text>    Append a rule to .vibectl/steer.md
/new             Reset conversation history
/spec            Show the current spec plan
/cfg             Print effective config
/provider        Show current provider + model
/clear           Clear the message view

Everything else is sent to the agent as a prompt.
";

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
    pub kind: Option<String>,
}

impl MessageItem {
    pub fn user(text: String) -> Self {
        Self {
            role: MsgRole::User,
            text,
            kind: None,
        }
    }
    pub fn assistant(text: String) -> Self {
        Self {
            role: MsgRole::Assistant,
            text,
            kind: None,
        }
    }
    pub fn system(text: String) -> Self {
        Self {
            role: MsgRole::System,
            text,
            kind: None,
        }
    }
    pub fn error(text: String) -> Self {
        Self {
            role: MsgRole::Error,
            text,
            kind: None,
        }
    }
    pub fn tool(kind: String, text: String) -> Self {
        Self {
            role: MsgRole::Tool,
            text,
            kind: Some(kind),
        }
    }
    pub fn plan(text: String) -> Self {
        Self {
            role: MsgRole::Plan,
            text,
            kind: None,
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
}

impl App {
    pub fn new(session: Session) -> Self {
        Self {
            session,
            messages: vec![MessageItem::system(
                "vibectl — vibe coding agent. Type /help for commands.".to_string(),
            )],
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
        }
    }

    pub fn model_line(&self) -> String {
        format!(
            "vibectl | {} | {} model | cwd: {}",
            self.session.provider_label,
            self.session.agent.model,
            self.session.cwd.display()
        )
    }

    pub fn insert_char(&mut self, c: char) {
        self.input.insert(self.cursor, c);
        self.cursor += 1;
    }

    pub fn backspace(&mut self) {
        if self.cursor > 0 && !self.input.is_empty() {
            self.input.remove(self.cursor - 1);
            self.cursor -= 1;
        }
    }

    pub fn delete_at_cursor(&mut self) {
        if self.cursor < self.input.len() && !self.input.is_empty() {
            self.input.remove(self.cursor);
        }
    }

    pub fn move_left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    pub fn move_right(&mut self) {
        if self.cursor < self.input.len() {
            self.cursor += 1;
        }
    }

    pub fn move_home(&mut self) {
        self.cursor = 0;
    }

    pub fn move_end(&mut self) {
        self.cursor = self.input.len();
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
        self.cursor = self.input.len();
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
        self.cursor = self.input.len();
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

    pub fn push_tool(&mut self, kind: String, text: String) {
        self.messages.push(MessageItem::tool(kind, text));
    }

    pub fn push_plan(&mut self, text: String) {
        self.messages.push(MessageItem::plan(text));
    }

    pub fn begin_run(&mut self) {
        self.busy = true;
        self.running_tool = None;
        self.active_assistant = None;
        self.pending_approval = None;
    }

    pub fn stream_text(&mut self, delta: &str) {
        if let Some(i) = self.active_assistant
            && i < self.messages.len()
        {
            self.messages[i].text.push_str(delta);
            return;
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
        let status = if ok { "ok" } else { "failed" };
        self.push_tool(format!("{name} ({status})"), content);
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
        self.finish_run(None);
        self.push_system("interrupted".to_string());
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
