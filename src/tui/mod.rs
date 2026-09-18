pub mod app;
pub mod backend;
pub mod ui;

use crate::agent::{AgentEvent, Approval, Approver};
use crate::session::Session;
use crate::tui::backend::ResilientBackend;
use anyhow::Result;
use async_trait::async_trait;
use crossterm::event::{
    self, Event as CEvent, KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind,
};
use ratatui::Terminal;
use std::io;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::sync::oneshot;

use app::{App, MessageItem};

const TICK_MS: Duration = Duration::from_millis(80);

pub enum Msg {
    Tick,
    Key(KeyEvent),
    Mouse(MouseEvent),
    App(AppMsg),
}

pub enum AppMsg {
    /// run_id tags each event so stale events after interrupt are dropped.
    Text(u64, String),
    ToolStart(u64, String),
    ToolResult {
        run_id: u64,
        name: String,
        content: String,
        ok: bool,
    },
    Done(u64),
    Error(u64, String),
    Plan(String),
    ApprovalRequested(String, oneshot::Sender<bool>),
}

pub struct TuiApprover {
    tx: mpsc::Sender<Msg>,
}

#[async_trait]
impl Approver for TuiApprover {
    async fn approve(&self, description: String) -> Approval {
        let (otx, orx) = oneshot::channel();
        let _ = self
            .tx
            .send(Msg::App(AppMsg::ApprovalRequested(description, otx)))
            .await;
        match orx.await {
            Ok(true) => Approval::Allow,
            _ => Approval::Deny,
        }
    }
}

pub async fn run(session: Session) -> Result<()> {
    let mut app = App::new(session);

    let (msg_tx, mut msg_rx) = mpsc::channel::<Msg>(256);

    let input_tx = msg_tx.clone();
    std::thread::spawn(move || {
        while let Ok(ev) = event::read() {
            let msg = match ev {
                CEvent::Key(k) => Msg::Key(k),
                CEvent::Mouse(m) => Msg::Mouse(m),
                _ => continue,
            };
            if input_tx.blocking_send(msg).is_err() {
                break;
            }
        }
    });

    let approver = Arc::new(TuiApprover { tx: msg_tx.clone() });
    app.session.agent.approver = Some(approver.clone());

    crossterm::terminal::enable_raw_mode()?;

    let mut terminal = Terminal::new(ResilientBackend::new())?;
    terminal.clear()?;

    crossterm::execute!(
        io::stdout(),
        crossterm::cursor::Hide,
        event::EnableMouseCapture
    )?;

    let res = run_loop(&mut terminal, &msg_tx, &mut msg_rx, &mut app).await;

    let _ = crossterm::terminal::disable_raw_mode();
    crossterm::execute!(
        io::stdout(),
        crossterm::cursor::Show,
        event::DisableMouseCapture
    )?;
    terminal.clear()?;
    res
}

async fn run_loop(
    terminal: &mut Terminal<ResilientBackend>,
    msg_tx: &mpsc::Sender<Msg>,
    msg_rx: &mut mpsc::Receiver<Msg>,
    app: &mut App,
) -> Result<()> {
    loop {
        terminal.draw(|f| ui::render(f, app))?;

        let msg = tokio::time::timeout(TICK_MS, msg_rx.recv())
            .await
            .map(|r| r.unwrap_or(Msg::Tick))
            .unwrap_or(Msg::Tick);

        match msg {
            Msg::Tick => app.frame = app.frame.wrapping_add(1),
            Msg::Key(key) => handle_key(terminal, msg_tx, app, key).await?,
            Msg::Mouse(m) => handle_mouse(app, m),
            Msg::App(m) => handle_app_msg(app, m).await,
        }
    }
}

async fn handle_app_msg(app: &mut App, msg: AppMsg) {
    match msg {
        AppMsg::Text(id, t) => {
            if id == app.run_id { app.stream_text(&t); }
        }
        AppMsg::ToolStart(id, name) => {
            if id == app.run_id { app.tool_start(name); }
        }
        AppMsg::ToolResult { run_id, name, content, ok } => {
            if run_id == app.run_id { app.tool_end(name, ok, content); }
        }
        AppMsg::Done(id) => {
            if id == app.run_id { app.finish_run(None); }
        }
        AppMsg::Error(id, e) => {
            if id == app.run_id {
                app.finish_run(None);
                app.push_error(e);
            }
        }
        AppMsg::Plan(p) => {
            app.finish_run(None);
            app.push_plan(p);
        }
        AppMsg::ApprovalRequested(cmd, otx) => {
            app.pending_approval = Some(cmd);
            app.pending_approval_tx = Some(otx);
        }
    }
}

fn handle_mouse(app: &mut App, m: MouseEvent) {
    match m.kind {
        MouseEventKind::ScrollUp => app.scroll_up(3),
        MouseEventKind::ScrollDown => app.scroll_down(3),
        _ => {}
    }
}

async fn handle_key(
    _terminal: &mut Terminal<ResilientBackend>,
    msg_tx: &mpsc::Sender<Msg>,
    app: &mut App,
    key: KeyEvent,
) -> Result<()> {
    if let Some(otx) = app.pending_approval_tx.take() {
        match key.code {
            KeyCode::Char('y' | 'Y') => {
                app.pending_approval = None;
                let _ = otx.send(true);
            }
            KeyCode::Char('n' | 'N') | KeyCode::Esc => {
                app.pending_approval = None;
                let _ = otx.send(false);
            }
            _ => {
                app.pending_approval_tx = Some(otx);
                return Ok(());
            }
        }
        return Ok(());
    }

    match key.code {
        KeyCode::Esc => {
            if app.show_help {
                app.show_help = false;
            } else if !app.busy {
                app.input.clear();
                app.cursor = 0;
            }
        }
        KeyCode::Enter => {
            if app.busy {
                app.follow_bottom();
                let typed = app.input.clone();
                if typed.trim_start().starts_with('/') {
                    let text = app.submit();
                    handle_command(msg_tx, app, &text).await;
                }
                return Ok(());
            }
            // Ctrl+Enter — force send even in multiline mode
            if key.modifiers.contains(KeyModifiers::CONTROL) {
                let text = app.submit();
                if !text.trim().is_empty() {
                    if text.starts_with('/') {
                        handle_command(msg_tx, app, &text).await;
                    } else {
                        app.push_user(text.clone());
                        app.begin_run();
                        spawn_agent(msg_tx.clone(), app, text);
                    }
                }
                return Ok(());
            }
            if key.modifiers.contains(KeyModifiers::SHIFT) {
                app.insert_char('\n');
                return Ok(());
            }
            let text = app.submit();
            if text.trim().is_empty() {
                return Ok(());
            }
            if text.starts_with('/') {
                handle_command(msg_tx, app, &text).await;
                return Ok(());
            }
            app.push_user(text.clone());
            app.begin_run();
            spawn_agent(msg_tx.clone(), app, text);
        }
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            if app.busy {
                // cancel running agent
                app.interrupt();
                // reset exit counter
                app.ctrl_c_count = 0;
            } else {
                if !app.input.is_empty() {
                    // first Ctrl+C: clear input
                    app.input.clear();
                    app.cursor = 0;
                    app.ctrl_c_count = 0;
                } else {
                    // input already empty — count toward exit
                    // timeout: if last press was > 50 frames ago (~4s), reset
                    let elapsed = app.frame.saturating_sub(app.ctrl_c_frame);
                    if elapsed > 50 {
                        app.ctrl_c_count = 0;
                    }
                    app.ctrl_c_count += 1;
                    app.ctrl_c_frame = app.frame;

                    if app.ctrl_c_count >= 2 {
                        // second Ctrl+C — exit
                        std::process::exit(0);
                    } else {
                        // first press — show hint
                        app.push_system(
                            "Press Ctrl+C again to exit  (or /quit)".to_string(),
                        );
                    }
                }
            }
        }
        KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            std::process::exit(0);
        }
        KeyCode::Char('l') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.follow_bottom();
        }
        // Ctrl+K — toggle help (shown in hint bar)
        KeyCode::Char('k') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.ctrl_c_count = 0;
            app.show_help = !app.show_help;
        }
        KeyCode::Char('?') => {
            app.ctrl_c_count = 0;
            app.show_help = !app.show_help;
        }
        KeyCode::Up => app.history_prev(),
        KeyCode::Down => app.history_next(),
        KeyCode::Left => app.move_left(),
        KeyCode::Right => app.move_right(),
        KeyCode::Home => app.move_home(),
        KeyCode::End => app.move_end(),
        KeyCode::Backspace => {
            app.ctrl_c_count = 0;
            app.backspace();
        }
        KeyCode::Delete => {
            app.ctrl_c_count = 0;
            app.delete_at_cursor();
        }
        KeyCode::PageUp => app.scroll_up(10),
        KeyCode::PageDown => app.scroll_down(10),
        KeyCode::Char(c) => {
            app.ctrl_c_count = 0;
            app.insert_char(c);
        }
        _ => {}
    }
    Ok(())
}

async fn handle_command(msg_tx: &mpsc::Sender<Msg>, app: &mut App, cmd: &str) {
    let (name, rest) = match cmd.split_once([' ', '\t']) {
        Some((n, r)) => (n.to_lowercase(), r.trim().to_string()),
        None => (cmd.to_lowercase(), String::new()),
    };

    match name.as_str() {
        "/help" | "/?" => app.show_help = !app.show_help,
        "/quit" | "/exit" => std::process::exit(0),
        "/clear" => {
            app.messages.clear();
            app.follow_bottom();
        }
        "/provider" => {
            app.push_system(format!(
                "provider: {} | model: {}",
                app.session.provider_label, app.session.agent.model
            ));
        }
        "/cfg" => {
            let cfg =
                serde_yaml::to_string(&app.session.config).unwrap_or_else(|_| "parse error".into());
            let _ = msg_tx.send(Msg::App(AppMsg::Plan(cfg))).await;
        }
        "/new" => {
            app.session.agent.set_history(vec![]);
            app.messages.push(MessageItem::system(
                "Session reset. Previous conversation cleared.".to_string(),
            ));
        }
        "/model" => {
            if rest.is_empty() {
                app.push_system("usage: /model <name>".to_string());
                return;
            }
            match app.session.set_model(rest.clone()) {
                Ok(()) => app.push_system(format!(
                    "switched to {} via {}",
                    rest, app.session.provider_label
                )),
                Err(e) => app.push_error(e.to_string()),
            }
        }
        "/plan" => {
            if rest.is_empty() {
                app.push_system("usage: /plan <task>".to_string());
                return;
            }
            app.push_user(format!("/plan {rest}"));
            app.begin_run();
            let agent = app.session.agent.clone();
            let plan_tx = msg_tx.clone();
            let task = rest.clone();
            let cwd = app.session.cwd.clone();
            let plan_run_id = app.run_id;
            let handle = tokio::spawn(async move {
                let result = agent.plan(&task).await;
                match result {
                    Ok(plan) => {
                        let _ = crate::agent::steer::save_plan(&cwd, &plan);
                        let _ = plan_tx.send(Msg::App(AppMsg::Plan(plan))).await;
                    }
                    Err(e) => {
                        let _ = plan_tx.send(Msg::App(AppMsg::Error(plan_run_id, e.to_string()))).await;
                    }
                }
            });
            app.run_handle = Some(handle);
        }
        "/spec" => {
            let plan = crate::agent::steer::read_plan(&app.session.cwd).unwrap_or(None);
            match plan {
                Some(p) => app.push_plan(p),
                None => app.push_system("No plan found. Run /plan <task> first.".to_string()),
            }
        }
        "/steer" => {
            if rest.is_empty() {
                app.push_system("usage: /steer <rule text>".to_string());
                return;
            }
            let cwd = app.session.cwd.clone();
            let root = crate::agent::steer::find_project_root(&cwd).unwrap_or(cwd.clone());
            let dir = root.join(".vibectl");
            let _ = std::fs::create_dir_all(&dir);
            let path = dir.join("steer.md");
            let mut existing = std::fs::read_to_string(&path).unwrap_or_default();
            if !existing.ends_with('\n') && !existing.is_empty() {
                existing.push('\n');
            }
            existing.push_str(&format!("- {rest}\n"));
            match std::fs::write(&path, &existing) {
                Ok(()) => app.push_system(format!("steering appended to {}", path.display())),
                Err(e) => app.push_error(e.to_string()),
            }
        }
        _ => {
            app.push_system(format!("unknown command {name}. Type /help for commands."));
        }
    }
}

fn spawn_agent(msg_tx: mpsc::Sender<Msg>, app: &mut App, prompt: String) {
    let agent = app.session.agent.clone();
    let (mut stream, handle) = agent.spawn_run(prompt);
    // capture the run_id at the moment of spawn — stale events will be filtered
    let run_id = app.run_id;
    app.run_handle = Some(handle);
    tokio::spawn(async move {
        while let Some(ev) = stream.recv().await {
            let msg = match ev {
                AgentEvent::Text(t) => AppMsg::Text(run_id, t),
                AgentEvent::ToolCall { id: _, name } => AppMsg::ToolStart(run_id, name),
                AgentEvent::ToolResult { name, content, .. } => AppMsg::ToolResult {
                    run_id,
                    name,
                    content,
                    ok: true,
                },
                AgentEvent::ToolError { name, error, .. } => AppMsg::ToolResult {
                    run_id,
                    name,
                    content: error,
                    ok: false,
                },
                AgentEvent::Done { .. } => AppMsg::Done(run_id),
                AgentEvent::Error(e) => AppMsg::Error(run_id, e),
            };
            if msg_tx.send(Msg::App(msg)).await.is_err() {
                break;
            }
        }
    });
}
