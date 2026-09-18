use crate::tui::app::{App, HELP_TEXT, MsgRole};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use unicode_width::UnicodeWidthChar;

const SPINNER: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

fn spinner(app: &App) -> char {
    SPINNER[(app.frame as usize) % SPINNER.len()]
}

pub fn render(frame: &mut Frame, app: &App) {
    let [header, body, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(if app.pending_approval.is_some() { 3 } else { 1 }),
    ])
    .areas(frame.area());

    render_header(frame, header, app);
    render_body(frame, body, app);
    render_footer(frame, footer, app);
}

fn render_header(frame: &mut Frame, area: Rect, app: &App) {
    let mut status = if app.busy {
        let sp = spinner(app);
        match &app.running_tool {
            Some(tool) => format!(" {sp} running: {tool}"),
            None => format!(" {sp} working…"),
        }
    } else {
        String::new()
    };
    if app.scroll_offset > 0 {
        status.push_str(&format!("  ↑{}", app.scroll_offset));
    }
    let line = Line::from(Span::styled(
        format!("{}{}", app.model_line(), status),
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    ));
    frame.render_widget(Paragraph::new(line), area);
}

fn render_footer(frame: &mut Frame, area: Rect, app: &App) {
    if let Some(cmd) = &app.pending_approval {
        let text = Line::from(vec![
            Span::styled(
                " Approve? [y]es [n]o (esc=cancel)",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!(" :: {cmd}"), Style::default().fg(Color::Gray)),
        ]);
        frame.render_widget(
            Paragraph::new(text).block(
                Block::default()
                    .borders(Borders::TOP)
                    .border_style(Style::default().fg(Color::Yellow)),
            ),
            area,
        );
        return;
    }
    let cursor_vis = if app.busy { '·' } else { '█' };
    let indicator = if app.busy {
        Span::styled(
            spinner(app).to_string(),
            Style::default().fg(Color::Magenta),
        )
    } else {
        Span::styled("❯", Style::default().fg(Color::Green))
    };
    let line = Line::from(vec![
        indicator,
        Span::raw(" "),
        Span::styled(
            app.input.clone(),
            Style::default().fg(if app.busy {
                Color::DarkGray
            } else {
                Color::White
            }),
        ),
        Span::styled(
            format!(" {cursor_vis}"),
            Style::default().fg(Color::DarkGray),
        ),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

fn render_body(frame: &mut Frame, area: Rect, app: &App) {
    let width = area.width.max(1) as usize;
    let view_h = area.height.max(1) as usize;

    let rows = if app.show_help {
        help_rows()
    } else {
        message_rows(app)
    };

    let expanded = wrap_rows(rows, width);
    let total = expanded.len();
    let max_offset = total.saturating_sub(view_h);
    let offset = app.scroll_offset.min(max_offset);
    let start = max_offset - offset;
    let end = (start + view_h).min(total);
    let visible: Vec<Line> = expanded[start..end].to_vec();

    let p = Paragraph::new(visible).block(
        Block::default()
            .borders(Borders::ALL)
            .title(if app.show_help { " Help " } else { " Chat " }),
    );
    frame.render_widget(p, area);
}

fn help_rows() -> Vec<Line<'static>> {
    HELP_TEXT
        .lines()
        .map(|l| {
            Line::from(Span::styled(
                l.to_string(),
                if l.starts_with('═') {
                    Style::default().fg(Color::DarkGray)
                } else if l.trim().starts_with('/') {
                    Style::default().fg(Color::White)
                } else if l.is_empty() {
                    Style::default()
                } else {
                    Style::default().fg(Color::Cyan)
                },
            ))
        })
        .collect()
}

fn message_rows(app: &App) -> Vec<Line<'static>> {
    let mut rows: Vec<Line<'static>> = Vec::new();
    for msg in &app.messages {
        match msg.role {
            MsgRole::Tool => {
                append_tool(&mut rows, msg.kind.clone().unwrap_or_default(), &msg.text)
            }
            _ => append_text(&mut rows, &msg.role, &msg.text),
        }
    }
    if rows.is_empty() {
        rows.push(Line::raw(""));
    }
    rows
}

fn append_text(rows: &mut Vec<Line<'static>>, role: &MsgRole, text: &str) {
    let (prefix, pstyle, body_style) = match role {
        MsgRole::User => (
            "You: ",
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
            Style::default().fg(Color::White),
        ),
        MsgRole::Assistant => (
            "Agent: ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
            Style::default().fg(Color::LightGreen),
        ),
        MsgRole::Error => (
            "✖ ",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            Style::default().fg(Color::Red),
        ),
        MsgRole::System => ("", Style::default(), Style::default().fg(Color::DarkGray)),
        MsgRole::Plan => (
            "◈ ",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
            Style::default().fg(Color::Cyan),
        ),
        MsgRole::Tool => unreachable!(),
    };
    let mut body = markdown_to_lines(text, body_style);
    if body.is_empty() {
        body.push(Line::raw(""));
    }
    let mut first = Line::from(Span::styled(prefix, pstyle));
    if let Some(l) = body.first_mut() {
        first.spans.extend(std::mem::take(&mut l.spans));
    }
    rows.push(first);
    rows.extend(body.into_iter().skip(1));
}

fn append_tool(rows: &mut Vec<Line<'static>>, kind: String, text: &str) {
    rows.push(Line::from(vec![
        Span::styled(
            "◧ ",
            Style::default()
                .fg(Color::Magenta)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            kind,
            Style::default()
                .fg(Color::Magenta)
                .add_modifier(Modifier::BOLD),
        ),
    ]));
    for line in tool_summary(text) {
        rows.push(Line::from(Span::styled(
            line,
            Style::default().fg(Color::DarkGray),
        )));
    }
}

fn tool_summary(text: &str) -> Vec<String> {
    const MAX_LINES: usize = 5;
    const MAX_CHARS: usize = 200;
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    if lines.is_empty() {
        return vec!["(empty)".to_string()];
    }
    let total = lines.len();
    let mut out: Vec<String> = lines
        .iter()
        .take(MAX_LINES)
        .map(|l| {
            if l.chars().count() > MAX_CHARS {
                let cut: String = l.chars().take(MAX_CHARS).collect();
                format!("{cut}…")
            } else {
                (*l).to_string()
            }
        })
        .collect();
    if total > MAX_LINES {
        out.push(format!("… (+{} more lines)", total - MAX_LINES));
    }
    out
}

fn wrap_rows(rows: Vec<Line<'static>>, width: usize) -> Vec<Line<'static>> {
    let mut out: Vec<Line<'static>> = Vec::new();
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
    for span in line.spans {
        let style = span.style;
        let content: String = span.content.into_owned();
        let mut seg = String::new();
        let mut segw = 0usize;
        for ch in content.chars() {
            let w = UnicodeWidthChar::width(ch).unwrap_or(0);
            if segw + w > width && !seg.is_empty() {
                current.push(Span::styled(std::mem::take(&mut seg), style));
                out.push(Line::from(std::mem::take(&mut current)));
                segw = 0;
            }
            seg.push(ch);
            segw += w;
        }
        if !seg.is_empty() {
            current.push(Span::styled(seg, style));
        }
    }
    if !current.is_empty() {
        out.push(Line::from(current));
    }
}

fn markdown_to_lines(md: &str, base: Style) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let mut in_code = false;
    let mut code_lines = Vec::new();

    let style = Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::DIM);

    for src in md.lines() {
        let trimmed = src.trim();
        if trimmed.starts_with("```") {
            if in_code {
                flush_code(&mut code_lines, &mut lines, style);
                in_code = false;
            } else {
                in_code = true;
            }
            continue;
        }
        if in_code {
            code_lines.push(Line::raw(src.to_string()));
            continue;
        }

        let line = if let Some(rest) = trimmed.strip_prefix("### ") {
            Line::from(vec![Span::styled(
                rest.to_string(),
                base.add_modifier(Modifier::BOLD),
            )])
        } else if let Some(rest) = trimmed.strip_prefix("## ") {
            Line::from(vec![Span::styled(
                rest.to_string(),
                base.add_modifier(Modifier::BOLD),
            )])
        } else if let Some(rest) = trimmed.strip_prefix("# ") {
            Line::from(vec![Span::styled(
                rest.to_string(),
                base.add_modifier(Modifier::BOLD),
            )])
        } else if let Some(rest) = trimmed.strip_prefix("> ") {
            Line::from(vec![Span::styled(
                format!("▍{rest}"),
                base.add_modifier(Modifier::ITALIC),
            )])
        } else if trimmed.chars().all(|c| c == '-' || c == '*' || c == '=') && !trimmed.is_empty() {
            Line::from(Span::styled("──────", Style::default().fg(Color::DarkGray)))
        } else {
            markdown_inline(src, base)
        };
        lines.push(line);
    }
    flush_code(&mut code_lines, &mut lines, style);

    while lines
        .last()
        .map(|l| l.spans.iter().all(|s| s.content.is_empty()))
        .unwrap_or(false)
    {
        lines.pop();
    }
    lines
}

fn flush_code(code_lines: &mut Vec<Line<'static>>, lines: &mut Vec<Line<'static>>, style: Style) {
    if code_lines.is_empty() {
        return;
    }
    let styled: Vec<Line> = code_lines.drain(..).map(|l| l.patch_style(style)).collect();
    lines.extend(styled);
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
        if rest.starts_with('`')
            && let Some(i) = rest[1..].find('`')
        {
            let end = i + 1;
            spans.push(Span::styled(
                rest[1..end].to_string(),
                Style::default().fg(Color::Cyan),
            ));
            rest = &rest[end + 1..];
            continue;
        }
        if rest.starts_with("**")
            && let Some(i) = rest[2..].find("**")
        {
            let end = i + 2;
            spans.push(Span::styled(
                rest[2..end].to_string(),
                base.add_modifier(Modifier::BOLD),
            ));
            rest = &rest[end + 2..];
            continue;
        }
        if rest.starts_with('*')
            && let Some(i) = rest[1..].find('*')
        {
            let end = i + 1;
            spans.push(Span::styled(
                rest[1..end].to_string(),
                base.add_modifier(Modifier::ITALIC),
            ));
            rest = &rest[end + 1..];
            continue;
        }

        let next_bare = rest.find('`').unwrap_or(rest.len());
        let next_star = rest.find('*').unwrap_or(rest.len());
        let next = next_bare.min(next_star);
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
