use crate::session::Session;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

pub const HELP_TEXT: &str = "\
  vibectl — keyboard reference
  ════════════════════════════════════════════════

  Sending
  ────────────────────────────────────────────────
  Send message while a task is running = queued, sent when it finishes
  Enter          Send message
  Shift+Enter    Insert newline
  Ctrl+Enter     Send multiline message
  Ctrl+X         Collapse multiline to single line
  Ctrl+C         Cancel running task / clear input
  Ctrl+D         Quit

  Navigation
  ────────────────────────────────────────────────
  ↑ / ↓          Input history
  PgUp / PgDn    Scroll conversation
  Scroll wheel   Scroll conversation (mouse capture on)
  Alt+C or /copy Toggle copy mode (select/copy text with mouse)
  Ctrl+L         Jump to latest message
  Esc            Close help / cancel

  Slash commands
  ────────────────────────────────────────────────
  /model <name>  Switch model  e.g. /model gpt-4o
  /plan <task>   Generate a step-by-step plan
  /undo          Rollback last agent file changes (git stash pop)
  /steer <text>  Append a rule to .vibectl/steer.md
  /new           Reset conversation history
  /spec          Show current plan
  /cfg           Print effective config
  /provider      Show provider + model info
  /clear         Clear the message view
  /help or ?     Toggle this help

  Shell commands and file writes pause for approval.
  Writes outside the project root are refused.
  A git stash checkpoint is created before the first write in each run.
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
    /// Grouped summary of files the agent intends to create/update,
    /// shown as a "Implementation" bubble before the confirm prompt.
    pub fn implement(text: String) -> Self {
        Self {
            kind: Some("Implementation".into()),
            ..Self::new(MsgRole::Plan, text)
        }
    }
}

/// Entry in the @ file dropdown.
#[derive(Debug, Clone)]
pub struct AtEntry {
    /// Display label (relative path)
    pub label: String,
    /// Absolute path
    pub path: std::path::PathBuf,
    /// true = directory
    pub is_dir: bool,
}

/// All slash commands with their description and usage hint.
pub const COMMANDS: &[(&str, &str, &str)] = &[
    ("/help", "Toggle help panel", "/help"),
    ("/clear", "Clear message view", "/clear"),
    ("/new", "Reset conversation history", "/new"),
    ("/model", "Switch model", "/model <name>"),
    ("/plan", "Generate implementation plan", "/plan <task>"),
    ("/spec", "Show current plan", "/spec"),
    ("/undo", "Rollback last agent changes", "/undo"),
    ("/steer", "Append rule to steer.md", "/steer <rule>"),
    ("/cfg", "Print effective config", "/cfg"),
    ("/provider", "Show provider + model info", "/provider"),
    ("/copy", "Toggle copy mode (mouse selection)", "/copy"),
    ("/quit", "Exit vibectl", "/quit"),
];

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
    pub ctrl_c_count: u8,
    pub ctrl_c_frame: u64,
    pub run_id: u64,
    /// Filtered command suggestions (indices into COMMANDS)
    pub suggestions: Vec<usize>,
    /// Currently highlighted suggestion index (into suggestions vec)
    pub suggestion_sel: usize,
    /// Whether suggestion dropdown is visible
    pub suggestion_visible: bool,
    /// Files/dirs matching current @ query
    pub at_files: Vec<AtEntry>,
    /// Selected index in at_files
    pub at_sel: usize,
    /// Whether @ file dropdown is visible
    pub at_visible: bool,
    /// The @ query being typed (chars after @ up to cursor)
    pub at_query: String,
    /// Tagged file paths that will be injected into the next prompt
    pub at_tagged: Vec<std::path::PathBuf>,
    /// Messages typed while the agent is busy — delivered one at a time
    /// after the current run finishes.
    pub queued_input: Vec<String>,
    /// When true, mouse capture is off so terminal text can be selected/copied.
    pub copy_mode: bool,
    /// Set to true to trigger graceful exit after terminal cleanup.
    pub should_quit: bool,
}

impl App {
    pub fn new(session: Session) -> Self {
        let welcome = format!(
            "vibectl  {}  {}",
            session.provider_label, session.agent.model
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
            suggestions: vec![],
            suggestion_sel: 0,
            suggestion_visible: false,
            at_files: vec![],
            at_sel: 0,
            at_visible: false,
            at_query: String::new(),
            at_tagged: vec![],
            queued_input: vec![],
            copy_mode: false,
            should_quit: false,
        }
    }

    /// Update suggestion list based on current input. Call after every keystroke.
    pub fn update_suggestions(&mut self) {
        let input = self.input.trim_start();
        if !input.starts_with('/') || self.busy {
            self.suggestion_visible = false;
            self.suggestions.clear();
            return;
        }
        // Filter commands that start with the typed prefix
        let prefix = input.split_whitespace().next().unwrap_or(input);
        self.suggestions = COMMANDS
            .iter()
            .enumerate()
            .filter(|(_, (cmd, _, _))| cmd.starts_with(prefix))
            .map(|(i, _)| i)
            .collect();
        self.suggestion_visible = !self.suggestions.is_empty();
        // Clamp selection
        if self.suggestion_sel >= self.suggestions.len() {
            self.suggestion_sel = 0;
        }
    }

    pub fn suggestion_prev(&mut self) {
        if self.suggestions.is_empty() {
            return;
        }
        if self.suggestion_sel == 0 {
            self.suggestion_sel = self.suggestions.len() - 1;
        } else {
            self.suggestion_sel -= 1;
        }
    }

    pub fn suggestion_next(&mut self) {
        if self.suggestions.is_empty() {
            return;
        }
        self.suggestion_sel = (self.suggestion_sel + 1) % self.suggestions.len();
    }

    /// Complete input with the currently selected suggestion.
    /// Returns true if completion happened.
    pub fn complete_suggestion(&mut self) -> bool {
        if !self.suggestion_visible || self.suggestions.is_empty() {
            return false;
        }
        let idx = self.suggestions[self.suggestion_sel];
        let (cmd, _, _usage) = COMMANDS[idx];
        // Always land on "<cmd> " and let the user keep typing the arguments.
        self.input = format!("{cmd} ");
        self.cursor = self.input.chars().count();
        self.suggestion_visible = false;
        self.suggestions.clear();
        true
    }

    /// Called when '@' is typed — scan files and show dropdown.
    pub fn trigger_at(&mut self) {
        self.at_query.clear();
        self.at_files = scan_at_files(&self.session.cwd, "");
        self.at_sel = 0;
        self.at_visible = !self.at_files.is_empty();
    }

    /// Update @ dropdown as user types after '@'.
    pub fn update_at(&mut self, query: &str) {
        self.at_query = query.to_string();
        self.at_files = scan_at_files(&self.session.cwd, query);
        self.at_sel = 0;
        self.at_visible = !self.at_files.is_empty();
    }

    pub fn at_prev(&mut self) {
        if self.at_files.is_empty() {
            return;
        }
        if self.at_sel == 0 {
            self.at_sel = self.at_files.len() - 1;
        } else {
            self.at_sel -= 1;
        }
    }

    pub fn at_next(&mut self) {
        if self.at_files.is_empty() {
            return;
        }
        self.at_sel = (self.at_sel + 1) % self.at_files.len();
    }

    /// Complete the @ mention with selected file path.
    pub fn complete_at(&mut self) {
        if !self.at_visible || self.at_files.is_empty() {
            return;
        }
        let entry = self.at_files[self.at_sel].clone();
        // Find the @ position in input and replace query with label
        let at_pos = self.find_at_pos();
        if let Some(pos) = at_pos {
            let before: String = self.input.chars().take(pos).collect();
            let after: String = self
                .input
                .chars()
                .skip(pos + 1 + self.at_query.chars().count())
                .collect();
            let trail = if entry.is_dir { "/" } else { " " };
            self.input = format!("{before}@{}{trail}{after}", entry.label);
            self.cursor = before.chars().count() + 1 + entry.label.chars().count() + 1;

            // Tag file or all files inside directory for context injection.
            if entry.is_dir {
                // Enumerate source files in the directory (non-recursive for top-level,
                // recursive for src/ style dirs — capped at 20 files to stay concise)
                let tagged = collect_dir_files(&entry.path, 20);
                for p in tagged {
                    if !self.at_tagged.iter().any(|x| x == &p) {
                        self.at_tagged.push(p);
                    }
                }
            } else if !self.at_tagged.iter().any(|p| p == &entry.path) {
                self.at_tagged.push(entry.path);
            }
        }
        self.at_visible = false;
        self.at_files.clear();
        self.at_query.clear();
    }

    pub fn hide_at(&mut self) {
        self.at_visible = false;
        self.at_files.clear();
        self.at_query.clear();
    }

    /// Find the position (char index) of the active @ trigger in input.
    fn find_at_pos(&self) -> Option<usize> {
        let chars: Vec<char> = self.input.chars().collect();
        // Search backward from cursor
        let end = self.cursor.min(chars.len());
        for i in (0..end).rev() {
            if chars[i] == '@' {
                return Some(i);
            }
            if chars[i] == ' ' || chars[i] == '\n' {
                break;
            }
        }
        None
    }

    /// Build the prompt with tagged file contents appended.
    pub fn build_prompt_with_context(&self, prompt: &str) -> String {
        if self.at_tagged.is_empty() {
            return prompt.to_string();
        }
        let mut result = prompt.to_string();
        let count = self.at_tagged.len();
        result.push_str(&format!(
            "\n\n---\nAttached file context ({count} file{}):\n",
            if count == 1 { "" } else { "s" }
        ));
        for path in &self.at_tagged {
            let label = path
                .strip_prefix(&self.session.cwd)
                .map(|p| p.display().to_string())
                .unwrap_or_else(|_| path.display().to_string());
            result.push_str(&format!("\n### {}\n", label));
            if path.is_dir() {
                // Should not happen (dirs are expanded in complete_at),
                // but handle gracefully by listing files inside.
                let files = collect_dir_files(path, 10);
                result.push_str(&format!(
                    "(directory — {} source files found)\n",
                    files.len()
                ));
                for f in &files {
                    if let Ok(contents) = std::fs::read_to_string(f) {
                        let flabel = f
                            .strip_prefix(&self.session.cwd)
                            .map(|p| p.display().to_string())
                            .unwrap_or_else(|_| f.display().to_string());
                        result.push_str(&format!("#### {flabel}\n```\n"));
                        result.push_str(&contents);
                        if !contents.ends_with('\n') {
                            result.push('\n');
                        }
                        result.push_str("```\n");
                    }
                }
            } else {
                match std::fs::read_to_string(path) {
                    Ok(contents) => {
                        result.push_str("```\n");
                        result.push_str(&contents);
                        if !contents.ends_with('\n') {
                            result.push('\n');
                        }
                        result.push_str("```\n");
                    }
                    Err(e) => result.push_str(&format!("(error reading file: {e})\n")),
                }
            }
        }
        result
    }

    pub fn hide_suggestions(&mut self) {
        self.suggestion_visible = false;
        self.suggestions.clear();
        self.suggestion_sel = 0;
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
        // Clamp at the oldest entry rather than wrapping — history_next clears
        // past the newest, so wrapping here would make recall asymmetric.
        let pos = match self.history_pos {
            Some(p) => p.saturating_sub(1),
            None => self.history.len().saturating_sub(1),
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

    /// Queue a message asked while the agent is busy.
    /// Shows it as a user bubble immediately; the agent receives it as a
    /// follow-up turn once the current run ends. Returns the queue length.
    pub fn queue_input(&mut self, text: String) -> usize {
        self.push_user(text.clone());
        self.queued_input.push(text);
        self.queued_input.len()
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

    pub fn push_implement(&mut self, text: String) {
        self.messages.push(MessageItem::implement(text));
    }

    pub fn begin_run(&mut self) {
        self.run_id = self.run_id.wrapping_add(1);
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

// ─── @ file mention helpers ──────────────────────────────────────────────────

/// Scan cwd for files/dirs matching a query prefix.
/// Returns up to 20 results, dirs first, sorted.
pub fn scan_at_files(cwd: &std::path::Path, query: &str) -> Vec<AtEntry> {
    use std::fs;

    let query_lower = query.to_lowercase();
    // Determine base dir and filename prefix
    let (base_dir, name_prefix) =
        if query.contains('/') || query.contains(std::path::MAIN_SEPARATOR) {
            let p = std::path::Path::new(query);
            let parent = p.parent().unwrap_or(std::path::Path::new("."));
            let name = p
                .file_name()
                .map(|n| n.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            (cwd.join(parent), name.to_string())
        } else {
            (cwd.to_path_buf(), query_lower.clone())
        };

    let Ok(entries) = fs::read_dir(&base_dir) else {
        return vec![];
    };

    let mut results: Vec<AtEntry> = entries
        .flatten()
        .filter_map(|e| {
            let path = e.path();
            let name = path.file_name()?.to_string_lossy().to_lowercase();
            // skip hidden and target/
            if name.starts_with('.') {
                return None;
            }
            if name == "target" {
                return None;
            }
            if !name.starts_with(&name_prefix) {
                return None;
            }
            let is_dir = path.is_dir();
            // relative label from cwd
            let label = path
                .strip_prefix(cwd)
                .map(|p| p.display().to_string())
                .unwrap_or_else(|_| path.display().to_string());
            Some(AtEntry {
                label,
                path,
                is_dir,
            })
        })
        .collect();

    // Dirs first, then files, both sorted
    results.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then(a.label.cmp(&b.label)));
    results.truncate(20);
    results
}

/// Collect source files inside a directory (recursive), capped at `max`.
/// Skips target/, node_modules/, .git/, hidden files, and binary files.
pub fn collect_dir_files(dir: &std::path::Path, max: usize) -> Vec<std::path::PathBuf> {
    let source_exts = [
        "rs", "py", "js", "ts", "tsx", "jsx", "go", "java", "kt", "rb", "toml", "yaml", "yml",
        "json", "md", "sh", "sql", "html", "css", "txt", "env",
    ];
    let skip_dirs = [
        "target",
        "node_modules",
        ".git",
        "dist",
        "build",
        "out",
        "__pycache__",
    ];

    let mut out = Vec::new();
    collect_dir_recursive(dir, &source_exts, &skip_dirs, &mut out, max);
    out
}

fn collect_dir_recursive(
    dir: &std::path::Path,
    source_exts: &[&str],
    skip_dirs: &[&str],
    out: &mut Vec<std::path::PathBuf>,
    max: usize,
) {
    if out.len() >= max {
        return;
    }
    let mut entries: Vec<_> = match std::fs::read_dir(dir) {
        Ok(rd) => rd.flatten().map(|e| e.path()).collect(),
        Err(_) => return,
    };
    entries.sort();
    for p in entries {
        if out.len() >= max {
            return;
        }
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name.starts_with('.') {
            continue;
        }
        if p.is_dir() {
            if skip_dirs.contains(&name) {
                continue;
            }
            collect_dir_recursive(&p, source_exts, skip_dirs, out, max);
        } else if p.is_file() {
            let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("");
            if source_exts.contains(&ext) {
                out.push(p);
            }
        }
    }
}

/// ─── Char/byte index helpers ──────────────────────────────────────────────────
fn char_count(s: &str) -> usize {
    s.chars().count()
}

fn char_to_byte(s: &str, char_idx: usize) -> usize {
    s.char_indices()
        .nth(char_idx)
        .map(|(b, _)| b)
        .unwrap_or(s.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    /// A throwaway app backed by a temp project dir.  The default model routes
    /// to the Ollama provider, which constructs without any network access.
    fn app() -> (App, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let session =
            Session::new(Config::default(), dir.path().to_path_buf(), None).expect("session");
        (App::new(session), dir)
    }

    // ── Char/byte index helpers ───────────────────────────────────────────────

    #[test]
    fn char_helpers_count_chars_not_bytes() {
        assert_eq!(char_count("abc"), 3);
        assert_eq!(char_count("héllo"), 5);
        assert_eq!(char_count("日本語"), 3);
        assert_eq!(char_count(""), 0);
    }

    #[test]
    fn char_to_byte_maps_char_index_to_byte_offset() {
        let s = "héllo";
        assert_eq!(char_to_byte(s, 0), 0);
        assert_eq!(char_to_byte(s, 1), 1, "é is two bytes");
        assert_eq!(char_to_byte(s, 2), 3);
        assert_eq!(char_to_byte(s, 99), s.len(), "out of range clamps to len");
    }

    // ── Cursor editing ────────────────────────────────────────────────────────

    #[test]
    fn insert_and_backspace_handle_multibyte() {
        let (mut a, _d) = app();
        for c in "日本語".chars() {
            a.insert_char(c);
        }
        assert_eq!(a.input, "日本語");
        assert_eq!(a.cursor, 3, "cursor counts chars, not bytes");

        a.backspace();
        assert_eq!(a.input, "日本");
        assert_eq!(a.cursor, 2);
    }

    #[test]
    fn backspace_at_start_is_a_noop() {
        let (mut a, _d) = app();
        a.backspace();
        assert!(a.input.is_empty());
        assert_eq!(a.cursor, 0);
    }

    #[test]
    fn insert_at_cursor_position_lands_in_the_right_place() {
        let (mut a, _d) = app();
        for c in "ac".chars() {
            a.insert_char(c);
        }
        a.move_left(); // cursor between 'a' and 'c'
        a.insert_char('b');
        assert_eq!(a.input, "abc");
        assert_eq!(a.cursor, 2);
    }

    #[test]
    fn delete_at_cursor_removes_forward() {
        let (mut a, _d) = app();
        for c in "abc".chars() {
            a.insert_char(c);
        }
        a.move_home();
        a.delete_at_cursor();
        assert_eq!(a.input, "bc");
        assert_eq!(a.cursor, 0, "deleting forward must not move the cursor");
    }

    #[test]
    fn cursor_movement_clamps_at_both_ends() {
        let (mut a, _d) = app();
        a.move_left();
        assert_eq!(a.cursor, 0);
        a.move_right();
        assert_eq!(a.cursor, 0, "cannot move past an empty input");

        for c in "hi".chars() {
            a.insert_char(c);
        }
        a.move_right();
        assert_eq!(a.cursor, 2, "cannot move past the end");
        a.move_end();
        assert_eq!(a.cursor, 2);
        a.move_home();
        assert_eq!(a.cursor, 0);
    }

    // ── History ───────────────────────────────────────────────────────────────

    #[test]
    fn history_walks_back_then_forward() {
        let (mut a, _d) = app();
        a.submit();
        a.input = "first".into();
        a.submit();
        a.input = "second".into();
        a.submit();
        assert!(a.input.is_empty(), "submit clears the input");

        a.history_prev();
        assert_eq!(a.input, "second");
        a.history_prev();
        assert_eq!(a.input, "first");
        a.history_prev();
        assert_eq!(a.input, "first", "clamps at the oldest entry");

        a.history_next();
        assert_eq!(a.input, "second");
        a.history_next();
        assert_eq!(a.input, "", "past the newest entry clears the input");
        assert_eq!(a.history_pos, None);
    }

    #[test]
    fn submit_ignores_whitespace_only_input() {
        let (mut a, _d) = app();
        a.input = "   ".into();
        let out = a.submit();
        assert_eq!(out, "   ");
        assert!(a.history.is_empty(), "whitespace is not worth recalling");
    }

    #[test]
    fn history_is_capped() {
        let (mut a, _d) = app();
        for i in 0..250 {
            a.input = format!("msg{i}");
            a.submit();
        }
        assert_eq!(a.history.len(), 200);
        assert_eq!(a.history.last().unwrap(), "msg249");
    }

    // ── @ mention scanning ────────────────────────────────────────────────────

    #[test]
    fn scan_at_files_filters_by_prefix_and_sorts_dirs_first() {
        let (_a, dir) = app();
        std::fs::create_dir(dir.path().join("src")).unwrap();
        std::fs::create_dir(dir.path().join("assets")).unwrap();
        std::fs::write(dir.path().join("src/main.rs"), "").unwrap();
        std::fs::write(dir.path().join("README.md"), "").unwrap();

        let hits = scan_at_files(dir.path(), "s");
        let labels: Vec<&str> = hits.iter().map(|h| h.label.as_str()).collect();
        assert_eq!(labels, vec!["src"], "only prefix matches, dirs first");

        let all = scan_at_files(dir.path(), "");
        assert_eq!(all.len(), 3, "empty query matches everything visible");
    }

    #[test]
    fn scan_at_files_skips_hidden_and_target() {
        let (_a, dir) = app();
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        std::fs::create_dir(dir.path().join("target")).unwrap();
        std::fs::write(dir.path().join("visible.txt"), "").unwrap();

        let labels: Vec<String> = scan_at_files(dir.path(), "")
            .into_iter()
            .map(|h| h.label)
            .collect();
        assert_eq!(labels, vec!["visible.txt"]);
    }

    #[test]
    fn scan_at_files_supports_relative_subpaths() {
        let (_a, dir) = app();
        std::fs::create_dir_all(dir.path().join("src/agent")).unwrap();
        std::fs::write(dir.path().join("src/agent/mod.rs"), "").unwrap();
        std::fs::write(dir.path().join("src/other.rs"), "").unwrap();

        let labels: Vec<String> = scan_at_files(dir.path(), "src/ag")
            .into_iter()
            .map(|h| h.label)
            .collect();
        assert_eq!(labels, vec!["src/agent"], "prefix-matches the directory");
    }

    #[test]
    fn scan_at_files_with_trailing_slash_lists_that_directory() {
        // Picking the directory is what triggers expansion in complete_at, so
        // the dropdown offering the directory itself is the intended behaviour.
        let (_a, dir) = app();
        std::fs::create_dir_all(dir.path().join("src/agent")).unwrap();
        std::fs::write(dir.path().join("src/agent/mod.rs"), "").unwrap();

        let labels: Vec<String> = scan_at_files(dir.path(), "src/agent/")
            .into_iter()
            .map(|h| h.label)
            .collect();
        assert_eq!(labels, vec!["src/agent"]);
    }

    #[test]
    fn scan_at_files_is_empty_for_a_missing_directory() {
        let (_a, dir) = app();
        assert!(scan_at_files(dir.path(), "nope/").is_empty());
    }

    // ── collect_dir_files ─────────────────────────────────────────────────────

    #[test]
    fn collect_dir_files_filters_extensions_and_skips_build_dirs() {
        let (_a, dir) = app();
        std::fs::create_dir_all(dir.path().join("target/debug")).unwrap();
        std::fs::create_dir_all(dir.path().join("node_modules/pkg")).unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("target/debug/big.rs"), "").unwrap();
        std::fs::write(dir.path().join("node_modules/pkg/index.js"), "").unwrap();
        std::fs::write(dir.path().join("src/lib.rs"), "").unwrap();
        std::fs::write(dir.path().join("src/logo.png"), "").unwrap();

        let files: Vec<String> = collect_dir_files(dir.path(), 100)
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(files, vec!["lib.rs"], "binary ext + build dirs excluded");
    }

    #[test]
    fn collect_dir_files_respects_the_cap() {
        let (_a, dir) = app();
        for i in 0..10 {
            std::fs::write(dir.path().join(format!("f{i}.rs")), "").unwrap();
        }
        assert_eq!(collect_dir_files(dir.path(), 4).len(), 4);
    }

    // ── Run lifecycle ─────────────────────────────────────────────────────────

    #[test]
    fn interrupt_marks_idle_and_records_the_event() {
        let (mut a, _d) = app();
        a.busy = true;
        let before = a.messages.len();
        a.interrupt();
        assert!(!a.busy);
        assert_eq!(a.scroll_offset, 0);
        assert!(a.messages.len() > before);
    }

    #[test]
    fn scroll_never_goes_negative() {
        let (mut a, _d) = app();
        a.scroll_down(5);
        assert_eq!(a.scroll_offset, 0, "saturating_sub keeps this at zero");
        a.scroll_up(3);
        assert_eq!(a.scroll_offset, 3);
        a.follow_bottom();
        assert_eq!(a.scroll_offset, 0);
    }

    // ── Misc ──────────────────────────────────────────────────────────────────

    #[test]
    fn now_hhmm_is_a_valid_clock_time() {
        let stamp = now_hhmm();
        assert_eq!(stamp.len(), 5, "expected HH:MM, got {stamp:?}");
        let (h, m) = stamp.split_once(':').expect("HH:MM shape");
        assert_eq!(h.len(), 2, "hour must be zero-padded: {stamp:?}");
        assert_eq!(m.len(), 2, "minute must be zero-padded: {stamp:?}");
        let hour: u32 = h.parse().expect("hour");
        let minute: u32 = m.parse().expect("minute");
        assert!(hour < 24, "hour out of range: {stamp}");
        assert!(minute < 60, "minute out of range: {stamp}");
    }

    #[test]
    fn command_table_has_unique_names_and_matching_usage() {
        let mut names: Vec<&str> = COMMANDS.iter().map(|(n, _, _)| *n).collect();
        let count = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), count, "duplicate slash command in COMMANDS");

        for (name, desc, usage) in COMMANDS {
            assert!(name.starts_with('/'), "{name} should start with /");
            assert!(!desc.is_empty(), "{name} has no description");
            assert!(
                usage.starts_with(name),
                "{name} usage hint {usage:?} should start with the command"
            );
        }
    }

    #[test]
    fn suggestions_filter_by_typed_prefix() {
        let (mut a, _d) = app();
        a.input = "/mo".into();
        a.update_suggestions();
        assert!(a.suggestion_visible);
        let picked: Vec<&str> = a.suggestions.iter().map(|&i| COMMANDS[i].0).collect();
        assert!(picked.contains(&"/model"), "got {picked:?}");
        assert!(!picked.contains(&"/help"));
    }

    #[test]
    fn suggestions_hidden_for_non_command_input() {
        let (mut a, _d) = app();
        a.input = "hello".into();
        a.update_suggestions();
        assert!(!a.suggestion_visible);
        assert!(a.suggestions.is_empty());
    }

    #[test]
    fn complete_suggestion_fills_in_the_command() {
        let (mut a, _d) = app();
        a.input = "/prov".into();
        a.update_suggestions();
        assert!(a.complete_suggestion());
        assert!(a.input.starts_with("/provider"));
        assert!(!a.suggestion_visible, "dropdown closes after completion");
    }
}
