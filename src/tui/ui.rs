use crate::tui::app::{App, HELP_TEXT, MsgRole};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

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
    let status = if let Some(tool) = &app.running_tool {
        format!(" ⟳ running tool: {tool}")
    } else if app.busy {
        " ⟳ agent thinking…".to_string()
    } else {
        String::new()
    };
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
                " Approve shell command? [y]es [n]o (esc=cancel)",
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
        Span::styled("⟳", Style::default().fg(Color::Magenta))
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
    if app.show_help {
        let help_lines: Vec<Line> = HELP_TEXT
            .lines()
            .map(|l| {
                Line::from(Span::styled(
                    l,
                    if l.starts_with('═') {
                        Style::default().fg(Color::DarkGray)
                    } else if l.is_empty() {
                        Style::default()
                    } else {
                        Style::default().fg(Color::Cyan)
                    },
                ))
            })
            .collect();
        let p = Paragraph::new(help_lines)
            .scroll((app.scroll, 0))
            .block(Block::default().borders(Borders::ALL).title(" Help "));
        frame.render_widget(p, area);
        return;
    }

    let mut lines: Vec<Line> = Vec::new();
    for msg in &app.messages {
        let (label, label_style) = label_for(&msg.role);
        lines.push(Line::from(vec![Span::styled(
            label,
            label_style.add_modifier(Modifier::BOLD),
        )]));
        if let Some(kind) = &msg.kind {
            lines.push(Line::from(Span::styled(
                format!("  {} ", kind),
                Style::default()
                    .fg(Color::Magenta)
                    .add_modifier(Modifier::BOLD),
            )));
        }
        let body_style = match msg.role {
            MsgRole::User => Style::default().fg(Color::White),
            MsgRole::Assistant => Style::default().fg(Color::LightGreen),
            MsgRole::Tool => Style::default().fg(Color::DarkGray),
            MsgRole::Error => Style::default().fg(Color::Red),
            MsgRole::System => Style::default().fg(Color::DarkGray),
            MsgRole::Plan => Style::default().fg(Color::Cyan),
        };
        let mut rendered = markdown_to_lines(&msg.text, body_style);
        lines.append(&mut rendered);
        lines.push(Line::raw(""));
    }

    if lines.is_empty() {
        lines.push(Line::raw(""));
    }

    let max_scroll = lines.len().saturating_sub(area.height as usize);
    let max_scroll = max_scroll.min(u16::MAX as usize) as u16;
    let scroll = app.scroll.clamp(0, max_scroll);

    let p = Paragraph::new(lines)
        .scroll((scroll, 0))
        .wrap(Wrap { trim: true });
    frame.render_widget(p, area);
}

fn label_for(role: &MsgRole) -> (String, Style) {
    match role {
        MsgRole::User => ("You".to_string(), Style::default().fg(Color::Green)),
        MsgRole::Assistant => ("Agent".to_string(), Style::default().fg(Color::Cyan)),
        MsgRole::Tool => ("Tool".to_string(), Style::default().fg(Color::Magenta)),
        MsgRole::Error => ("Error".to_string(), Style::default().fg(Color::Red)),
        MsgRole::System => ("System".to_string(), Style::default().fg(Color::DarkGray)),
        MsgRole::Plan => ("Plan".to_string(), Style::default().fg(Color::Yellow)),
    }
}

fn markdown_to_lines(md: &str, base: Style) -> Vec<Line<'_>> {
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

fn flush_code<'a>(code_lines: &mut Vec<Line<'a>>, lines: &mut Vec<Line<'a>>, style: Style) {
    if code_lines.is_empty() {
        return;
    }
    let styled: Vec<Line> = code_lines.drain(..).map(|l| l.patch_style(style)).collect();
    lines.extend(styled);
    lines.push(Line::raw(""));
}

fn markdown_inline(src: &str, base: Style) -> Line<'_> {
    let spans = parse_inline(src, base);
    if spans.is_empty() {
        Line::raw(src)
    } else {
        Line::from(spans)
    }
}

fn parse_inline(src: &str, base: Style) -> Vec<Span<'_>> {
    let mut spans = Vec::new();
    let mut rest = src;
    while !rest.is_empty() {
        if rest.starts_with('`')
            && let Some(i) = rest[1..].find('`')
        {
            let end = i + 1;
            spans.push(Span::styled(
                &rest[1..end],
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
                &rest[2..end],
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
                &rest[1..end],
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
