use std::io::{self, Stdout};

use anyhow::Result;
use crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Frame;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph};

use crate::tui::app::{self, App, BrowserEntryKind, DiffRowKind, Focus, ReaderOrigin, View};
use crate::tui::links::LinkTarget;

pub type Term = Terminal<CrosstermBackend<Stdout>>;

pub fn setup_terminal() -> Result<Term> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let term = Terminal::new(backend)?;
    Ok(term)
}

pub fn restore_terminal(term: &mut Term) -> Result<()> {
    disable_raw_mode()?;
    execute!(
        term.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    term.show_cursor()?;
    Ok(())
}

/// Restore raw/alt screen state for use in a panic hook (best-effort).
pub fn restore_raw() -> Result<()> {
    let _ = disable_raw_mode();
    let mut out = io::stdout();
    let _ = execute!(out, LeaveAlternateScreen, DisableMouseCapture);
    Ok(())
}

pub fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),    // body (full screen except for one row)
            Constraint::Length(1), // statusline
        ])
        .split(area);

    let body = chunks[0];
    let status = chunks[1];
    app.viewport = body;
    app.statusline_area = status;

    // The conflict resolver is a full-screen modal — draw it over everything
    // and skip the normal reader/browser pipeline.
    if app.conflict.is_some() {
        draw_conflict(f, app, area);
        return;
    }

    // Reserve 1 column on the right for the reader scrollbar so layout stays
    // stable whether or not content overflows. Browser ignores this width.
    // In split-edit mode the preview pane is narrower than the body, so
    // re-target the wrap width to whichever pane will actually display the
    // rendered lines — otherwise the preview overflows on the right.
    let render_width = match &app.view {
        View::Reader(r) if r.in_split_edit() => {
            if body.width >= 100 {
                // Horizontal split: preview is the right half minus separator.
                body.width.saturating_sub(body.width / 2 + 1)
            } else {
                // Vertical stack: preview spans full body width.
                body.width
            }
        }
        _ => body.width.saturating_sub(1),
    };
    app.ensure_rendered(render_width);

    if app.git_lens.is_some() && matches!(app.view, View::Reader(_)) {
        draw_git_lens(f, app, body);
    } else {
        match &app.view {
            View::Reader(r) => {
                if r.in_split_edit() {
                    draw_edit_split(f, app, body);
                } else {
                    draw_reader(f, app, body);
                }
            }
            View::Browser(_) => draw_browser(f, app, body),
            View::Cloud(_) => draw_cloud_browser(f, app, body),
        }
        if matches!(app.view, View::Reader(_)) {
            // Image rendering is read-only; in split-edit mode we still
            // show images in the preview pane area but the layout below
            // wires its own rect, so the global overlay is suppressed.
            let in_split = matches!(&app.view, View::Reader(r) if r.in_split_edit());
            if !in_split {
                draw_images_overlay(f, app, body);
            }
        }
    }

    draw_statusline(f, app, status);

    if app.search.is_some() {
        draw_search(f, app, area);
    }

    if app.prompt.is_some() {
        draw_prompt(f, app, area);
    }

    if app.toc.is_some() {
        draw_toc(f, app, area);
    }

    if app.help_open {
        draw_help(f, app, area);
    }
}

/// Centered table-of-contents popup (`t` in the Reader). One row per
/// heading, indented by level; the selection is windowed into view.
fn draw_toc(f: &mut Frame, app: &App, area: Rect) {
    let Some(toc) = &app.toc else {
        return;
    };
    let View::Reader(r) = &app.view else {
        return;
    };
    let Some(rendered) = r.rendered.as_ref() else {
        return;
    };
    let headings = &rendered.headings;
    if headings.is_empty() {
        return;
    }
    let theme = &app.opts.theme;
    let w = 60.min(area.width.saturating_sub(4)).max(20);
    let h = (headings.len() as u16 + 2)
        .min(area.height.saturating_sub(4))
        .max(3);
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    let popup = Rect {
        x,
        y,
        width: w,
        height: h,
    };
    f.render_widget(Clear, popup);
    // Window the list around the selection so it stays visible.
    let visible = h.saturating_sub(2) as usize;
    let start = toc
        .selected
        .saturating_sub(visible.saturating_sub(1) / 2)
        .min(headings.len().saturating_sub(visible.max(1)));
    let mut lines: Vec<Line<'static>> = Vec::new();
    for (i, hd) in headings.iter().enumerate().skip(start).take(visible.max(1)) {
        let indent = "  ".repeat(hd.level.saturating_sub(1) as usize);
        let text = format!(" {}{}", indent, hd.text);
        let style = if i == toc.selected {
            Style::default()
                .fg(theme.heading[0])
                .add_modifier(Modifier::BOLD | Modifier::REVERSED)
        } else {
            Style::default()
        };
        lines.push(Line::from(Span::styled(text, style)));
    }
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Contents ")
        .border_style(Style::default().fg(theme.heading[0]));
    f.render_widget(Paragraph::new(lines).block(block), popup);
}

/// Centered one-line modal prompt (new note title, push title, download
/// filename) or delete confirmation.
fn draw_prompt(f: &mut Frame, app: &App, area: Rect) {
    let Some(p) = &app.prompt else {
        return;
    };
    let theme = &app.opts.theme;
    let w = 64.min(area.width.saturating_sub(4)).max(20);
    let h = 3u16;
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    let popup = Rect {
        x,
        y,
        width: w,
        height: h,
    };
    f.render_widget(Clear, popup);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(p.title.clone())
        .border_style(Style::default().fg(theme.heading[0]));
    let inner = block.inner(popup);
    f.render_widget(block, popup);

    let line = if matches!(p.kind, app::PromptKind::ConfirmDelete { .. }) {
        Line::from(Span::styled(
            "y / Enter to delete — any other key cancels",
            Style::default()
                .fg(theme.heading[3])
                .add_modifier(Modifier::BOLD),
        ))
    } else {
        Line::from(vec![
            Span::styled(
                "▸ ",
                Style::default()
                    .fg(theme.heading[0])
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(p.input.clone()),
            Span::styled("█", Style::default().fg(theme.heading[0])),
        ])
    };
    f.render_widget(Paragraph::new(line), inner);
}

/// Render `p` shortened against the launch root (or `~/`) when one is a prefix.
fn display_path(p: &std::path::Path, root: &std::path::Path) -> String {
    if let Ok(rel) = p.strip_prefix(root) {
        let s = rel.display().to_string();
        if s.is_empty() {
            return ".".to_string();
        }
        return s;
    }
    if let Some(home) = dirs::home_dir() {
        if let Ok(rel) = p.strip_prefix(&home) {
            return format!("~/{}", rel.display());
        }
    }
    p.display().to_string()
}

fn draw_reader(f: &mut Frame, app: &mut App, area: Rect) {
    let View::Reader(r) = &app.view else {
        return;
    };
    let Some(rendered) = &r.rendered else {
        return;
    };
    let theme = &app.opts.theme;

    let total = rendered.lines.len();
    let scroll = (r.scroll as usize).min(total.saturating_sub(1));
    let visible_h = area.height as usize;

    // Mark the file read once its last line is on screen. Documents shorter
    // than the viewport satisfy this on open (nothing to scroll = read); longer
    // ones require scrolling to the end. `mark_read` is idempotent, so calling
    // it every frame while parked at the bottom is cheap. Not in edit mode —
    // that's a separate draw path.
    let mark_read_path = if r.edit.is_none() && r.scroll as usize + visible_h >= total {
        match &r.origin {
            ReaderOrigin::File(p) => Some(p.clone()),
            ReaderOrigin::Stdin | ReaderOrigin::CloudNote { .. } => None,
        }
    } else {
        None
    };

    let line_num_w = if app.opts.line_numbers {
        format!("{}", total).len() as u16 + 1
    } else {
        0
    };
    let scrollbar_w: u16 = 1;
    let line_num_area = Rect {
        x: area.x,
        y: area.y,
        width: line_num_w,
        height: area.height,
    };
    let body_area = Rect {
        x: area.x + line_num_w,
        y: area.y,
        width: area
            .width
            .saturating_sub(line_num_w)
            .saturating_sub(scrollbar_w),
        height: area.height,
    };
    let scrollbar_area = Rect {
        x: area.x + area.width.saturating_sub(scrollbar_w),
        y: area.y,
        width: scrollbar_w,
        height: area.height,
    };

    let mut display_lines: Vec<Line> = Vec::with_capacity(visible_h);
    let mut nums: Vec<Line> = Vec::with_capacity(visible_h);
    // Suppress the keyboard-focus highlight while the user's been driving
    // with the mouse — two simultaneous "cursors" was the user's complaint.
    let show_focus = !app.mouse_recent;
    for i in 0..visible_h {
        let idx = scroll + i;
        if idx >= total {
            break;
        }
        let mut line = rendered.lines[idx].clone();
        if show_focus {
            match r.focus {
                Some(Focus::Link(fi)) => {
                    if let Some(link) = rendered.link_map.links.get(fi) {
                        if link.line == idx {
                            highlight_focused(&mut line, link, theme);
                        }
                    }
                }
                Some(Focus::Checkbox(ci)) => {
                    if let Some(cb) = rendered.checkbox_map.items.get(ci) {
                        if cb.line == idx {
                            highlight_checkbox_hover(&mut line, cb.col_start, cb.col_end);
                        }
                    }
                }
                None => {}
            }
        }
        if let Some(hi) = r.hover_link {
            if let Some(link) = rendered.link_map.links.get(hi) {
                if link.line == idx {
                    highlight_focused(&mut line, link, theme);
                }
            }
        }
        if let Some(ci) = r.hover_checkbox {
            if let Some(cb) = rendered.checkbox_map.items.get(ci) {
                if cb.line == idx {
                    highlight_checkbox_hover(&mut line, cb.col_start, cb.col_end);
                }
            }
        }
        if let Some(bi) = r.hover_jsonl {
            if let Some(btn) = r.jsonl_overlay.as_ref().and_then(|o| o.buttons.get(bi)) {
                if btn.line == idx {
                    highlight_checkbox_hover(&mut line, btn.col_start, btn.col_end);
                }
            }
        }
        if let Some(s) = r.doc_search.as_ref() {
            for (mi, m) in s.matches.iter().enumerate() {
                if m.line == idx {
                    let is_current = !s.editing && mi == s.current;
                    highlight_doc_match(&mut line, m.col_start, m.col_end, is_current, theme);
                }
            }
        }
        display_lines.push(line);
        if app.opts.line_numbers {
            nums.push(Line::from(Span::styled(
                format!(
                    "{:>width$} ",
                    idx + 1,
                    width = (line_num_w as usize).saturating_sub(1)
                ),
                Style::default().fg(theme.muted),
            )));
        }
    }

    if app.opts.line_numbers {
        f.render_widget(Paragraph::new(nums), line_num_area);
    }
    f.render_widget(Paragraph::new(display_lines), body_area);
    draw_scrollbar(f, scrollbar_area, scroll, total, visible_h, theme);

    // Drag-select overlay: paint reverse-video over the selection range.
    // Drawn before the cursor so the cursor halo wins on top.
    if let Some(sel) = app.selection.filter(|s| s.is_active()) {
        let ((s_line, s_col), (e_line, e_col)) = sel.normalized();
        let buf = f.buffer_mut();
        for li in s_line..=e_line {
            let cy_view = li as i32 - r.scroll as i32;
            if cy_view < 0 || cy_view as u16 >= body_area.height {
                continue;
            }
            let row = body_area.y + cy_view as u16;
            let from = if li == s_line { s_col as usize } else { 0 };
            let to = if li == e_line {
                e_col as usize
            } else {
                body_area.width as usize
            };
            for c in from..to {
                let x = body_area.x + c as u16;
                if x >= body_area.x + body_area.width {
                    break;
                }
                let cell = &mut buf[(x, row)];
                cell.set_style(cell.style().add_modifier(Modifier::REVERSED));
            }
        }
    }

    // Edit mode: paint the source cursor as a reverse-video cell on top of
    // the body. Doing this *after* rendering the paragraph keeps the cursor
    // visible regardless of the underlying span's foreground/background.
    if let Some((cx, cy)) = rendered.cursor_xy {
        let cy_view = cy as i32 - r.scroll as i32;
        if cy_view >= 0 && (cy_view as u16) < body_area.height {
            let row = body_area.y + cy_view as u16;
            let col = body_area.x + cx;
            if col < body_area.x + body_area.width {
                let buf = f.buffer_mut();
                let cell = &mut buf[(col, row)];
                // Carry the existing cell's char so we don't overwrite the
                // glyph the cursor is "on" — we just invert it.
                cell.set_style(Style::default().add_modifier(Modifier::REVERSED));
            }
        }
    }

    // Last line is visible — record this file as read. Done after all `r`
    // borrows above are out of scope so the disjoint `&mut app.read_state`
    // borrow is clean.
    if let Some(path) = mark_read_path {
        app.read_state.mark_read(&path);
    }
}

/// Render split-screen edit mode: raw editor on the left (or top on
/// narrow terminals), rendered preview on the right (or bottom). Both
/// panes get a labeled header row; the focused pane (always the raw
/// editor for now since that's where the cursor lives) is highlighted.
fn draw_edit_split(f: &mut Frame, app: &mut App, area: Rect) {
    let View::Reader(r) = &app.view else {
        return;
    };
    let theme = &app.opts.theme;
    let Some(rendered) = r.rendered.as_ref() else {
        return;
    };

    // Layout choice: side-by-side at >= 100 cols, vertical stack below.
    let horizontal = area.width >= 100;
    let (raw_area, preview_area, _split_dir) = if horizontal {
        let half = area.width / 2;
        let raw = Rect {
            x: area.x,
            y: area.y,
            width: half,
            height: area.height,
        };
        let prev = Rect {
            x: area.x + half + 1,
            y: area.y,
            width: area.width.saturating_sub(half + 1),
            height: area.height,
        };
        // Draw the vertical separator column.
        let sep = Rect {
            x: area.x + half,
            y: area.y,
            width: 1,
            height: area.height,
        };
        let sep_style = Style::default().fg(theme.muted);
        let sep_lines: Vec<Line> = (0..sep.height)
            .map(|_| Line::from(Span::styled("│", sep_style)))
            .collect();
        f.render_widget(Paragraph::new(sep_lines), sep);
        (raw, prev, "h")
    } else {
        let half = area.height / 2;
        let raw = Rect {
            x: area.x,
            y: area.y,
            width: area.width,
            height: half,
        };
        let prev = Rect {
            x: area.x,
            y: area.y + half + 1,
            width: area.width,
            height: area.height.saturating_sub(half + 1),
        };
        let sep = Rect {
            x: area.x,
            y: area.y + half,
            width: area.width,
            height: 1,
        };
        let sep_style = Style::default().fg(theme.muted);
        let bar = "─".repeat(sep.width as usize);
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(bar, sep_style))),
            sep,
        );
        (raw, prev, "v")
    };

    app.edit_raw_area = raw_area;
    app.edit_preview_area = preview_area;

    // ---- Raw pane ----
    let raw_rows = app::render_raw_pane(&r.raw, raw_area.width as usize);
    let cursor = r.edit.as_ref().map(|e| e.cursor).unwrap_or(0);
    let cur_row_idx = app::raw_row_for_cursor(&raw_rows, cursor);
    let cur_col = if let Some(row) = raw_rows.get(cur_row_idx) {
        app::raw_col_for_cursor(&r.raw, row, cursor)
    } else {
        0
    };

    // Auto-scroll raw pane so cursor is visible — but only when the cursor
    // *moved* since the last frame (typing, arrow keys, click). Wheel
    // scrolling doesn't touch the cursor, so without this gate every frame
    // would snap the scroll back and the wheel would feel inert.
    let visible_h_raw = raw_area.height as usize;
    let mut raw_scroll = r.scroll as usize;
    let cursor_changed = r
        .edit
        .as_ref()
        .map(|e| e.last_drawn_cursor != Some(cursor))
        .unwrap_or(false);
    if cursor_changed {
        if cur_row_idx < raw_scroll {
            raw_scroll = cur_row_idx;
        }
        if visible_h_raw > 0 && cur_row_idx >= raw_scroll + visible_h_raw {
            raw_scroll = cur_row_idx + 1 - visible_h_raw;
        }
    }
    let max_raw_scroll = raw_rows.len().saturating_sub(visible_h_raw);
    if raw_scroll > max_raw_scroll {
        raw_scroll = max_raw_scroll;
    }

    let mut raw_lines: Vec<Line> = Vec::with_capacity(visible_h_raw);
    for i in 0..visible_h_raw {
        let idx = raw_scroll + i;
        if idx >= raw_rows.len() {
            break;
        }
        let row = &raw_rows[idx];
        // Kind is computed per source line so wrapped continuation rows
        // keep the same styling as their head row.
        let style = match row.kind {
            app::RawRowKind::Heading => Style::default()
                .fg(theme.heading[0])
                .add_modifier(Modifier::BOLD),
            app::RawRowKind::Quote => Style::default().fg(theme.quote),
            app::RawRowKind::Normal => Style::default(),
        };
        raw_lines.push(Line::from(Span::styled(row.text.clone(), style)));
    }
    f.render_widget(Paragraph::new(raw_lines), raw_area);

    // Drag-selection highlight: reverse-video over the selected byte range,
    // row by row (rows wholly outside the range are skipped).
    if let Some(sel) = r
        .edit
        .as_ref()
        .and_then(|e| e.selection.as_ref())
        .filter(|s| s.is_active())
    {
        let (sel_start, sel_end) = sel.range();
        let buf = f.buffer_mut();
        for i in 0..visible_h_raw {
            let Some(row) = raw_rows.get(raw_scroll + i) else {
                break;
            };
            let from = row.source_range.start.max(sel_start);
            let to = row.source_range.end.min(sel_end);
            if from >= to {
                continue;
            }
            let c0 = app::raw_col_for_cursor(&r.raw, row, from);
            let c1 = app::raw_col_for_cursor(&r.raw, row, to);
            let y = raw_area.y + i as u16;
            for x in c0..c1 {
                let col = raw_area.x + x;
                if col < raw_area.x + raw_area.width {
                    let cell = &mut buf[(col, y)];
                    cell.set_style(cell.style().add_modifier(Modifier::REVERSED));
                }
            }
        }
    }

    // Cursor: reverse-video cell at (cur_col, cur_row_idx - raw_scroll).
    let cy_view = cur_row_idx as i32 - raw_scroll as i32;
    if cy_view >= 0 && (cy_view as u16) < raw_area.height {
        let row = raw_area.y + cy_view as u16;
        let col = raw_area.x + cur_col;
        if col < raw_area.x + raw_area.width {
            let buf = f.buffer_mut();
            let cell = &mut buf[(col, row)];
            cell.set_style(Style::default().add_modifier(Modifier::REVERSED));
        }
    }

    // ---- Preview pane ----
    // Sync preview to follow cursor block — but, like the raw pane, only
    // when the cursor moved. Otherwise wheel-driven scrolls (which sync
    // preview via the events layer) would be immediately undone by this
    // snap-back on the next frame.
    let preview_target = app::preview_row_for_source(rendered, cursor);
    let visible_h_prev = preview_area.height as usize;
    let mut prev_scroll = r.preview_scroll as usize;
    if cursor_changed {
        if preview_target < prev_scroll {
            prev_scroll = preview_target;
        }
        if visible_h_prev > 0 && preview_target >= prev_scroll + visible_h_prev {
            prev_scroll = preview_target + 1 - visible_h_prev;
        }
    }
    let max_prev_scroll = rendered.lines.len().saturating_sub(visible_h_prev);
    if prev_scroll > max_prev_scroll {
        prev_scroll = max_prev_scroll;
    }

    let mut prev_lines: Vec<Line> = Vec::with_capacity(visible_h_prev);
    for i in 0..visible_h_prev {
        let idx = prev_scroll + i;
        if idx >= rendered.lines.len() {
            break;
        }
        prev_lines.push(rendered.lines[idx].clone());
    }
    f.render_widget(Paragraph::new(prev_lines), preview_area);

    // Mark the preview row that corresponds to the cursor's block. In the
    // side-by-side layout we stamp the marker onto the center divider column
    // (preview_area.x - 1) so it never steals the row's first glyph; in the
    // vertical stack — which has no left divider beside the preview — we tint
    // the row's background instead, again leaving every character visible.
    let cy_prev = preview_target as i32 - prev_scroll as i32;
    if cy_prev >= 0 && (cy_prev as u16) < preview_area.height && preview_area.width > 0 {
        let row = preview_area.y + cy_prev as u16;
        let buf = f.buffer_mut();
        if horizontal {
            // Use the heavy box-drawing vertical so it sits centered in the
            // cell exactly like the `│` divider it replaces — just bolder and
            // colored to flag the active row.
            let cell = &mut buf[(preview_area.x - 1, row)];
            cell.set_char('┃');
            cell.set_style(Style::default().fg(theme.heading[0]));
        } else {
            let wash = theme.code_bg.unwrap_or(theme.heading[0]);
            for col in preview_area.x..preview_area.x + preview_area.width {
                buf[(col, row)].set_bg(wash);
            }
        }
    }

    // Persist scroll positions back to the reader so wheel handlers can
    // build on them. Record the cursor we just drew so the next frame
    // only re-follows when it actually moved.
    if let View::Reader(r) = &mut app.view {
        r.scroll = raw_scroll as u16;
        r.preview_scroll = prev_scroll as u16;
        if let Some(e) = r.edit.as_mut() {
            e.last_drawn_cursor = Some(cursor);
        }
    }
}

/// Render the git lens overlay: each diff row is one display line, with
/// added rows on a green background, removed on red, hunk headers muted.
/// The viewport scrolls via `git_lens.scroll`.
fn draw_git_lens(f: &mut Frame, app: &App, area: Rect) {
    use ratatui::style::Color;
    let Some(g) = app.git_lens.as_ref() else {
        return;
    };
    let theme = &app.opts.theme;
    let added_bg = Color::Rgb(0x2d, 0x4f, 0x2d);
    let removed_bg = Color::Rgb(0x5a, 0x2d, 0x2d);
    let visible_h = area.height as usize;
    let scroll = (g.scroll as usize).min(g.rows.len().saturating_sub(1));

    let mut display_lines: Vec<Line> = Vec::with_capacity(visible_h);
    for i in 0..visible_h {
        let idx = scroll + i;
        if idx >= g.rows.len() {
            break;
        }
        let row = &g.rows[idx];
        let style = match row.kind {
            DiffRowKind::Added => Style::default().bg(added_bg),
            DiffRowKind::Removed => Style::default().bg(removed_bg),
            DiffRowKind::Hunk => Style::default()
                .fg(theme.heading[0])
                .add_modifier(Modifier::BOLD),
            DiffRowKind::Header => Style::default().fg(theme.muted),
            DiffRowKind::Info => Style::default()
                .fg(theme.muted)
                .add_modifier(Modifier::BOLD),
            DiffRowKind::Context => Style::default(),
        };
        // Pad rows to full width so the bg color extends the whole line.
        let visible_w = unicode_width::UnicodeWidthStr::width(row.text.as_str());
        let pad = (area.width as usize).saturating_sub(visible_w);
        let mut spans: Vec<Span<'static>> = Vec::new();
        spans.push(Span::styled(row.text.clone(), style));
        if pad > 0 {
            spans.push(Span::styled(" ".repeat(pad), style));
        }
        display_lines.push(Line::from(spans));
    }
    f.render_widget(Paragraph::new(display_lines), area);
}

/// Render any visible images in the document on top of the body, using the
/// terminal's detected graphics protocol. No-op if the picker isn't available
/// or if the image fails to decode.
fn draw_images_overlay(f: &mut Frame, app: &mut App, body: Rect) {
    use ratatui_image::StatefulImage;

    if app.image_picker.is_none() {
        return;
    }
    let (images, scroll) = match &app.view {
        View::Reader(r) => match &r.rendered {
            Some(rd) => (rd.images.clone(), r.scroll as i32),
            None => return,
        },
        _ => return,
    };

    for img in &images {
        let rel_y = img.line as i32 - scroll;
        if rel_y < 0 || rel_y as u16 >= body.height {
            continue;
        }
        let path = match std::fs::canonicalize(&img.source) {
            Ok(p) => p,
            Err(_) => continue,
        };
        if !app.image_protocols.contains_key(&path) {
            let dyn_img = match image::ImageReader::open(&path) {
                Ok(rdr) => match rdr.decode() {
                    Ok(d) => d,
                    Err(_) => continue,
                },
                Err(_) => continue,
            };
            let proto = match app.image_picker.as_ref() {
                Some(p) => p.new_resize_protocol(dyn_img),
                None => continue,
            };
            app.image_protocols.insert(path.clone(), proto);
        }
        let proto = match app.image_protocols.get_mut(&path) {
            Some(p) => p,
            None => continue,
        };
        let max_h = body.height.saturating_sub(rel_y as u16);
        let h = 12u16.min(max_h);
        if h == 0 {
            continue;
        }
        let area = Rect {
            x: body.x,
            y: body.y + rel_y as u16,
            width: body.width.saturating_sub(1),
            height: h,
        };
        f.render_stateful_widget(StatefulImage::default(), area, proto);
    }
}

/// Vertical scrollbar with a thumb sized proportionally to the visible viewport
/// (`thumb_h ≈ visible_h * track_h / total`). When content fits entirely the
/// thumb fills the track.
fn draw_scrollbar(
    f: &mut Frame,
    area: Rect,
    scroll: usize,
    total: usize,
    visible_h: usize,
    theme: &crate::tui::theme::Theme,
) {
    let track_h = area.height as usize;
    if track_h == 0 || area.width == 0 {
        return;
    }
    // Nothing to scroll → no bar at all. A full-height thumb only adds noise
    // on pages that already fit.
    if total <= visible_h {
        return;
    }

    // Paint the cell BACKGROUND instead of relying on a `█`/`│` glyph: many
    // terminals add line-spacing padding between rows that no character can
    // cover, so a glyph-based bar appears as separate ticks. The cell bg fills
    // the entire cell (padding included), so a space with `.bg()` produces a
    // visually continuous column.
    let track_style = Style::default().bg(theme.muted);
    let thumb_style = Style::default().bg(theme.heading[0]);

    let (thumb_top, thumb_h) = {
        let h = ((track_h * visible_h) / total).max(1).min(track_h);
        let max_scroll = total - visible_h;
        let span = track_h - h;
        let top = if max_scroll == 0 {
            0
        } else {
            (scroll * span + max_scroll / 2) / max_scroll
        };
        (top.min(span), h)
    };

    let lines: Vec<Line> = (0..track_h)
        .map(|i| {
            let in_thumb = i >= thumb_top && i < thumb_top + thumb_h;
            let style = if in_thumb { thumb_style } else { track_style };
            Line::from(Span::styled(" ", style))
        })
        .collect();
    f.render_widget(Paragraph::new(lines), area);
}

fn highlight_focused(
    line: &mut Line<'_>,
    link: &crate::tui::links::LinkSpan,
    theme: &crate::tui::theme::Theme,
) {
    let mut col = 0usize;
    for span in &mut line.spans {
        let w = unicode_width::UnicodeWidthStr::width(span.content.as_ref());
        let span_start = col;
        let span_end = col + w;
        if span_start >= link.col_start && span_end <= link.col_end {
            span.style = span
                .style
                .fg(theme.link_focused)
                .add_modifier(Modifier::REVERSED);
        }
        col = span_end;
    }
}

/// Paint a match span. Non-current matches get the code-block background
/// (subtle); the current match gets the link-focus color reversed.
fn highlight_doc_match(
    line: &mut Line<'_>,
    col_start: usize,
    col_end: usize,
    is_current: bool,
    theme: &crate::tui::theme::Theme,
) {
    let mut col = 0usize;
    for span in &mut line.spans {
        let w = unicode_width::UnicodeWidthStr::width(span.content.as_ref());
        let span_start = col;
        let span_end = col + w;
        if span_start >= col_start && span_end <= col_end {
            span.style = if is_current {
                span.style
                    .fg(theme.link_focused)
                    .add_modifier(Modifier::REVERSED)
                    .add_modifier(Modifier::BOLD)
            } else {
                span.style.add_modifier(Modifier::REVERSED)
            };
        }
        col = span_end;
    }
}

/// Paint reverse-video over spans that fall within `[col_start, col_end)` to
/// signal a hovered checkbox marker.
fn highlight_checkbox_hover(line: &mut Line<'_>, col_start: usize, col_end: usize) {
    let mut col = 0usize;
    for span in &mut line.spans {
        let w = unicode_width::UnicodeWidthStr::width(span.content.as_ref());
        let span_start = col;
        let span_end = col + w;
        if span_start >= col_start && span_end <= col_end {
            span.style = span.style.add_modifier(Modifier::REVERSED);
        }
        col = span_end;
    }
}

/// One row of a hand-highlighted list. Reproduces what ratatui's
/// `highlight_style` does to the selected row: `hl` overrides every span's
/// own color (a span-level style would otherwise sit on top of the row
/// style), and the row style paints the bar across the full width.
fn list_row(mut spans: Vec<Span<'static>>, selected: bool, hl: Style) -> ListItem<'static> {
    if selected {
        for s in &mut spans {
            s.style = hl;
        }
    }
    let item = ListItem::new(Line::from(spans));
    if selected { item.style(hl) } else { item }
}

fn draw_browser(f: &mut Frame, app: &App, area: Rect) {
    let View::Browser(b) = &app.view else {
        return;
    };
    let theme = &app.opts.theme;
    let title = format!(" {} ", b.dir.display());
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(Style::default().fg(theme.muted));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let badge_style = Style::default()
        .fg(theme.heading[3])
        .add_modifier(Modifier::BOLD);
    // Selection is styled by hand (no `ListState` selection): the wheel
    // scrolls the viewport independently of the cursor, and ratatui's List
    // would otherwise snap the offset back to keep the selection visible.
    let hl = Style::default()
        .bg(theme.status_bg)
        .fg(theme.status_fg)
        .add_modifier(Modifier::BOLD);
    let items: Vec<ListItem> = b
        .entries
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let selected = i == b.selected;
            let prefix = if selected { "▶ " } else { "  " };
            let mut spans = vec![
                Span::raw(prefix),
                Span::styled(e.display.clone(), browser_entry_style(e.kind, theme)),
            ];
            let unread = match e.kind {
                BrowserEntryKind::Markdown => app.read_state.is_unread(&e.path),
                BrowserEntryKind::Dir => app.read_state.dir_has_unread(&e.path),
                BrowserEntryKind::ParentDir => false,
            };
            if unread {
                spans.push(Span::styled(" [unread]", badge_style));
            }
            list_row(spans, selected, hl)
        })
        .collect();
    let mut state = ListState::default().with_offset(b.scroll as usize);
    f.render_stateful_widget(List::new(items), inner, &mut state);
    draw_scrollbar(
        f,
        scrollbar_track(area, inner),
        b.scroll as usize,
        b.entries.len(),
        inner.height as usize,
        theme,
    );
}

/// The right-border column of a bordered block, aligned with the list
/// rows (`inner` — already past the tab bar, if any) — where the list
/// scrollbar lives.
fn scrollbar_track(area: Rect, inner: Rect) -> Rect {
    Rect {
        x: area.x + area.width.saturating_sub(1),
        y: inner.y,
        width: 1.min(area.width),
        height: inner.height,
    }
}

/// Render the HackMD note browser: one tab per workspace ("My notes" +
/// each team), with the tab bar shown only when there's more than one.
/// Every note gets a visibility badge (published / anyone with link /
/// signed in / only team / only me).
fn draw_cloud_browser(f: &mut Frame, app: &mut App, area: Rect) {
    app.cloud_tab_hits.clear();
    let View::Cloud(c) = &app.view else {
        return;
    };
    let theme = &app.opts.theme;
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" hackmd.io ")
        .border_style(Style::default().fg(theme.muted));
    let mut inner = block.inner(area);
    f.render_widget(block, area);

    if c.tabs.is_empty() {
        let msg = if app.cloud.is_connected() {
            "Loading notes from hackmd.io…"
        } else {
            app::NO_TOKEN_HINT
        };
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                msg,
                Style::default().fg(theme.muted),
            ))),
            inner,
        );
        return;
    }

    // Tab bar (only with multiple workspaces). Built by hand instead of the
    // Tabs widget so each label's column range can be recorded for clicks.
    let mut tab_hits = Vec::new();
    if c.show_tab_bar() && inner.height > 0 {
        let mut spans = Vec::new();
        let mut x = inner.x;
        for (i, t) in c.tabs.iter().enumerate() {
            if i > 0 {
                spans.push(Span::styled(" │ ", Style::default().fg(theme.muted)));
                x += 3;
            }
            let label = format!(" {} ", t.label);
            let w = label.chars().count() as u16;
            tab_hits.push((x, x + w.saturating_sub(1), i));
            let style = if i == c.active {
                Style::default()
                    .bg(theme.status_bg)
                    .fg(theme.status_fg)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.muted)
            };
            spans.push(Span::styled(label, style));
            x += w;
        }
        let bar = Rect::new(inner.x, inner.y, inner.width, 1);
        f.render_widget(Paragraph::new(Line::from(spans)), bar);
        inner.y += 1;
        inner.height = inner.height.saturating_sub(1);
    }

    let Some(tab) = c.tab() else {
        return;
    };
    // Selection styled by hand for the same reason as the file browser:
    // wheel scrolling moves the viewport without dragging the cursor along.
    let hl = Style::default()
        .bg(theme.status_bg)
        .fg(theme.status_fg)
        .add_modifier(Modifier::BOLD);
    let items: Vec<ListItem> = tab
        .notes
        .iter()
        .enumerate()
        .map(|(i, n)| {
            let selected = i == tab.selected;
            // Loudest exposure gets the loudest style: published is
            // bold, link-shared uses the link color, private is muted.
            let badge_style = match n.visibility {
                "published" => Style::default()
                    .fg(theme.heading[3])
                    .add_modifier(Modifier::BOLD),
                "only me" | "only team" => Style::default().fg(theme.muted),
                _ => Style::default().fg(theme.link),
            };
            list_row(
                vec![
                    Span::raw(if selected { "▶ " } else { "  " }),
                    Span::raw(n.title.clone()),
                    Span::styled(format!(" [{}]", n.visibility), badge_style),
                ],
                selected,
                hl,
            )
        })
        .collect();
    let mut state = ListState::default().with_offset(tab.scroll as usize);
    f.render_stateful_widget(List::new(items), inner, &mut state);
    draw_scrollbar(
        f,
        scrollbar_track(area, inner),
        tab.scroll as usize,
        tab.notes.len(),
        inner.height as usize,
        theme,
    );
    app.cloud_tab_hits = tab_hits;
}

fn browser_entry_style(kind: BrowserEntryKind, theme: &crate::tui::theme::Theme) -> Style {
    match kind {
        BrowserEntryKind::ParentDir => Style::default().fg(theme.muted),
        BrowserEntryKind::Dir => Style::default()
            .fg(theme.heading[0])
            .add_modifier(Modifier::BOLD),
        BrowserEntryKind::Markdown => Style::default(),
    }
}

fn draw_search(f: &mut Frame, app: &App, area: Rect) {
    let Some(s) = &app.search else {
        return;
    };
    let theme = &app.opts.theme;

    let w = area.width.saturating_sub(8).max(20).min(100);
    let h = area.height.saturating_sub(4).max(8);
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    let popup = Rect {
        x,
        y,
        width: w,
        height: h,
    };

    f.render_widget(Clear, popup);

    let title = format!(" Search [{}] ", s.results.len());
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(Style::default().fg(theme.heading[0]));
    let inner = block.inner(popup);
    f.render_widget(block, popup);

    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(2), Constraint::Min(1)])
        .split(inner);

    let prompt = Line::from(vec![
        Span::styled(
            "▸ ",
            Style::default()
                .fg(theme.heading[0])
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(s.query.clone()),
        Span::styled("█", Style::default().fg(theme.heading[0])),
        Span::styled(
            format!("   {}", short_root(&app.root)),
            Style::default().fg(theme.muted),
        ),
    ]);
    f.render_widget(Paragraph::new(prompt), layout[0]);

    let items: Vec<ListItem> = s
        .results
        .iter()
        .map(|r| {
            let style = if r.is_dir {
                Style::default()
                    .fg(theme.heading[0])
                    .add_modifier(Modifier::BOLD)
            } else if app::is_markdown_file(&r.path) {
                Style::default()
            } else {
                Style::default().fg(theme.muted)
            };
            let display = if r.is_dir {
                format!("{}/", r.display)
            } else {
                r.display.clone()
            };
            ListItem::new(Span::styled(display, style))
        })
        .collect();

    let list = List::new(items)
        .highlight_style(
            Style::default()
                .bg(theme.status_bg)
                .fg(theme.status_fg)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▶ ");
    let mut state = ListState::default().with_selected(Some(s.selected));
    f.render_stateful_widget(list, layout[1], &mut state);
}

fn short_root(p: &std::path::Path) -> String {
    if let Some(home) = dirs::home_dir() {
        if let Ok(rel) = p.strip_prefix(&home) {
            return format!("~/{}", rel.display());
        }
    }
    p.display().to_string()
}

/// Single-row context-aware statusline. Anatomy from left to right:
///   ` ‹ Back  path/file.md   <middle>   25% `
/// where `<middle>` resolves in priority order to the doc-search prompt, the
/// hover-URL, the keyboard-focus URL, an explicit status message, or a hint
/// snippet of the most useful current shortcuts.
///
/// Edge case (per user request): when the hovered link sits on the bottom-most
/// body row (visually adjacent to this statusline), the URL gets pulled to the
/// opposite half of the row from the mouse column so it doesn't crowd the
/// pointer.
fn draw_statusline(f: &mut Frame, app: &mut App, area: Rect) {
    use unicode_width::UnicodeWidthStr;

    if area.width == 0 || area.height == 0 {
        return;
    }
    let theme = &app.opts.theme;

    // Vim-style command line: after the first Esc in edit mode the whole
    // statusline becomes the command place — typed text after a ':', the
    // best completion continuing in grey, and the non-vim escape hatches
    // spelled out for users who don't know :wq.
    if let View::Reader(r) = &app.view {
        if let Some(input) = r.edit.as_ref().and_then(|e| e.command.as_deref()) {
            draw_edit_command_line(f, theme, area, input);
            return;
        }
    }

    let bg = Style::default().bg(theme.status_bg).fg(theme.status_fg);
    let path_style = Style::default().add_modifier(Modifier::BOLD);
    let muted = Style::default().fg(theme.muted);

    // Path (or browser dir) — same display logic as before.
    let path = match &app.view {
        View::Reader(r) => match &r.origin {
            crate::tui::app::ReaderOrigin::File(p) => display_path(p, &app.root),
            crate::tui::app::ReaderOrigin::Stdin => "<stdin>".to_string(),
            crate::tui::app::ReaderOrigin::CloudNote { title, .. } => format!("☁ {title}"),
        },
        View::Browser(b) => format!("{}/", display_path(&b.dir, &app.root)),
        View::Cloud(_) => "☁ hackmd.io".to_string(),
    };
    // The untruncated path a click on the statusline path should copy.
    // Cloud notes and stdin have no filesystem path to offer.
    let full_path: Option<String> = match &app.view {
        View::Reader(r) => match &r.origin {
            crate::tui::app::ReaderOrigin::File(p) => Some(p.display().to_string()),
            _ => None,
        },
        View::Browser(b) => Some(b.dir.display().to_string()),
        View::Cloud(_) => None,
    };

    let scroll_pos = match &app.view {
        View::Reader(r) => {
            let total = r.rendered.as_ref().map(|x| x.lines.len()).unwrap_or(0);
            let h = app.viewport.height as usize;
            if total == 0 || total <= h {
                "All".to_string()
            } else {
                let max_scroll = total - h;
                let pct = ((r.scroll as usize) * 100 / max_scroll).min(100);
                format!("{}%", pct)
            }
        }
        View::Browser(b) => format!("{}/{}", b.selected + 1, b.entries.len().max(1)),
        View::Cloud(c) => match c.tab() {
            Some(t) => format!("{}/{}", t.selected + 1, t.notes.len().max(1)),
            None => "0/0".to_string(),
        },
    };

    // Resolve middle content + classify the mode (search / hover / hint).
    let middle = compute_middle(app);

    // Right span: scroll %.
    let right = Span::styled(format!(" {} ", scroll_pos), bg);
    let right_w = UnicodeWidthStr::width(right.content.as_ref());

    // Left span: edit-mode badge (if editing) OR optional back button +
    // path. The edit badge takes precedence over Back so the user always
    // knows they're in a mutating mode.
    let mut back_span: Option<Span> = None;
    app.back_button_hit = None;
    app.statusline_url_hit = None;
    app.statusline_path_hit = None;
    let edit_badge: Option<Span> = if let View::Reader(r) = &app.view {
        r.edit.as_ref().map(|e| {
            let label = if e.dirty { " EDIT* " } else { " EDIT " };
            Span::styled(
                label.to_string(),
                Style::default()
                    .bg(theme.heading[0])
                    .fg(theme.status_fg)
                    .add_modifier(Modifier::BOLD),
            )
        })
    } else {
        None
    };
    let lens_badge: Option<Span> = if app.git_lens.is_some() {
        Some(Span::styled(
            " GIT LENS ".to_string(),
            Style::default()
                .bg(theme.heading[1])
                .fg(theme.status_fg)
                .add_modifier(Modifier::BOLD),
        ))
    } else {
        None
    };
    let edit_badge = edit_badge.or(lens_badge);
    if edit_badge.is_none() && !app.history.is_empty() {
        let label = " ‹ Back ";
        let start_x = area.x;
        let end_x = area.x + label.chars().count() as u16;
        back_span = Some(Span::styled(
            label.to_string(),
            bg.add_modifier(Modifier::BOLD),
        ));
        app.back_button_hit = Some((start_x, end_x));
    }
    let left_badge = edit_badge.or(back_span);

    // Detect whether we should apply the bottom-row hover edge case: the
    // hovered link sits on the last body row, and the user's mouse is over it.
    let edge_swap = compute_edge_swap(app, &middle);

    // Actually render. Two layouts:
    //   - default: [back] [path]   middle   right
    //   - edge_swap to right: [back] [path]              [middle][right]
    //   - edge_swap to left:  [middle][gap][path]        [right]
    let total_w = area.width as usize;

    let mut line_spans: Vec<Span<'static>> = Vec::new();
    let mid_text = middle.text();
    // When the middle is a URL, record the screen column range it occupies so
    // a click on it can copy the (untruncated) target. `record_url_hit` reads
    // the accumulated span width to learn where the next span will start.
    let is_url = matches!(middle, Mid::Url { .. });
    // Returns the hit region for the URL span about to be pushed: it starts at
    // `area.x + <accumulated width>` and spans `width` columns. Returns None
    // when the middle isn't a URL.
    let url_hit = |spans: &[Span<'static>], width: usize| -> Option<(u16, u16, String)> {
        if is_url {
            let start_x = area.x + span_width(spans) as u16;
            Some((start_x, start_x + width as u16, mid_text.clone()))
        } else {
            None
        }
    };
    let mut url_hit_region: Option<(u16, u16, String)> = None;
    let mut path_cols: Option<(usize, usize)> = None;
    match edge_swap {
        EdgeSwap::Right => {
            // URL pinned to the right of the row (just before scroll%).
            path_cols = Some(push_left(&mut line_spans, &left_badge, &path, path_style));
            let mid_styled = Span::styled(format!(" {} ", mid_text), middle.style(theme));
            let mid_w = UnicodeWidthStr::width(mid_styled.content.as_ref());
            let used = span_width(&line_spans) + mid_w + right_w;
            line_spans.push(Span::raw(" ".repeat(total_w.saturating_sub(used))));
            url_hit_region = url_hit(&line_spans, mid_w);
            line_spans.push(mid_styled);
            line_spans.push(right);
        }
        EdgeSwap::Left => {
            // URL takes the left of the row, suppressing the path.
            if let Some(b) = left_badge.clone() {
                line_spans.push(b);
                line_spans.push(Span::raw(" "));
            }
            let mid_styled = Span::styled(format!(" {} ", mid_text), middle.style(theme));
            let mid_w = UnicodeWidthStr::width(mid_styled.content.as_ref());
            url_hit_region = url_hit(&line_spans, mid_w);
            line_spans.push(mid_styled);
            let used = span_width(&line_spans) + right_w;
            line_spans.push(Span::raw(" ".repeat(total_w.saturating_sub(used))));
            line_spans.push(right);
        }
        EdgeSwap::None => {
            // Default layout: [back] [path]   <middle>   <right>
            path_cols = Some(push_left(&mut line_spans, &left_badge, &path, path_style));
            if !mid_text.is_empty() {
                let pad_left = 2usize;
                let used_left = span_width(&line_spans) + pad_left;
                let max_mid = total_w
                    .saturating_sub(used_left)
                    .saturating_sub(right_w + 2);
                let truncated = truncate_mid(&mid_text, max_mid);
                let mid_w = UnicodeWidthStr::width(truncated.as_str());
                line_spans.push(Span::raw(" ".repeat(pad_left)));
                url_hit_region = url_hit(&line_spans, mid_w);
                line_spans.push(Span::styled(truncated, middle.style(theme)));
            }
            let used = span_width(&line_spans) + right_w;
            line_spans.push(Span::raw(" ".repeat(total_w.saturating_sub(used))));
            line_spans.push(right);
        }
    }
    app.statusline_url_hit = url_hit_region;
    if let (Some((ps, pe)), Some(full)) = (path_cols, full_path) {
        let start = area.x + ps as u16;
        let end = (area.x + pe as u16).min(area.x + area.width);
        if start < end {
            app.statusline_path_hit = Some((start, end, path.clone(), full));
        }
    }

    let _ = muted;
    f.render_widget(Paragraph::new(Line::from(line_spans)), area);

    // In-progress drag over the path: reverse-video the selected columns so
    // the user can see what mouse-up will copy.
    if let (Some((s, e, _, _)), Some((a, b))) =
        (app.statusline_path_hit.as_ref(), app.statusline_path_drag)
    {
        let lo = a.min(b).max(*s);
        let hi = a.max(b).min(e.saturating_sub(1));
        let buf = f.buffer_mut();
        for x in lo..=hi {
            let cell = &mut buf[(x, area.y)];
            cell.set_style(cell.style().add_modifier(Modifier::REVERSED));
        }
    }
}

/// The editor command line, drawn over the whole statusline row. Layout:
/// ` :typed▏completion-ghost   hints…` with the right edge carrying the
/// run/complete keys while the user is typing.
fn draw_edit_command_line(
    f: &mut Frame,
    theme: &crate::tui::theme::Theme,
    area: Rect,
    input: &str,
) {
    use unicode_width::UnicodeWidthStr;
    let bg = Style::default().bg(theme.status_bg).fg(theme.status_fg);
    let bold = bg.add_modifier(Modifier::BOLD);
    let grey = Style::default().bg(theme.status_bg).fg(theme.muted);

    let mut spans: Vec<Span<'static>> = vec![
        Span::styled(" :", bold),
        Span::styled(input.to_string(), bold),
        // The "cursor" — the command place owns the input now.
        Span::styled("▏", bg),
    ];
    if let Some(full) = crate::tui::events::complete_edit_command(input) {
        spans.push(Span::styled(full[input.len()..].to_string(), grey));
        spans.push(Span::styled("  (Tab to complete)", grey));
    }
    let right = if input.is_empty() {
        "type a vim command — :wq save+quit · :q discard · :preview · Esc again discards "
    } else {
        "Enter:run  Esc:discard+exit "
    };
    let right_w = UnicodeWidthStr::width(right);
    let used = span_width(&spans);
    let pad = (area.width as usize)
        .saturating_sub(used)
        .saturating_sub(right_w);
    spans.push(Span::styled(" ".repeat(pad), bg));
    spans.push(Span::styled(right.to_string(), grey));
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

#[derive(Clone, Debug)]
enum Mid {
    Hint(String),
    /// Static status message (e.g. "Copied: ...", "File reloaded").
    Status(String),
    /// `/query_` while typing, or `/query  [n/m]` after commit.
    Search(String),
    /// Hovered or focused link target.
    Url {
        text: String,
        on_last_row: bool,
    },
}

#[derive(Clone, Copy, Debug)]
enum EdgeSwap {
    None,
    Left,
    Right,
}

impl Mid {
    fn text(&self) -> String {
        match self {
            Mid::Hint(s) | Mid::Status(s) | Mid::Search(s) => s.clone(),
            Mid::Url { text, .. } => text.clone(),
        }
    }
    fn style(&self, theme: &crate::tui::theme::Theme) -> Style {
        match self {
            Mid::Url { .. } => Style::default()
                .fg(theme.link)
                .add_modifier(Modifier::UNDERLINED),
            Mid::Search(_) => Style::default().fg(theme.heading[0]),
            Mid::Status(_) => Style::default()
                .fg(theme.heading[0])
                .add_modifier(Modifier::BOLD),
            Mid::Hint(_) => Style::default().fg(theme.muted),
        }
    }
}

fn compute_middle(app: &App) -> Mid {
    // `:preview` overlay: a dedicated hint so the user knows the buffer is
    // unsaved and how to get back. (The command line itself never reaches
    // this function — draw_statusline renders it as a full row.)
    if let View::Reader(r) = &app.view {
        if let Some(e) = r.edit.as_ref() {
            if e.preview_full {
                return Mid::Hint("preview of unsaved edit  j/k scroll  Esc back to editor".into());
            }
        }
    }
    // An active mouse hover over a link shows its target immediately — even
    // over a sticky status message ("Opened …", "Copied …", "File reloaded").
    // Hovering is a live gesture and the user wants feedback on what they're
    // pointing at; moving the mouse away restores the status. Suppressed while
    // editing or typing a search, which own the middle. Keyboard focus stays a
    // lower-priority fallback below the status (see the link block further on).
    if let View::Reader(r) = &app.view {
        let busy = r.edit.is_some() || r.doc_search.as_ref().map(|s| s.editing).unwrap_or(false);
        if !busy {
            if let (Some(rendered), Some(hi)) = (r.rendered.as_ref(), r.hover_link) {
                if let Some(link) = rendered.link_map.links.get(hi) {
                    let visible_h = app.viewport.height as usize;
                    let scroll = r.scroll as usize;
                    let last_row_idx = scroll + visible_h.saturating_sub(1);
                    let on_last_row = link.line == last_row_idx;
                    return Mid::Url {
                        text: describe_target(&link.target),
                        on_last_row,
                    };
                }
            }
        }
    }
    // Browser type-ahead find: show the live query and whether it matches.
    if let View::Browser(b) = &app.view {
        if let Some(q) = b.find.as_ref() {
            let matched = !q.is_empty() && b.find_from(q, 0).is_some();
            let txt = if q.is_empty() {
                "find: _  (type to jump  ;/, next/prev  Esc cancel)".to_string()
            } else if matched {
                format!("find: {q}_  (;/, next/prev  Enter open  Esc cancel)")
            } else {
                format!("find: {q}_  (no match)")
            };
            return Mid::Search(txt);
        }
    }
    if !app.status.is_empty() {
        return Mid::Status(app.status.clone());
    }
    // Cloud operations in flight — keep the user informed without blocking.
    if app.cloud.pending > 0 {
        return Mid::Status("⟳ syncing…".into());
    }
    if app.git_lens.is_some() {
        return Mid::Hint("git lens (vs HEAD)  j/k scroll  Ctrl-G or Esc to dismiss".into());
    }
    if app.toc.is_some() {
        return Mid::Hint("contents  j/k move  Enter jump  Esc close".into());
    }
    if let View::Reader(r) = &app.view {
        // Edit-mode hint replaces the normal viewer hint when active.
        if let Some(e) = r.edit.as_ref() {
            // An active drag-selection swaps in its action menu.
            if e.selection.as_ref().map(|s| s.is_active()).unwrap_or(false) {
                return Mid::Hint(
                    "selection  y copy  x cut  p paste  Del delete  Esc cancel".into(),
                );
            }
            return Mid::Hint(
                "type to edit  Ctrl-S save  Alt-←/→ word  Ctrl-Z undo  Esc command".into(),
            );
        }
        if let Some(s) = r.doc_search.as_ref() {
            let txt = if s.editing {
                format!("/{}_", s.query)
            } else if s.matches.is_empty() {
                format!("no match: /{}", s.query)
            } else {
                format!("/{}  [{}/{}]", s.query, s.current + 1, s.matches.len())
            };
            return Mid::Search(txt);
        }
        if let Some(rendered) = r.rendered.as_ref() {
            // Active hover is handled earlier (it outranks a sticky status); here
            // we fall back to the keyboard-focused link when nothing's hovered.
            let pick = r.hover_link.or_else(|| match r.focus {
                Some(Focus::Link(i)) => Some(i),
                _ => None,
            });
            if let Some(hi) = pick {
                if let Some(link) = rendered.link_map.links.get(hi) {
                    let visible_h = app.viewport.height as usize;
                    let scroll = r.scroll as usize;
                    let last_row_idx = scroll + visible_h.saturating_sub(1);
                    let on_last_row = link.line == last_row_idx;
                    return Mid::Url {
                        text: describe_target(&link.target),
                        on_last_row,
                    };
                }
            }
        }
    }
    Mid::Hint(default_hint(app))
}

fn default_hint(app: &App) -> String {
    match &app.view {
        // Most important keys first; the full reference lives behind `?`.
        View::Reader(r) => match &r.origin {
            ReaderOrigin::CloudNote { .. } => {
                "j/k:scroll  /:find  t:toc  e:edit  y:link  ?:help  q:quit".into()
            }
            _ => "j/k:scroll  /:find  e:edit  U:publish  Tab:links  ?:help  q:quit".into(),
        },
        View::Browser(_) => {
            "j/k  Enter:open  n:new  U:publish  c:rename  f:find  H:hackmd  ?:help".into()
        }
        View::Cloud(c) => {
            if c.show_tab_bar() {
                "j/k  Enter:open  Tab:workspace  R:refresh  H:local  ?:help  q:quit".into()
            } else {
                "j/k  Enter:open  R:refresh  H:local  ?:help  q:quit".into()
            }
        }
    }
}

fn compute_edge_swap(app: &App, middle: &Mid) -> EdgeSwap {
    match middle {
        Mid::Url {
            on_last_row: true, ..
        } => {
            let half = app.viewport.width / 2;
            // Mouse on the left half → push URL to the right (away from cursor).
            // Mouse on the right half → URL on the left (the user-defined default).
            if app.last_mouse_col < app.viewport.x + half {
                EdgeSwap::Right
            } else {
                EdgeSwap::Left
            }
        }
        _ => EdgeSwap::None,
    }
}

/// Push the left side of the statusline (badge/back + path) and return the
/// width range `[start, end)` the path span occupies within the line, so the
/// caller can record a click/drag hit region for it.
fn push_left(
    out: &mut Vec<Span<'static>>,
    back: &Option<Span<'static>>,
    path: &str,
    path_style: Style,
) -> (usize, usize) {
    if let Some(b) = back.clone() {
        out.push(b);
        out.push(Span::raw(" "));
    } else {
        out.push(Span::raw(" "));
    }
    let start = span_width(out);
    out.push(Span::styled(path.to_string(), path_style));
    (start, start + unicode_width::UnicodeWidthStr::width(path))
}

fn span_width(spans: &[Span<'_>]) -> usize {
    spans
        .iter()
        .map(|s| unicode_width::UnicodeWidthStr::width(s.content.as_ref()))
        .sum()
}

fn truncate_mid(s: &str, max: usize) -> String {
    use unicode_width::UnicodeWidthChar;
    if max == 0 {
        return String::new();
    }
    let total: usize = s.chars().map(|c| c.width().unwrap_or(0)).sum();
    if total <= max {
        return s.to_string();
    }
    let mut out = String::new();
    let mut w = 0usize;
    for ch in s.chars() {
        let cw = ch.width().unwrap_or(0);
        if w + cw + 1 > max {
            break;
        }
        out.push(ch);
        w += cw;
    }
    out.push('…');
    out
}

fn describe_target(t: &LinkTarget) -> String {
    match t {
        LinkTarget::Url(u) => u.clone(),
        LinkTarget::LocalFile(p) => p.display().to_string(),
        LinkTarget::Anchor(a) => format!("#{}", a),
        LinkTarget::FileAnchor(p, a) => format!("{}#{}", p.display(), a),
    }
}

/// Truncate `s` to at most `w` display columns (by char count — good enough
/// for the resolver) and right-pad with spaces to exactly `w`.
fn pad_truncate(s: &str, w: usize) -> String {
    let mut out: String = s.chars().take(w).collect();
    let len = out.chars().count();
    if len < w {
        out.push_str(&" ".repeat(w - len));
    }
    out
}

/// `(local_style, remote_style)` for a conflict hunk given the user's choice:
/// the kept side is bright green, the dropped side dim; an unresolved hunk
/// shows both in distinct inviting colours.
fn conflict_side_styles(choice: app::ConflictChoice) -> (Style, Style) {
    use app::ConflictChoice::*;
    let chosen = Style::default()
        .fg(Color::Green)
        .add_modifier(Modifier::BOLD);
    let dim = Style::default().fg(Color::DarkGray);
    match choice {
        Unresolved => (
            Style::default().fg(Color::Yellow),
            Style::default().fg(Color::Cyan),
        ),
        Local => (chosen, dim),
        Remote => (dim, chosen),
        Both => (chosen, chosen),
        Neither => (dim, dim),
    }
}

/// Full-screen three-way conflict resolver: stable context dimmed, each
/// conflict hunk shown as side-by-side local/upstream columns with the
/// focused hunk highlighted. Driven by [`crate::tui::events`]'s conflict key
/// handler.
fn draw_conflict(f: &mut Frame, app: &App, area: Rect) {
    let Some(st) = app.conflict.as_ref() else {
        return;
    };
    let theme = &app.opts.theme;
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // title
            Constraint::Min(1),    // body
            Constraint::Length(1), // hint
        ])
        .split(area);
    let (title_area, body_area, hint_area) = (chunks[0], chunks[1], chunks[2]);

    let total = st.conflict_count();
    let unresolved = st.unresolved_count();
    let title = format!(
        " Resolve conflicts — {total} hunk(s), {unresolved} unresolved  ({}) ",
        st.path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("file")
    );
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            title,
            Style::default()
                .fg(theme.heading[0])
                .add_modifier(Modifier::BOLD),
        ))),
        title_area,
    );

    let full_w = body_area.width as usize;
    let col_w = (full_w.saturating_sub(3) / 2).max(4);
    let ctx_style = Style::default().fg(Color::DarkGray);
    let mut lines: Vec<Line> = Vec::new();
    let mut conflict_idx = 0usize;
    let mut selected_line = 0usize;

    let push_stable = |lines: &mut Vec<Line>, s: &str| {
        let all: Vec<&str> = s.lines().collect();
        let emit = |lines: &mut Vec<Line>, t: &str| {
            let shown: String = t.chars().take(full_w.saturating_sub(2)).collect();
            lines.push(Line::from(Span::styled(format!("  {shown}"), ctx_style)));
        };
        // Collapse long unchanged runs so conflicts stay in view.
        if all.len() <= 6 {
            for t in &all {
                emit(lines, t);
            }
        } else {
            for t in &all[..3] {
                emit(lines, t);
            }
            lines.push(Line::from(Span::styled(
                format!("  ⋯ {} unchanged lines ⋯", all.len() - 6),
                ctx_style,
            )));
            for t in &all[all.len() - 3..] {
                emit(lines, t);
            }
        }
    };

    for item in &st.items {
        match item {
            app::ConflictItem::Stable(s) => push_stable(&mut lines, s),
            app::ConflictItem::Conflict {
                local,
                remote,
                choice,
            } => {
                let is_sel = conflict_idx == st.selected;
                if is_sel {
                    selected_line = lines.len();
                }
                let chosen = match choice {
                    app::ConflictChoice::Unresolved => "unresolved",
                    app::ConflictChoice::Local => "local",
                    app::ConflictChoice::Remote => "upstream",
                    app::ConflictChoice::Both => "both",
                    app::ConflictChoice::Neither => "drop",
                };
                let marker = if is_sel { "▶" } else { " " };
                let header_style = if is_sel {
                    Style::default()
                        .fg(theme.heading[1])
                        .add_modifier(Modifier::BOLD | Modifier::REVERSED)
                } else {
                    Style::default()
                        .fg(theme.heading[1])
                        .add_modifier(Modifier::BOLD)
                };
                lines.push(Line::from(Span::styled(
                    format!("{marker} conflict {}/{total}  [{chosen}]", conflict_idx + 1),
                    header_style,
                )));
                // Column captions aligned over the side-by-side body.
                lines.push(Line::from(vec![
                    Span::styled(
                        pad_truncate("  local", col_w),
                        Style::default().fg(Color::DarkGray),
                    ),
                    Span::styled(" │ ", ctx_style),
                    Span::styled("upstream", Style::default().fg(Color::DarkGray)),
                ]));
                let llines: Vec<&str> = local.split('\n').collect();
                let rlines: Vec<&str> = remote.split('\n').collect();
                let rows = llines.len().max(rlines.len());
                let (lstyle, rstyle) = conflict_side_styles(*choice);
                for i in 0..rows {
                    let l = pad_truncate(llines.get(i).copied().unwrap_or(""), col_w);
                    let r = pad_truncate(rlines.get(i).copied().unwrap_or(""), col_w);
                    lines.push(Line::from(vec![
                        Span::styled(l, lstyle),
                        Span::styled(" │ ", ctx_style),
                        Span::styled(r, rstyle),
                    ]));
                }
                conflict_idx += 1;
            }
        }
    }

    // Keep the focused hunk in view: scroll it into the upper third.
    let h = body_area.height as usize;
    let max_scroll = lines.len().saturating_sub(h.max(1));
    let scroll = selected_line.saturating_sub(h / 3).min(max_scroll);
    f.render_widget(Paragraph::new(lines).scroll((scroll as u16, 0)), body_area);

    let hint = " j/k move   l:local  u:upstream  b:both  n:drop   Enter:apply   Esc:cancel ";
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            hint,
            Style::default().fg(theme.heading[1]),
        ))),
        hint_area,
    );
}

fn draw_help(f: &mut Frame, app: &App, area: Rect) {
    let w = 66.min(area.width.saturating_sub(4));
    let h = 42.min(area.height.saturating_sub(4));
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    let popup = Rect {
        x,
        y,
        width: w,
        height: h,
    };
    f.render_widget(Clear, popup);
    let theme = &app.opts.theme;
    let section = |t: &str| {
        Line::from(Span::styled(
            t.to_string(),
            Style::default()
                .fg(theme.heading[1])
                .add_modifier(Modifier::BOLD),
        ))
    };
    let body = vec![
        section(" Reading"),
        Line::from("  j / k / ↓ ↑      scroll line          d / u   half page"),
        Line::from("  f / b / Space    page  (also Ctrl-F/B, Ctrl-D/U, PgDn/PgUp)"),
        Line::from("  gg / G           top / bottom         NG      go to line N"),
        Line::from("  ]] / [[          next / previous heading"),
        Line::from("  t                table of contents (Enter jumps)"),
        Line::from("  zz               center focus   H/M/L  focus top/mid/bot"),
        Line::from("  5j 10G 3]]       counts work on any motion"),
        Line::from(""),
        section(" Find"),
        Line::from("  /                search in doc (Reader) / files (Browser)"),
        Line::from("  n / N            next / prev match    T  fuzzy file search"),
        Line::from(""),
        section(" Links & checkboxes"),
        Line::from("  Tab / S-Tab      cycle focus across links + checkboxes"),
        Line::from("  Enter / →        follow link or toggle checkbox"),
        Line::from("  o                open focused link in system browser"),
        Line::from(""),
        section(" History"),
        Line::from("  h / l            back / forward       Ctrl-O  back (vim)"),
        Line::from("  Backspace / Esc  back  (Esc at root quits)"),
        Line::from(""),
        section(" Edit  (e/i open · A append · O open-above · split preview)"),
        Line::from("  Ctrl-S           save  (Ctrl-W fallback for XOFF terminals)"),
        Line::from("  Ctrl-Z / Ctrl-Y  undo / redo          Esc Esc  discard"),
        Line::from("  Alt-←/→          word jump    Alt-Bksp/Del  word delete"),
        Line::from("  drag select      y copy  x cut  p paste  Del delete  Esc cancel"),
        Line::from("  ( [ {            auto-close;  select + ( * _ ` [  wraps it"),
        Line::from("  Enter            continue list (-, 1., - [ ]); empty item ends it"),
        Line::from("  Esc              command — :w :wq :q :preview  :s/old/new/[g]"),
        Line::from("                   :%s/…/…/g all lines · ↑/↓ command history"),
        Line::from(""),
        section(" Local files"),
        Line::from("  n                new file (browser)   c / F2  rename"),
        Line::from("  U                publish to HackMD — links the file, then"),
        Line::from("                   auto-syncs both ways (on open, save & poll)"),
        Line::from("  conflicts        l:local u:upstream b:both n:drop Enter:apply"),
        Line::from("  Ctrl-G           git lens (diff vs HEAD; staged + unstaged)"),
        Line::from("  r                mark read (browser; dirs recurse)"),
        Line::from(""),
        section(if app.cloud.is_connected() {
            " HackMD cloud"
        } else {
            " HackMD cloud  (not logged in — run `hackmd login`)"
        }),
        Line::from("  H (browsers) / gh   toggle local ↔ hackmd.io"),
        Line::from("  Enter  open      R  refresh       Tab  workspace tab"),
        Line::from("  n  new note      D  delete        P  publish/unpublish"),
        Line::from("  y / o            copy / open the publish link"),
        Line::from("  S                download note to a local file"),
        Line::from(""),
        Line::from("  m  toggle mouse capture    q / Ctrl-C  quit    ?  help"),
    ];
    let block = Block::default().borders(Borders::ALL).title(" Help ");
    let para = Paragraph::new(body).block(block);
    f.render_widget(para, popup);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::{App, Options, Source, View};
    use crate::tui::theme::Theme;

    fn app_with_link() -> App {
        let mut p = std::env::temp_dir();
        p.push(format!("md-tui-ui-test-{}.md", std::process::id()));
        std::fs::write(
            &p,
            "a paragraph with a [site](https://example.com/x) here\n",
        )
        .unwrap();
        let opts = Options {
            width: 80,
            line_numbers: false,
            theme: Theme::dark(),
        };
        let mut app = App::new(Source::File(p), opts).unwrap();
        app.ensure_rendered(80);
        // compute_middle reads app.viewport.height to decide last-row edge case.
        app.viewport = Rect::new(0, 0, 80, 24);
        // Sanity: the rendered doc must contain exactly one link to hover.
        let View::Reader(r) = &app.view else {
            panic!("expected reader");
        };
        assert_eq!(r.rendered.as_ref().unwrap().link_map.links.len(), 1);
        app
    }

    // An active hover over a link must show the URL preview even when a sticky
    // status message ("Opened …", "Copied …") is present — the live gesture
    // wins. This is the regression the fix addresses.
    #[test]
    fn hover_url_outranks_sticky_status() {
        let mut app = app_with_link();
        app.status = "Copied: something".into();
        if let View::Reader(r) = &mut app.view {
            r.hover_link = Some(0);
        }
        match compute_middle(&app) {
            Mid::Url { text, .. } => assert_eq!(text, "https://example.com/x"),
            other => panic!("expected Url preview while hovering, got {:?}", other),
        }
    }

    // Rendering a file reader records a clickable hit region for the path on
    // the statusline, carrying the full path a click should copy.
    #[test]
    fn statusline_records_path_hit() {
        use ratatui::backend::TestBackend;
        let mut app = app_with_link();
        let mut term = ratatui::Terminal::new(TestBackend::new(80, 24)).unwrap();
        term.draw(|f| draw(f, &mut app)).unwrap();
        let (start, end, display, full) = app
            .statusline_path_hit
            .clone()
            .expect("file reader must expose a path hit");
        assert!(start < end);
        assert!(full.ends_with(".md"), "full path is the on-disk path");
        assert!(
            full.ends_with(&display) || display.ends_with(".md"),
            "display is derived from the same file"
        );
    }

    // A page that fits the viewport draws no scrollbar at all; a longer one
    // paints a track with a proportional thumb.
    #[test]
    fn scrollbar_hidden_when_content_fits() {
        use ratatui::backend::TestBackend;
        let theme = Theme::dark();
        let area = Rect::new(0, 0, 1, 10);
        let mut term = ratatui::Terminal::new(TestBackend::new(1, 10)).unwrap();

        let painted = |term: &ratatui::Terminal<TestBackend>| {
            let buf = term.backend().buffer();
            (0..10)
                .filter(|&y| buf[(0, y)].bg != ratatui::style::Color::Reset)
                .count()
        };

        // Content fits (8 lines in a 10-row viewport) → nothing painted.
        term.draw(|f| draw_scrollbar(f, area, 0, 8, 10, &theme))
            .unwrap();
        assert_eq!(painted(&term), 0, "no bar when the page fits");

        // Content overflows → the whole track is painted.
        term.draw(|f| draw_scrollbar(f, area, 0, 40, 10, &theme))
            .unwrap();
        assert_eq!(painted(&term), 10, "track + thumb fill the column");
    }

    // With nothing hovered, the sticky status still shows (hover doesn't
    // suppress it spuriously).
    #[test]
    fn status_shows_when_not_hovering() {
        let mut app = app_with_link();
        app.status = "Copied: something".into();
        if let View::Reader(r) = &mut app.view {
            r.hover_link = None;
        }
        match compute_middle(&app) {
            Mid::Status(s) => assert_eq!(s, "Copied: something"),
            other => panic!("expected status, got {:?}", other),
        }
    }
}
