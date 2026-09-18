use crate::tui::app::{App, HELP_TEXT, MsgRole};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap};
use unicode_width::UnicodeWidthChar;

// ─── Palette ──────────────────────────────────────────────────────────────────
//
//  Design principles:
//  • Base background: #0f0f17 (very dark blue-black)
//  • Surface elevated: #16161f → #1e1e2a → #252535
//  • Text contrast: primary ≥7:1, secondary ≥4.5:1, muted ≥3:1 on base
//  • Accent: blue-500 (#4d9de0), green-400 (#4ade80), amber-400 (#fbbf24)
//  • All "hint" text must be ≥3:1 contrast — minimum readable gray is #6b7280

// ── Backgrounds (darkest → lightest) ─────────────────────────────────────────
const C_BASE:     Color = Color::Rgb(12,  12,  20 ); // terminal fill
const C_SURFACE:  Color = Color::Rgb(16,  16,  26 ); // chat body bg
const C_SURFACE2: Color = Color::Rgb(22,  22,  34 ); // elevated card / modal
const C_SURFACE3: Color = Color::Rgb(30,  32,  48 ); // input box — visibly lifted

// ── Borders ───────────────────────────────────────────────────────────────────
const C_BORDER:        Color = Color::Rgb(52,  58,  82 ); // default border
const C_BORDER_BRIGHT: Color = Color::Rgb(72,  82, 120 ); // hover / focus border

// ── Header ────────────────────────────────────────────────────────────────────
const C_HDR_BG:   Color = Color::Rgb(10,  10,  18 ); // darkest strip
const C_HDR_LOGO: Color = Color::Rgb(99, 168, 249 ); // brand blue (vibrant)
const C_HDR_SEP:  Color = Color::Rgb(38,  44,  66 ); // separator line
const C_HDR_VAL:  Color = Color::Rgb(158, 172, 210); // provider/model text — good contrast
const C_HDR_META: Color = Color::Rgb(108, 118, 158); // cwd/path — readable muted

// ── User messages ─────────────────────────────────────────────────────────────
const C_USER_MARK: Color = Color::Rgb(74,  222, 128); // green-400
const C_USER_TEXT: Color = Color::Rgb(230, 236, 250); // near-white

// ── Agent messages ────────────────────────────────────────────────────────────
const C_AGENT_MARK: Color = Color::Rgb(99,  168, 249); // blue-400
const C_AGENT_TEXT: Color = Color::Rgb(205, 222, 255); // light periwinkle

// ── Tool calls ────────────────────────────────────────────────────────────────
const C_TOOL_MARK: Color = Color::Rgb(251, 191,  36 ); // amber-400
const C_TOOL_TEXT: Color = Color::Rgb(180, 160,  90 ); // readable amber-muted

// ── Errors ────────────────────────────────────────────────────────────────────
const C_ERROR_MARK: Color = Color::Rgb(248,  90,  90 ); // red-500
const C_ERROR_TEXT: Color = Color::Rgb(255, 160, 160 ); // light red

// ── System / meta ─────────────────────────────────────────────────────────────
const C_SYS_TEXT:  Color = Color::Rgb(110, 120, 155); // readable muted — ≥3:1

// ── Plan ──────────────────────────────────────────────────────────────────────
const C_PLAN_MARK: Color = Color::Rgb(251, 191,  36 ); // amber-400
const C_PLAN_TEXT: Color = Color::Rgb(230, 210, 120 ); // warm yellow — readable

// ── Code blocks ──────────────────────────────────────────────────────────────
const C_CODE_TEXT: Color = Color::Rgb(134, 198, 134 ); // muted green — readable

// ── Input box ─────────────────────────────────────────────────────────────────
const C_INPUT_BORDER: Color = Color::Rgb(52,  62,  95 ); // resting border
const C_INPUT_ACTIVE: Color = Color::Rgb(78,  130, 210); // focused / has-text border + send btn
const C_PROMPT:       Color = Color::Rgb(74,  222, 128); // green prompt glyph
const C_CURSOR:       Color = Color::Rgb(74,  222, 128); // green cursor

// ── Hints & placeholders ──────────────────────────────────────────────────────
//  Must be ≥3:1 on C_SURFACE (#101018) → minimum ~#6b7080
const C_HINT:        Color = Color::Rgb(100, 110, 145); // was 58,64,88 — now readable
const C_PLACEHOLDER: Color = Color::Rgb( 88,  96, 130); // was 68,76,104 — lifted

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

/// Screen split:
///   header  = 2 rows  (logo bar + separator line)
///   input   = 4 rows  (╭─╮ border box 3 rows + 1 hint line)
///   body    = rest
fn main_areas(area: Rect, with_input: bool) -> [Rect; 3] {
    let input_h: u16 = if with_input { 4 } else { 0 };
    let [header, rest] =
        Layout::vertical([Constraint::Length(2), Constraint::Min(0)]).areas(area);
    let [body, input] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(input_h)]).areas(rest);
    [header, body, input]
}

/// Inset a rect by `h` columns on each side, clamped.
#[allow(dead_code)]
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
        Block::default().style(Style::default().bg(C_BASE)),
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
//  Full-width navbar, 2 rows:
//
//  Row 0:  ████████████████████████████████████████████████████████████████
//          ◈ vibectl   │   provider · model   │   ~/cwd        [status]
//          ████████████████████████████████████████████████████████████████
//  Row 1:  ────────────────────────────────── (separator, full width)

fn render_header(frame: &mut Frame, area: Rect, app: &App) {
    if area.height < 1 {
        return;
    }

    let w = area.width;

    // ── Row 0: solid navbar bar ───────────────────────────────────────────────
    let nav_row = Rect { height: 1, ..area };

    // Fill background
    frame.render_widget(
        Block::default().style(Style::default().bg(C_HDR_BG)),
        nav_row,
    );

    // ── LEFT: logo pill ───────────────────────────────────────────────────────
    //  "  ◈ vibectl  │  "
    let logo_spans = vec![
        Span::raw("  "),
        Span::styled(
            "◈",
            Style::default()
                .fg(C_HDR_LOGO)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(
            "vibectl",
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("  │  ", Style::default().fg(C_HDR_SEP)),
    ];
    let logo_w: u16 = logo_spans.iter().map(|s| s.content.chars().count() as u16).sum();

    // ── RIGHT: status pill ────────────────────────────────────────────────────
    //  idle:   "  ? help  "
    //  busy:   "  ⠹ toolname  "  (amber)
    //  scroll: "  ↑12  "
    let (right_spans, right_w) = build_right_pill(app);

    // ── CENTER: provider · model · cwd (fills between logo and right) ─────────
    let center_w = w.saturating_sub(logo_w + right_w + 2);
    let center_x = area.x + logo_w;

    // build center content — truncate cwd to fit
    let provider = &app.session.provider_label;
    let model    = &app.session.agent.model;
    let cwd_full = app.session.cwd.display().to_string();
    let cwd_short = short_cwd(&cwd_full, (center_w as usize).saturating_sub(20));

    let center_content = vec![
        Span::styled(provider.clone(), Style::default().fg(C_HDR_VAL)),
        Span::styled("  ·  ", Style::default().fg(C_HDR_SEP)),
        Span::styled(model.clone(), Style::default().fg(C_HDR_VAL)),
        Span::styled("  │  ", Style::default().fg(C_HDR_SEP)),
        Span::styled(cwd_short, Style::default().fg(C_HDR_META)),
    ];
    let content_w: u16 = center_content.iter().map(|s| s.content.chars().count() as u16).sum();

    // pad center so it's truly centered
    let left_pad = center_w.saturating_sub(content_w) / 2;

    let mut center_spans = vec![Span::raw(" ".repeat(left_pad as usize))];
    center_spans.extend(center_content);

    // ── Render logo (left) ────────────────────────────────────────────────────
    let logo_rect = Rect { x: area.x, y: area.y, width: logo_w, height: 1 };
    frame.render_widget(
        Paragraph::new(Line::from(logo_spans)).style(Style::default().bg(C_HDR_BG)),
        logo_rect,
    );

    // ── Render center ─────────────────────────────────────────────────────────
    let center_rect = Rect { x: center_x, y: area.y, width: center_w, height: 1 };
    frame.render_widget(
        Paragraph::new(Line::from(center_spans)).style(Style::default().bg(C_HDR_BG)),
        center_rect,
    );

    // ── Render right pill ─────────────────────────────────────────────────────
    let right_x = area.x + w.saturating_sub(right_w);
    let right_rect = Rect { x: right_x, y: area.y, width: right_w, height: 1 };
    frame.render_widget(
        Paragraph::new(Line::from(right_spans)).style(Style::default().bg(C_HDR_BG)),
        right_rect,
    );

    // ── Row 1: separator line ─────────────────────────────────────────────────
    if area.height >= 2 {
        let sep_row = Rect { y: area.y + 1, height: 1, ..area };
        // gradient-ish: brighter under logo, dims toward right
        let sep_char = "─";
        let line = sep_char.repeat(w as usize);
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                line,
                Style::default().fg(C_BORDER),
            )))
            .style(Style::default().bg(C_HDR_BG)),
            sep_row,
        );
    }
}

fn build_right_pill(app: &App) -> (Vec<Span<'static>>, u16) {
    // always starts with a │ divider
    let mut spans: Vec<Span> = vec![
        Span::styled("  │  ", Style::default().fg(Color::Rgb(38, 44, 66))),
    ];

    if app.busy {
        let sp = spinner_char(app);
        let label = if let Some(tool) = &app.running_tool {
            format!("{sp}  {}", tool)
        } else {
            format!("{sp}  thinking…")
        };
        spans.push(Span::styled(label, Style::default().fg(C_TOOL_MARK)));
    } else {
        if app.scroll_offset > 0 {
            spans.push(Span::styled(
                format!("↑{}  ", app.scroll_offset),
                Style::default().fg(C_AGENT_MARK),
            ));
        }
        spans.push(Span::styled("?  help", Style::default().fg(C_HINT)));
    }
    spans.push(Span::raw("  "));

    let total_w: u16 = spans.iter().map(|s| s.content.chars().count() as u16).sum();
    (spans, total_w)
}

fn short_cwd(cwd: &str, max_w: usize) -> String {
    // Show ~/last/two/components, truncated if needed
    let home = std::env::var("HOME").unwrap_or_default();
    let display = if !home.is_empty() && cwd.starts_with(&home) {
        format!("~{}", &cwd[home.len()..])
    } else {
        cwd.to_string()
    };

    if display.chars().count() <= max_w {
        return display;
    }

    // Take last N chars with leading "…"
    let take = max_w.saturating_sub(1);
    let chars: Vec<char> = display.chars().collect();
    let start = chars.len().saturating_sub(take);
    format!("…{}", chars[start..].iter().collect::<String>())
}

// ─── Chat body ────────────────────────────────────────────────────────────────

fn render_body(frame: &mut Frame, area: Rect, app: &App) {
    // No box border — clean open chat area with only a bottom separator line
    // so it visually connects with the input box below.
    frame.render_widget(
        Block::default().style(Style::default().bg(C_SURFACE)),
        area,
    );

    // padding: 3 cols horizontal, 1 row vertical — gives breathing room
    let padded = area.inner(Margin { horizontal: 3, vertical: 1 });
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
    // ── Pixel-accurate reference layout ──────────────────────────────────────
    //
    //  area = 4 rows, full terminal width
    //
    //  row 0: ╭──────────────────────────────────────────────────────────────╮
    //  row 1: │  ❯  Ask vibectl...▋                        ⊘   [/]   [↵]  │
    //  row 2: ╰──────────────────────────────────────────────────────────────╯
    //  row 3:    /help  ·  Ctrl+K  ·  ↑↓  history
    //
    //  The box spans rows 0-2 (3 rows).
    //  Hint bar is row 3, same x as box content.
    //  Horizontal margin: 2 cols each side from terminal edge.

    if area.height < 4 || area.width < 20 {
        return;
    }

    let margin: u16 = 2;
    let box_rect = Rect {
        x: area.x + margin,
        y: area.y,
        width: area.width.saturating_sub(margin * 2),
        height: 3,
    };
    let hint_rect = Rect {
        x: area.x + margin,
        y: area.y + 3,
        width: area.width.saturating_sub(margin * 2),
        height: 1,
    };

    // ── Border color: dim when idle/empty, bright when typing, muted when busy
    let border_color = if app.busy {
        C_INPUT_BORDER
    } else if app.input.is_empty() {
        C_INPUT_BORDER
    } else {
        C_INPUT_ACTIVE
    };

    // Draw the rounded box
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border_color))
        .style(Style::default().bg(C_SURFACE3));

    let inner = block.inner(box_rect); // 1 row tall, inset by 1 each side
    frame.render_widget(block, box_rect);

    if inner.width < 12 {
        return;
    }

    // ── Split inner row into: [text_area] [icons_area] ────────────────────────
    //
    //  icons:  "⊘ " (2) + " [/] " (5) + " [↵]" (4) = 11 chars + 1 pad = 12
    //  We reserve 12 cols for icons on the right.
    let icons_w: u16 = 12;
    let text_w   = inner.width.saturating_sub(icons_w);

    let text_area  = Rect { width: text_w,   ..inner };
    let icons_area = Rect { x: inner.x + text_w, width: icons_w, ..inner };

    // ── Build text content ────────────────────────────────────────────────────
    let (prompt_ch, prompt_col) = if app.busy { ("·", C_HINT) } else { ("❯", C_PROMPT) };

    let mut spans: Vec<Span> = vec![
        Span::styled(format!("{prompt_ch} "), Style::default().fg(prompt_col).add_modifier(Modifier::BOLD)),
    ];

    let input    = &app.input;
    let cursor   = app.cursor;
    let _char_len = input.chars().count();
    let show_cur = cursor_visible(app);
    // visible window: strip newlines for single-line display
    let flat_input: String = input.chars().map(|c| if c == '\n' { ' ' } else { c }).collect();
    let flat_cursor = cursor; // cursor position is same in flat view

    if flat_input.is_empty() {
        spans.push(Span::styled("Ask vibectl…", Style::default().fg(C_PLACEHOLDER)));
        if show_cur && !app.busy {
            spans.push(Span::styled("▋", Style::default().fg(C_CURSOR)));
        }
    } else {
        // viewport scroll: only show chars that fit
        let prompt_w: usize = 2; // "❯ "
        let avail = (text_w as usize).saturating_sub(prompt_w);
        let total_chars = flat_input.chars().count();

        // scroll right so cursor is always visible
        let win_end   = (flat_cursor + 1).min(total_chars);
        let win_start = win_end.saturating_sub(avail);

        let visible: String = flat_input.chars().skip(win_start).take(avail).collect();
        let vis_cursor = flat_cursor.saturating_sub(win_start); // cursor in visible window

        let _vis_len = visible.chars().count();

        let before: String = visible.chars().take(vis_cursor).collect();
        let at_char: Option<String> = visible.chars().nth(vis_cursor).map(|c| c.to_string());
        let after:   String = visible.chars().skip(vis_cursor + 1).collect();

        if !before.is_empty() {
            spans.push(Span::styled(before, Style::default().fg(C_USER_TEXT)));
        }

        if app.busy {
            spans.push(Span::styled(visible, Style::default().fg(C_HDR_META)));
        } else if let Some(at) = at_char {
            if show_cur {
                spans.push(Span::styled(at, Style::default().bg(C_CURSOR).fg(C_SURFACE)));
            } else {
                spans.push(Span::styled(at, Style::default().fg(C_USER_TEXT)));
            }
            if !after.is_empty() {
                spans.push(Span::styled(after, Style::default().fg(C_USER_TEXT)));
            }
        } else {
            // cursor past end
            if show_cur {
                spans.push(Span::styled("▋", Style::default().fg(C_CURSOR)));
            }
        }

        // show "…" prefix if scrolled
        if win_start > 0 {
            if let Some(first) = spans.get_mut(1) {
                let content = format!("…{}", first.content);
                first.content = content.into();
            }
        }
    }

    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::default().bg(C_SURFACE3)),
        text_area,
    );

    // ── Action icons: ⊘   [/]   [↵] ─────────────────────────────────────────
    //  ⊘ = attachment (always dim)
    //  [/] = command palette (always dim)
    //  [↵] = send — blue bg when has text & not busy, dim otherwise
    let has_text  = !app.input.is_empty();
    let send_active = has_text && !app.busy;
    let send_bg   = if send_active { C_INPUT_ACTIVE } else { C_SURFACE3 };
    let send_fg   = if send_active { Color::White     } else { C_BORDER  };

    let icon_line = Line::from(vec![
        Span::styled("⊘ ", Style::default().fg(C_HINT)),
        Span::styled(" [/] ", Style::default().fg(C_HINT)),
        Span::styled(
            " [↵] ",
            Style::default()
                .fg(send_fg)
                .bg(send_bg)
                .add_modifier(Modifier::BOLD),
        ),
    ]);

    frame.render_widget(
        Paragraph::new(icon_line).style(Style::default().bg(C_SURFACE3)),
        icons_area,
    );

    // ── Hint bar (row 3, outside box) ─────────────────────────────────────────
    let hint_line: Line = if app.busy {
        Line::from(vec![
            Span::styled(
                format!("  {} working… ", spinner_char(app)),
                Style::default().fg(C_TOOL_MARK),
            ),
            Span::styled("Ctrl+C ", Style::default().fg(C_ERROR_MARK)),
            Span::styled("to cancel", Style::default().fg(C_HINT)),
        ])
    } else if app.input.chars().any(|c| c == '\n') {
        // multiline state
        Line::from(vec![
            Span::styled("  Ctrl+Enter ", Style::default().fg(C_AGENT_MARK)),
            Span::styled("to send  ·  ", Style::default().fg(C_HINT)),
            Span::styled("Enter ", Style::default().fg(C_HINT)),
            Span::styled("newline", Style::default().fg(C_HINT)),
        ])
    } else {
        Line::from(vec![
            Span::styled("  /help", Style::default().fg(C_HINT)),
            Span::styled("  ·  ", Style::default().fg(C_BORDER)),
            Span::styled("Ctrl+K", Style::default().fg(C_HINT)),
            Span::styled("  ·  ", Style::default().fg(C_BORDER)),
            Span::styled("↑↓", Style::default().fg(C_HINT)),
            Span::styled("  history", Style::default().fg(C_HINT)),
        ])
    };

    frame.render_widget(
        Paragraph::new(hint_line).style(Style::default().bg(C_BASE)),
        hint_rect,
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
            // extra blank line between major turns (user↔agent) for breathing room
            if !is_tool && !prev_tool {
                rows.push(Line::raw(""));
            }
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
            Span::styled("  ─ ", Style::default().fg(C_BORDER_BRIGHT)),
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
        Style::default().fg(C_BORDER_BRIGHT),
    )));
    for l in buf.drain(..) {
        lines.push(Line::from(vec![
            Span::styled("║ ", Style::default().fg(C_BORDER_BRIGHT)),
            Span::styled(l, Style::default().fg(C_CODE_TEXT)),
        ]));
    }
    lines.push(Line::from(Span::styled(
        "╚══════".to_string(),
        Style::default().fg(C_BORDER_BRIGHT),
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
