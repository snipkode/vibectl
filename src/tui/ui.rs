use crate::tui::app::{App, HELP_TEXT, MsgRole};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap};
use unicode_width::UnicodeWidthChar;

// ─── Palette ──────────────────────────────────────────────────────────────────

const C_SURFACE: Color = Color::Rgb(18, 18, 26);
const C_SURFACE2: Color = Color::Rgb(24, 24, 34);
const C_BORDER: Color = Color::Rgb(48, 54, 72);

const C_HDR_BG: Color = Color::Rgb(14, 14, 22);
const C_HDR_LOGO: Color = Color::Rgb(80, 160, 240);   // brand blue
const C_HDR_SEP: Color = Color::Rgb(40, 46, 64);
const C_HDR_META: Color = Color::Rgb(80, 90, 120);
const C_HDR_VAL: Color = Color::Rgb(130, 145, 185);

const C_USER_MARK: Color = Color::Rgb(80, 205, 130);
const C_USER_TEXT: Color = Color::Rgb(225, 232, 245);

const C_AGENT_MARK: Color = Color::Rgb(90, 165, 245);
const C_AGENT_TEXT: Color = Color::Rgb(195, 218, 255);

const C_TOOL_MARK: Color = Color::Rgb(195, 148, 58);
const C_TOOL_TEXT: Color = Color::Rgb(140, 122, 72);

const C_ERROR_MARK: Color = Color::Rgb(238, 82, 82);
const C_ERROR_TEXT: Color = Color::Rgb(255, 145, 145);

const C_SYS_TEXT: Color = Color::Rgb(82, 90, 118);
const C_PLAN_MARK: Color = Color::Rgb(198, 178, 78);
const C_PLAN_TEXT: Color = Color::Rgb(218, 198, 98);
const C_CODE_TEXT: Color = Color::Rgb(125, 178, 125);

const C_INPUT_BG: Color = Color::Rgb(20, 22, 32);
const C_INPUT_BORDER: Color = Color::Rgb(45, 55, 80);
const C_INPUT_ACTIVE: Color = Color::Rgb(72, 120, 195);
const C_PROMPT: Color = Color::Rgb(80, 205, 130);
const C_CURSOR: Color = Color::Rgb(80, 205, 130);
const C_HINT: Color = Color::Rgb(58, 64, 88);
const C_PLACEHOLDER: Color = Color::Rgb(68, 76, 104);

const SPINNER: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

// Cursor blinks every 8 frames (~640ms at 80ms tick)
const BLINK_PERIOD: u64 = 8;

fn spinner_char(app: &App) -> char {
    SPINNER[(app.frame as usize) % SPINNER.len()]
}

fn cursor_visible(app: &App) -> bool {
    // blink off when busy, solid when idle
    !app.busy && (app.frame / BLINK_PERIOD) % 2 == 0
}

// ─── Layout ───────────────────────────────────────────────────────────────────

/// Split the screen into [header, body, input].
/// header = 2 rows (logo + separator), input = 5 rows (padded box), body = rest.
fn main_areas(area: Rect, with_input: bool) -> [Rect; 3] {
    let input_h: u16 = if with_input { 5 } else { 0 };
    let [header, rest] =
        Layout::vertical([Constraint::Length(2), Constraint::Min(0)]).areas(area);
    let [body, input] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(input_h)]).areas(rest);
    [header, body, input]
}

/// Inset a rect by `h` columns on each side, clamped.
fn h_inset(r: Rect, h: u16) -> Rect {
    let pad = h.min(r.width / 2);
    Rect {
        x: r.x + pad,
        y: r.y,
        width: r.width.saturating_sub(pad * 2),
        height: r.height,
    }
}

// ─── Entry point ──────────────────────────────────────────────────────────────

pub fn render(frame: &mut Frame, app: &App) {
    // fill entire background
    frame.render_widget(
        Block::default().style(Style::default().bg(C_SURFACE)),
        frame.area(),
    );

    if app.pending_approval.is_some() {
        let [header, body, _] = main_areas(frame.area(), false);
        render_header(frame, header, app);
        render_body(frame, body, app);
        render_approval_modal(frame, app);
        return;
    }

    let [header, body, input] = main_areas(frame.area(), true);
    render_header(frame, header, app);
    render_body(frame, body, app);
    render_input(frame, input, app);
}

// ─── Header ───────────────────────────────────────────────────────────────────
//
//  Row 0:  ▌ vibectl  ·  provider · model · cwd          [spinner / hint]
//  Row 1:  thin separator line
//  Row 2:  (body starts)

fn render_header(frame: &mut Frame, area: Rect, app: &App) {
    // background fill
    frame.render_widget(
        Block::default().style(Style::default().bg(C_HDR_BG)),
        area,
    );

    if area.height < 1 {
        return;
    }

    let total_w = area.width as usize;

    // ── logo row (row 0) ─────────────────────────────────────────────────────
    let logo_row = Rect { height: 1, ..area };

    // left: " ▌ vibectl  ·  provider  ·  model  ·  cwd"
    let mut left: Vec<Span> = vec![
        Span::raw(" "),
        Span::styled("▌", Style::default().fg(C_HDR_LOGO).add_modifier(Modifier::BOLD)),
        Span::raw(" "),
        Span::styled(
            "vibectl",
            Style::default()
                .fg(C_HDR_LOGO)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("  ·  ", Style::default().fg(C_HDR_SEP)),
        Span::styled(app.session.provider_label.clone(), Style::default().fg(C_HDR_VAL)),
        Span::styled("  ·  ", Style::default().fg(C_HDR_SEP)),
        Span::styled(app.session.agent.model.clone(), Style::default().fg(C_HDR_VAL)),
        Span::styled("  ·  ", Style::default().fg(C_HDR_SEP)),
        Span::styled(
            app.session.cwd.display().to_string(),
            Style::default().fg(C_HDR_META),
        ),
    ];

    // right: spinner state or idle hint
    let right_str = if app.busy {
        let sp = spinner_char(app);
        if let Some(tool) = &app.running_tool {
            format!("  {sp} {tool}  ")
        } else {
            format!("  {sp} thinking…  ")
        }
    } else {
        if app.scroll_offset > 0 {
            format!("  ↑{}  ? help  ", app.scroll_offset)
        } else {
            "  ? help  ".to_string()
        }
    };
    let right_color = if app.busy { C_TOOL_MARK } else { C_HINT };

    // measure left width to decide if right fits
    let left_w: usize = left.iter().map(|s| s.content.chars().count()).sum();
    let right_w = right_str.chars().count();

    if left_w + right_w < total_w {
        let pad = total_w.saturating_sub(left_w + right_w);
        left.push(Span::raw(" ".repeat(pad)));
        left.push(Span::styled(right_str, Style::default().fg(right_color)));
    }

    frame.render_widget(
        Paragraph::new(Line::from(left)).style(Style::default().bg(C_HDR_BG)),
        logo_row,
    );

    // ── separator row (row 1) ────────────────────────────────────────────────
    if area.height >= 2 {
        let sep_row = Rect {
            y: area.y + 1,
            height: 1,
            ..area
        };
        let sep_line = "─".repeat(total_w);
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                sep_line,
                Style::default().fg(C_HDR_SEP),
            )))
            .style(Style::default().bg(C_HDR_BG)),
            sep_row,
        );
    }
}

// ─── Chat body ────────────────────────────────────────────────────────────────

fn render_body(frame: &mut Frame, area: Rect, app: &App) {
    // outer rounded box
    let block = Block::default()
        .borders(Borders::LEFT | Borders::RIGHT | Borders::BOTTOM)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(C_BORDER))
        .style(Style::default().bg(C_SURFACE));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    // horizontal padding: 2 cols each side inside the box
    let padded = inner.inner(Margin { horizontal: 2, vertical: 0 });
    let inner_w = padded.width.max(1) as usize;
    let inner_h = padded.height.max(1) as usize;

    let rows = if app.show_help {
        help_rows()
    } else {
        message_rows(app, inner_w)
    };

    let expanded = wrap_rows(rows, inner_w);
    let total = expanded.len();
    let max_offset = total.saturating_sub(inner_h);
    let offset = app.scroll_offset.min(max_offset);
    let start = max_offset - offset;
    let end = (start + inner_h).min(total);
    let visible: Vec<Line> = expanded[start..end].to_vec();

    frame.render_widget(Paragraph::new(visible), padded);
}

// ─── Input box ────────────────────────────────────────────────────────────────

fn render_input(frame: &mut Frame, area: Rect, app: &App) {
    // ── outer box: 2-col margin each side, full height (5 rows) ─────────────
    //
    //  ╭────────────────────────────────────────────────────────────────────╮
    //  │                                                                    │  ← blank top
    //  │  ❯  Message vibectl…▋                          Shift+↵ newline    │  ← input row
    //  │                                                  Enter ↵  send    │  ← hint row
    //  ╰────────────────────────────────────────────────────────────────────╯
    //
    let outer = h_inset(area, 2);

    let border_color = if app.busy { C_INPUT_BORDER } else { C_INPUT_ACTIVE };
    let bg_color = C_INPUT_BG;

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border_color))
        .style(Style::default().bg(bg_color));

    let inner = block.inner(outer);
    frame.render_widget(block, outer);

    // inner has 3 rows:  row 0 = blank, row 1 = text input, row 2 = hint bar
    if inner.height < 2 {
        return;
    }

    let input_row = Rect { y: inner.y + 1, height: 1, ..inner };
    let hint_row  = Rect { y: inner.y + 2, height: 1, ..inner };

    // ── horizontal padding inside box ────────────────────────────────────────
    let pad: u16 = 2;
    let text_area = Rect {
        x: inner.x + pad,
        width: inner.width.saturating_sub(pad * 2),
        ..input_row
    };
    let hint_area = Rect {
        x: inner.x + pad,
        width: inner.width.saturating_sub(pad * 2),
        ..hint_row
    };

    if text_area.width < 4 {
        return;
    }

    // ── build input line ─────────────────────────────────────────────────────
    let (prompt_char, prompt_color) = if app.busy {
        ("·", C_HINT)
    } else {
        ("❯", C_PROMPT)
    };

    let mut spans: Vec<Span> = vec![Span::styled(
        format!("{prompt_char} "),
        Style::default()
            .fg(prompt_color)
            .add_modifier(Modifier::BOLD),
    )];

    let input    = &app.input;
    let cursor   = app.cursor;
    let char_len = input.chars().count();
    let show_cur = cursor_visible(app);

    if input.is_empty() {
        spans.push(Span::styled(
            "Message vibectl…",
            Style::default().fg(C_PLACEHOLDER),
        ));
        if show_cur && !app.busy {
            spans.push(Span::styled(
                "▋",
                Style::default().fg(C_CURSOR),
            ));
        }
    } else {
        let before: String = input.chars().take(cursor).collect();
        if !before.is_empty() {
            spans.push(Span::styled(before, Style::default().fg(C_USER_TEXT)));
        }

        if app.busy {
            let rest: String = input.chars().skip(cursor).collect();
            if !rest.is_empty() {
                spans.push(Span::styled(rest, Style::default().fg(C_HDR_META)));
            }
        } else if cursor < char_len {
            let at: String = input.chars().nth(cursor).unwrap().to_string();
            if show_cur {
                spans.push(Span::styled(
                    at,
                    Style::default().bg(C_CURSOR).fg(C_SURFACE),
                ));
            } else {
                spans.push(Span::styled(at, Style::default().fg(C_USER_TEXT)));
            }
            let after: String = input.chars().skip(cursor + 1).collect();
            if !after.is_empty() {
                spans.push(Span::styled(after, Style::default().fg(C_USER_TEXT)));
            }
        } else if show_cur {
            spans.push(Span::styled("▋", Style::default().fg(C_CURSOR)));
        }
    }

    // ── right badge: char count OR spinner, right-aligned inside box ─────────
    let badge: Option<Vec<Span>> = if app.busy {
        Some(vec![
            Span::styled(
                format!("{} ", spinner_char(app)),
                Style::default().fg(C_TOOL_MARK),
            ),
        ])
    } else if char_len > 0 {
        Some(vec![Span::styled(
            format!("{char_len} ch "),
            Style::default().fg(C_HINT).add_modifier(Modifier::DIM),
        )])
    } else {
        None
    };

    if let Some(badge_spans) = badge {
        let text_w: usize = spans.iter().map(|s| s.content.chars().count()).sum();
        let badge_w: usize = badge_spans.iter().map(|s| s.content.chars().count()).sum();
        let avail = text_area.width as usize;
        if text_w + badge_w + 1 <= avail {
            let gap = avail.saturating_sub(text_w + badge_w);
            spans.push(Span::raw(" ".repeat(gap)));
            spans.extend(badge_spans);
        }
    }

    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::default().bg(bg_color)),
        text_area,
    );

    // ── hint bar (row below input) ────────────────────────────────────────────
    //  left:   /help for commands  OR  working…
    //  right:  Shift+↵ newline · Enter ↵ send  OR  Ctrl+C cancel
    let (hint_left_str, hint_right_pieces): (&str, &[(&str, Color)]) = if app.busy {
        (
            "  working…",
            &[
                ("Ctrl+C ", C_ERROR_MARK),
                ("cancel  ", C_HINT),
            ],
        )
    } else {
        (
            "  /help for commands",
            &[
                ("Shift+↵ ", C_HDR_META),
                ("newline  ·  ", C_HINT),
                ("Enter ↵ ", C_AGENT_MARK),
                ("send  ", C_HINT),
            ],
        )
    };

    let right_w: usize = hint_right_pieces.iter().map(|(s, _)| s.chars().count()).sum();
    let left_w  = hint_left_str.chars().count();
    let avail   = hint_area.width as usize;

    let mut hint_spans = vec![Span::styled(
        hint_left_str.to_string(),
        Style::default().fg(if app.busy { C_TOOL_MARK } else { C_HINT }),
    )];

    if left_w + right_w < avail {
        let gap = avail.saturating_sub(left_w + right_w);
        hint_spans.push(Span::raw(" ".repeat(gap)));
        for (text, color) in hint_right_pieces {
            hint_spans.push(Span::styled(text.to_string(), Style::default().fg(*color)));
        }
    }

    frame.render_widget(
        Paragraph::new(Line::from(hint_spans)).style(Style::default().bg(bg_color)),
        hint_area,
    );
}

// ─── Approval modal ───────────────────────────────────────────────────────────

fn render_approval_modal(frame: &mut Frame, app: &App) {
    let cmd = match &app.pending_approval {
        Some(c) => c.clone(),
        None => return,
    };

    let area = frame.area();
    let modal_w = (area.width.min(78)).max(44);
    let modal_h: u16 = 7;
    let modal = Rect {
        x: area.width.saturating_sub(modal_w) / 2,
        y: area.height.saturating_sub(modal_h) / 2,
        width: modal_w,
        height: modal_h,
    };

    frame.render_widget(Clear, modal);

    let block = Block::default()
        .title(Span::styled(
            "  ⚠  Approval Required  ",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Yellow))
        .style(Style::default().bg(C_SURFACE2));

    let inner = block.inner(modal);
    frame.render_widget(block, modal);

    let max_cmd_w = inner.width as usize - 2;
    let truncated: String = if cmd.chars().count() > max_cmd_w {
        format!("{}…", cmd.chars().take(max_cmd_w - 1).collect::<String>())
    } else {
        cmd.clone()
    };

    let lines = vec![
        Line::raw(""),
        Line::from(Span::styled(
            format!("  {truncated}"),
            Style::default().fg(Color::White),
        )),
        Line::raw(""),
        Line::from(vec![
            Span::styled("  [y] ", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
            Span::styled("Allow    ", Style::default().fg(C_HDR_META)),
            Span::styled("[n] ", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
            Span::styled("Deny    ", Style::default().fg(C_HDR_META)),
            Span::styled("[Esc] ", Style::default().fg(C_HINT)),
            Span::styled("Cancel", Style::default().fg(C_HINT)),
        ]),
    ];

    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .style(Style::default().bg(C_SURFACE2)),
        inner,
    );
}

// ─── Help ─────────────────────────────────────────────────────────────────────

fn help_rows() -> Vec<Line<'static>> {
    HELP_TEXT
        .lines()
        .map(|l| {
            let trimmed = l.trim_start();
            let style = if trimmed.starts_with('═') || trimmed.starts_with("────") {
                Style::default().fg(C_BORDER)
            } else if trimmed.starts_with('/') {
                Style::default().fg(C_AGENT_TEXT)
            } else if l.trim().is_empty() {
                Style::default()
            } else if trimmed.chars().all(|c| c.is_uppercase() || c == ' ' || c == '/')
                && !trimmed.is_empty()
                && trimmed.len() > 3
            {
                Style::default()
                    .fg(C_AGENT_MARK)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(C_SYS_TEXT)
            };
            Line::from(Span::styled(l.to_string(), style))
        })
        .collect()
}

// ─── Message rows ─────────────────────────────────────────────────────────────

fn message_rows(app: &App, width: usize) -> Vec<Line<'static>> {
    let mut rows: Vec<Line<'static>> = Vec::new();
    let mut prev_role: Option<MsgRole> = None;

    for (i, msg) in app.messages.iter().enumerate() {
        let same_group = prev_role.as_ref().map(|r| r == &msg.role).unwrap_or(false);
        let is_tool = msg.role == MsgRole::Tool;
        let prev_tool = prev_role
            .as_ref()
            .map(|r| r == &MsgRole::Tool)
            .unwrap_or(false);

        if i > 0 && !same_group && !(is_tool && prev_tool) {
            rows.push(Line::raw(""));
        }

        match msg.role {
            MsgRole::User => append_user(&mut rows, msg, width),
            MsgRole::Assistant => append_agent(&mut rows, msg),
            MsgRole::Tool => append_tool(&mut rows, msg),
            MsgRole::Error => append_error(&mut rows, msg),
            MsgRole::System => append_system(&mut rows, msg),
            MsgRole::Plan => append_plan(&mut rows, msg),
        }

        prev_role = Some(msg.role.clone());
    }

    if rows.is_empty() {
        rows.push(Line::raw(""));
    }
    rows
}

// ─── Per-role renderers ───────────────────────────────────────────────────────

fn bubble_header(mark: Color, label: &str, ts: &str, width: usize) -> Line<'static> {
    let label_owned = label.to_string();
    let ts_owned = ts.to_string();
    let dash_w = width
        .saturating_sub(label_owned.chars().count() + ts_owned.chars().count() + 8)
        .max(1);
    Line::from(vec![
        Span::styled("╭─", Style::default().fg(mark).add_modifier(Modifier::DIM)),
        Span::styled(
            format!(" {label_owned} "),
            Style::default().fg(mark).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("{:─<w$}", "", w = dash_w),
            Style::default().fg(mark).add_modifier(Modifier::DIM),
        ),
        Span::styled(
            format!(" {ts_owned} "),
            Style::default().fg(mark).add_modifier(Modifier::DIM),
        ),
    ])
}

fn bubble_line(mark: Color, spans: Vec<Span<'static>>) -> Line<'static> {
    let mut line = Line::from(vec![Span::styled(
        "│ ",
        Style::default().fg(mark).add_modifier(Modifier::DIM),
    )]);
    line.spans.extend(spans);
    line
}

fn append_user(
    rows: &mut Vec<Line<'static>>,
    msg: &crate::tui::app::MessageItem,
    width: usize,
) {
    rows.push(bubble_header(C_USER_MARK, "You", &msg.ts, width));
    for line in msg.text.lines() {
        rows.push(bubble_line(
            C_USER_MARK,
            vec![Span::styled(
                line.to_string(),
                Style::default().fg(C_USER_TEXT),
            )],
        ));
    }
}

fn append_agent(rows: &mut Vec<Line<'static>>, msg: &crate::tui::app::MessageItem) {
    rows.push(bubble_header(C_AGENT_MARK, "Agent", &msg.ts, 60));
    let body_lines = markdown_to_lines(&msg.text, Style::default().fg(C_AGENT_TEXT));
    for line in body_lines {
        rows.push(bubble_line(C_AGENT_MARK, line.spans));
    }
}

fn append_tool(rows: &mut Vec<Line<'static>>, msg: &crate::tui::app::MessageItem) {
    let name = msg.kind.as_deref().unwrap_or("tool");
    let icon = tool_icon(name);
    let (mark, status) = if msg.ok {
        (C_TOOL_MARK, "")
    } else {
        (C_ERROR_MARK, " ✗")
    };

    rows.push(Line::from(vec![
        Span::styled("  ", Style::default()),
        Span::styled(
            format!("{icon} {name}{status}"),
            Style::default().fg(mark).add_modifier(Modifier::DIM),
        ),
    ]));

    for line in &compact_tool_output(&msg.text) {
        rows.push(Line::from(vec![
            Span::styled("    ", Style::default()),
            Span::styled(line.clone(), Style::default().fg(C_TOOL_TEXT)),
        ]));
    }
}

fn append_error(rows: &mut Vec<Line<'static>>, msg: &crate::tui::app::MessageItem) {
    rows.push(bubble_header(C_ERROR_MARK, "Error", &msg.ts, 60));
    for line in msg.text.lines() {
        rows.push(bubble_line(
            C_ERROR_MARK,
            vec![Span::styled(
                line.to_string(),
                Style::default().fg(C_ERROR_TEXT),
            )],
        ));
    }
}

fn append_system(rows: &mut Vec<Line<'static>>, msg: &crate::tui::app::MessageItem) {
    for line in msg.text.lines() {
        rows.push(Line::from(vec![
            Span::styled("  · ", Style::default().fg(C_SYS_TEXT)),
            Span::styled(line.to_string(), Style::default().fg(C_SYS_TEXT)),
        ]));
    }
}

fn append_plan(rows: &mut Vec<Line<'static>>, msg: &crate::tui::app::MessageItem) {
    rows.push(bubble_header(C_PLAN_MARK, "Plan", &msg.ts, 60));
    let body_lines = markdown_to_lines(&msg.text, Style::default().fg(C_PLAN_TEXT));
    for line in body_lines {
        rows.push(bubble_line(C_PLAN_MARK, line.spans));
    }
}

// ─── Markdown ─────────────────────────────────────────────────────────────────

fn markdown_to_lines(md: &str, base: Style) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut in_code = false;
    let mut code_buf: Vec<String> = Vec::new();
    let mut lang = String::new();

    for src in md.lines() {
        let trimmed = src.trim();

        if trimmed.starts_with("```") {
            if in_code {
                flush_code_block(&mut code_buf, &mut lines, &lang);
                lang.clear();
                in_code = false;
            } else {
                lang = trimmed.trim_start_matches('`').to_string();
                in_code = true;
            }
            continue;
        }

        if in_code {
            code_buf.push(src.to_string());
            continue;
        }

        let line = if let Some(rest) = trimmed
            .strip_prefix("### ")
            .or_else(|| trimmed.strip_prefix("## "))
            .or_else(|| trimmed.strip_prefix("# "))
        {
            Line::from(Span::styled(
                rest.to_string(),
                base.add_modifier(Modifier::BOLD),
            ))
        } else if let Some(rest) = trimmed.strip_prefix("> ") {
            Line::from(vec![
                Span::styled(
                    "▍ ",
                    Style::default()
                        .fg(C_AGENT_MARK)
                        .add_modifier(Modifier::DIM),
                ),
                Span::styled(rest.to_string(), base.add_modifier(Modifier::ITALIC)),
            ])
        } else if !trimmed.is_empty()
            && trimmed.chars().all(|c| c == '-' || c == '*' || c == '=')
        {
            Line::from(Span::styled(
                "─────────────────────────────────",
                Style::default().fg(C_BORDER),
            ))
        } else if let Some(rest) = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
        {
            let mut inline = vec![Span::styled(
                "• ",
                Style::default().fg(C_AGENT_MARK),
            )];
            inline.extend(parse_inline(rest, base));
            Line::from(inline)
        } else if trimmed
            .chars()
            .next()
            .map(|c| c.is_ascii_digit())
            .unwrap_or(false)
        {
            let parts: Vec<&str> = trimmed.splitn(2, ". ").collect();
            if parts.len() == 2 {
                let mut inline = vec![Span::styled(
                    format!("{}. ", parts[0]),
                    Style::default()
                        .fg(C_AGENT_MARK)
                        .add_modifier(Modifier::BOLD),
                )];
                inline.extend(parse_inline(parts[1], base));
                Line::from(inline)
            } else {
                markdown_inline(src, base)
            }
        } else {
            markdown_inline(src, base)
        };

        lines.push(line);
    }
    flush_code_block(&mut code_buf, &mut lines, &lang);

    while lines
        .last()
        .map(|l: &Line| l.spans.iter().all(|s| s.content.trim().is_empty()))
        .unwrap_or(false)
    {
        lines.pop();
    }
    lines
}

fn flush_code_block(buf: &mut Vec<String>, lines: &mut Vec<Line<'static>>, lang: &str) {
    if buf.is_empty() {
        return;
    }
    let label = if lang.is_empty() {
        " code ".to_string()
    } else {
        format!(" {lang} ")
    };
    lines.push(Line::from(Span::styled(
        format!("╔═{label}═"),
        Style::default().fg(C_BORDER),
    )));
    for l in buf.drain(..) {
        lines.push(Line::from(vec![
            Span::styled("║ ", Style::default().fg(C_BORDER)),
            Span::styled(l, Style::default().fg(C_CODE_TEXT)),
        ]));
    }
    lines.push(Line::from(Span::styled(
        "╚══════".to_string(),
        Style::default().fg(C_BORDER),
    )));
    lines.push(Line::raw(""));
}

fn markdown_inline(src: &str, base: Style) -> Line<'static> {
    let spans = parse_inline(src, base);
    if spans.is_empty() {
        Line::raw(src.to_string())
    } else {
        Line::from(spans)
    }
}

fn parse_inline(src: &str, base: Style) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut rest = src;
    while !rest.is_empty() {
        if rest.starts_with('`') {
            if let Some(i) = rest[1..].find('`') {
                let end = i + 1;
                spans.push(Span::styled(
                    rest[1..end].to_string(),
                    Style::default().fg(C_CODE_TEXT),
                ));
                rest = &rest[end + 1..];
                continue;
            }
        }
        if rest.starts_with("**") {
            if let Some(i) = rest[2..].find("**") {
                let end = i + 2;
                spans.push(Span::styled(
                    rest[2..end].to_string(),
                    base.add_modifier(Modifier::BOLD),
                ));
                rest = &rest[end + 2..];
                continue;
            }
        }
        if rest.starts_with('*') {
            if let Some(i) = rest[1..].find('*') {
                let end = i + 1;
                spans.push(Span::styled(
                    rest[1..end].to_string(),
                    base.add_modifier(Modifier::ITALIC),
                ));
                rest = &rest[end + 1..];
                continue;
            }
        }
        let next = rest
            .find(|c| c == '`' || c == '*')
            .unwrap_or(rest.len());
        if next == 0 {
            spans.push(Span::styled(rest[..1].to_string(), base));
            rest = &rest[1..];
        } else {
            spans.push(Span::styled(rest[..next].to_string(), base));
            rest = &rest[next..];
        }
    }
    spans
}

// ─── Word wrap ────────────────────────────────────────────────────────────────

fn wrap_rows(rows: Vec<Line<'static>>, width: usize) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    for line in rows {
        let mut wrapped = Vec::new();
        wrap_line(line, width, &mut wrapped);
        if wrapped.is_empty() {
            out.push(Line::default());
        } else {
            out.extend(wrapped);
        }
    }
    out
}

fn wrap_line(line: Line<'static>, width: usize, out: &mut Vec<Line<'static>>) {
    let mut current: Vec<Span<'static>> = Vec::new();
    let mut col = 0usize;

    for span in line.spans {
        let style = span.style;
        let content: String = span.content.into_owned();
        let mut seg = String::new();
        let mut segw = 0usize;

        for ch in content.chars() {
            let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
            if col + segw + cw > width && col + segw > 0 {
                current.push(Span::styled(std::mem::take(&mut seg), style));
                out.push(Line::from(std::mem::take(&mut current)));
                col = 0;
                segw = 0;
            }
            seg.push(ch);
            segw += cw;
        }
        if !seg.is_empty() {
            current.push(Span::styled(seg, style));
            col += segw;
        }
    }
    if !current.is_empty() {
        out.push(Line::from(current));
    }
}

// ─── Tool helpers ─────────────────────────────────────────────────────────────

fn tool_icon(name: &str) -> &'static str {
    if name.contains("read") {
        "◎"
    } else if name.contains("write") {
        "✎"
    } else if name.contains("glob") || name.contains("find") {
        "◈"
    } else if name.contains("grep") || name.contains("search") {
        "⌕"
    } else if name.contains("git") {
        "⎇"
    } else if name.contains("shell") || name.contains("exec") {
        "$"
    } else {
        "○"
    }
}

fn compact_tool_output(text: &str) -> Vec<String> {
    const MAX_LINES: usize = 4;
    const MAX_CHARS: usize = 110;

    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();

    if lines.is_empty() {
        return vec![];
    }

    let total = lines.len();
    let mut out: Vec<String> = lines
        .iter()
        .take(MAX_LINES)
        .map(|l| {
            if l.chars().count() > MAX_CHARS {
                format!("{}…", l.chars().take(MAX_CHARS).collect::<String>())
            } else {
                l.to_string()
            }
        })
        .collect();

    if total > MAX_LINES {
        out.push(format!("  ⋯ +{} lines", total - MAX_LINES));
    }
    out
}
